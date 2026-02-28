package transport

import (
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"internex/internal/rewriter"
)

// AssetsDir is the path to the assets directory.  Set by cmd/server/main.go.
var AssetsDir string

// NewMux returns an http.ServeMux wired with all proxy / rewrite routes.
func NewMux() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /proxy", handleProxy)
	mux.HandleFunc("POST /rewrite/html", handleRewriteHTML)
	mux.HandleFunc("POST /rewrite/css", handleRewriteCSS)
	mux.HandleFunc("POST /rewrite/js", handleRewriteJS)
	mux.HandleFunc("/", handleStatic)
	return mux
}

// ---------- /proxy?url=<encoded> ----------

func handleProxy(w http.ResponseWriter, r *http.Request) {
	raw := r.URL.Query().Get("url")
	if raw == "" {
		http.Error(w, "missing 'url' query parameter", http.StatusBadRequest)
		return
	}

	// Decode & validate target URL.
	targetURL, ok := DecodeProxyURL(raw)
	if !ok {
		http.Error(w, "invalid target URL", http.StatusBadRequest)
		return
	}

	origin := ExtractOrigin(targetURL)

	// Attach per-origin cookies from our session store.
	cookieHeader := DefaultSessions.CookieHeader(origin)

	resp, err := FetchUpstreamWithCookies(targetURL, r.Method, r.Header, r.Body, cookieHeader)
	if err != nil {
		log.Printf("proxy fetch error: %v", err)
		http.Error(w, "upstream fetch failed", http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()

	// Store any Set-Cookie headers in our per-origin jar.
	DefaultSessions.SetCookiesFromResponse(origin, resp)

	// WebSocket upgrade — hijack and bridge.
	if resp.StatusCode == http.StatusSwitchingProtocols {
		hijackWebSocket(w, resp)
		return
	}

	// Copy upstream response headers with rewriting.
	CopyResponseHeadersWithContext(w.Header(), resp.Header, targetURL)

	// Detect content type and decide whether to rewrite.
	contentType := DetectContentType(resp.Header)
	category := Categorize(contentType)

	if r.Method == http.MethodHead {
		w.WriteHeader(resp.StatusCode)
		return
	}

	if category == ContentOther {
		// Not a rewritable type — stream straight through.
		w.WriteHeader(resp.StatusCode)
		io.Copy(w, resp.Body)
		return
	}

	// Read body for rewriting.
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		log.Printf("proxy body read error: %v", err)
		http.Error(w, "reading upstream body failed", http.StatusBadGateway)
		return
	}

	content := string(body)
	var result string

	switch category {
	case ContentHTML:
		result = rewriter.RewriteHTML(ProxyOrigin, targetURL, content)
	case ContentCSS:
		result = rewriter.RewriteCSS(ProxyOrigin, targetURL, content)
	case ContentJS:
		result = rewriter.RewriteJS(ProxyOrigin, targetURL, content)
	default:
		result = content
	}

	// Remove Content-Length since the rewritten size may differ.
	w.Header().Del("Content-Length")
	w.WriteHeader(resp.StatusCode)
	io.WriteString(w, result)
}

// hijackWebSocket takes over the client connection and bridges it
// bidirectionally with the upstream WebSocket connection.
func hijackWebSocket(w http.ResponseWriter, upResp *http.Response) {
	hj, ok := w.(http.Hijacker)
	if !ok {
		http.Error(w, "webSocket hijack not supported", http.StatusInternalServerError)
		return
	}

	clientConn, clientBuf, err := hj.Hijack()
	if err != nil {
		log.Printf("websocket hijack: %v", err)
		return
	}
	defer clientConn.Close()

	// Write the raw 101 response back to the client.
	_ = upResp.Write(clientConn)

	// upResp.Body is the raw upstream connection.
	upConn, ok := upResp.Body.(io.ReadWriteCloser)
	if !ok {
		log.Print("upstream body is not ReadWriteCloser")
		return
	}
	defer upConn.Close()

	// Bidirectional copy.
	done := make(chan struct{}, 2)
	copy := func(dst io.Writer, src io.Reader) {
		io.Copy(dst, src)
		done <- struct{}{}
	}

	// Flush anything the buffered reader already consumed.
	if clientBuf.Reader.Buffered() > 0 {
		buffered := make([]byte, clientBuf.Reader.Buffered())
		clientBuf.Read(buffered)
		upConn.Write(buffered)
	}

	go copy(upConn, clientConn)
	go copy(clientConn, upConn)
	<-done
}

// ---------- POST /rewrite/* ----------

func handleRewriteHTML(w http.ResponseWriter, r *http.Request) {
	rewriteBodyDirect(w, r, "html")
}

func handleRewriteCSS(w http.ResponseWriter, r *http.Request) {
	rewriteBodyDirect(w, r, "css")
}

func handleRewriteJS(w http.ResponseWriter, r *http.Request) {
	rewriteBodyDirect(w, r, "js")
}

func rewriteBodyDirect(w http.ResponseWriter, r *http.Request, kind string) {
	defer r.Body.Close()

	proxyOrigin := ProxyOrigin
	baseURL := r.URL.Query().Get("base") // optional base URL hint

	body, err := io.ReadAll(r.Body)
	if err != nil {
		log.Printf("rewrite body read error: %v", err)
		http.Error(w, "reading body failed", http.StatusBadRequest)
		return
	}

	content := string(body)
	var result string

	switch kind {
	case "html":
		result = rewriter.RewriteHTML(proxyOrigin, baseURL, content)
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
	case "css":
		result = rewriter.RewriteCSS(proxyOrigin, baseURL, content)
		w.Header().Set("Content-Type", "text/css; charset=utf-8")
	case "js":
		result = rewriter.RewriteJS(proxyOrigin, baseURL, content)
		w.Header().Set("Content-Type", "application/javascript; charset=utf-8")
	default:
		result = content
		w.Header().Set("Content-Type", "application/octet-stream")
	}

	io.WriteString(w, result)
}

// ---------- Static file serving ----------

var mimeTypes = map[string]string{
	".html": "text/html; charset=utf-8",
	".css":  "text/css; charset=utf-8",
	".js":   "application/javascript; charset=utf-8",
	".json": "application/json; charset=utf-8",
	".png":  "image/png",
	".svg":  "image/svg+xml",
	".ico":  "image/x-icon",
}

func handleStatic(w http.ResponseWriter, r *http.Request) {
	p := r.URL.Path
	if p == "/" {
		p = "/index.html"
	}

	// Strip leading "/" and prevent directory traversal.
	clean := filepath.Clean(strings.TrimPrefix(p, "/"))
	if strings.Contains(clean, "..") {
		http.Error(w, "Forbidden", http.StatusForbidden)
		return
	}

	fullPath := filepath.Join(AssetsDir, clean)
	data, err := os.ReadFile(fullPath)
	if err != nil {
		http.Error(w, "Not found", http.StatusNotFound)
		return
	}

	ext := filepath.Ext(fullPath)
	ct, ok := mimeTypes[ext]
	if !ok {
		ct = "application/octet-stream"
	}
	w.Header().Set("Content-Type", ct)
	w.Write(data)
}
