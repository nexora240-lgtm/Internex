package transport

import (
	"mime"
	"net/http"
	"net/url"
	"strings"
)

// ContentCategory is defined so the proxy handler can branch on it.
type ContentCategory int

const (
	ContentOther ContentCategory = iota
	ContentHTML
	ContentCSS
	ContentJS
)

// DetectContentType extracts the media type from an HTTP header set.
func DetectContentType(h http.Header) string {
	ct := h.Get("Content-Type")
	if ct == "" {
		return "application/octet-stream"
	}
	mediaType, _, _ := mime.ParseMediaType(ct)
	return mediaType
}

// Categorize maps a media-type string to a ContentCategory.
func Categorize(mediaType string) ContentCategory {
	switch {
	case strings.Contains(mediaType, "html"):
		return ContentHTML
	case mediaType == "text/css":
		return ContentCSS
	case strings.Contains(mediaType, "javascript"):
		return ContentJS
	default:
		return ContentOther
	}
}

// ---------------------------------------------------------------------------
// Request header forwarding
// ---------------------------------------------------------------------------

// safeRequestHeaders are the only headers forwarded from the browser to
// the upstream server.
var safeRequestHeaders = []string{
	"Accept",
	"Accept-Charset",
	"Accept-Language",
	"Accept-Encoding",
	"Content-Type",
	"Content-Length",
	"User-Agent",
	"Referer",
	"Origin",
	"Authorization",
	"X-Requested-With",
	"X-CSRF-Token",
	"If-Modified-Since",
	"If-None-Match",
	"If-Match",
	"If-Unmodified-Since",
	"Cache-Control",
	"Range",
	"DNT",
}

// forwardHeaders copies safe headers from src into dst.
func forwardHeaders(dst, src http.Header) {
	for _, k := range safeRequestHeaders {
		if v := src.Get(k); v != "" {
			dst.Set(k, v)
		}
	}
}

// ---------------------------------------------------------------------------
// Response header processing
// ---------------------------------------------------------------------------

// hopByHop headers that must not be forwarded.
var hopByHopHeaders = map[string]bool{
	"Connection":          true,
	"Keep-Alive":          true,
	"Proxy-Authenticate":  true,
	"Proxy-Authorization": true,
	"Te":                  true,
	"Trailer":             true,
	"Transfer-Encoding":   true,
	"Upgrade":             true,
}

// strippedSecurityHeaders are response headers that would prevent the
// proxied page from rendering inside our context.  They are removed
// entirely.
var strippedSecurityHeaders = map[string]bool{
	"Content-Security-Policy":             true,
	"Content-Security-Policy-Report-Only": true,
	"X-Content-Security-Policy":           true,
	"Cross-Origin-Opener-Policy":          true,
	"Cross-Origin-Embedder-Policy":        true,
	"Cross-Origin-Resource-Policy":        true,
	"X-Frame-Options":                     true,
	"Referrer-Policy":                     true,
	"Strict-Transport-Security":           true,
	"X-XSS-Protection":                    true,
	"Permissions-Policy":                  true,
}

// CopyResponseHeaders copies upstream response headers to the client
// writer, stripping hop-by-hop headers, security headers, and
// rewriting Location and Set-Cookie.
//
// targetURL is the original upstream URL (used to resolve relative
// Location redirects).
func CopyResponseHeaders(dst http.Header, src http.Header) {
	targetURL := "" // basic version without context
	CopyResponseHeadersWithContext(dst, src, targetURL)
}

// CopyResponseHeadersWithContext copies upstream response headers with
// full rewriting of Location and Set-Cookie.
func CopyResponseHeadersWithContext(dst http.Header, src http.Header, targetURL string) {
	proxyHost := strings.TrimPrefix(strings.TrimPrefix(ProxyOrigin, "https://"), "http://")

	for k, vv := range src {
		// Skip hop-by-hop.
		if hopByHopHeaders[k] {
			continue
		}
		// Strip security headers that block proxying.
		if strippedSecurityHeaders[k] {
			continue
		}

		switch strings.ToLower(k) {
		case "location":
			// Rewrite redirect targets through the proxy.
			for _, v := range vv {
				dst.Add(k, RewriteLocationHeader(targetURL, v))
			}

		case "set-cookie":
			// Rewrite cookie domain / attributes so the browser
			// stores them under the proxy's host.
			for _, v := range vv {
				rewritten := RewriteSetCookieDomain(v, proxyHost)
				dst.Add(k, rewritten)
			}

		case "content-length":
			// Will be re-set after rewriting if needed; skip for now.
			for _, v := range vv {
				dst.Add(k, v)
			}

		default:
			for _, v := range vv {
				dst.Add(k, v)
			}
		}
	}
}

// ExtractOrigin returns "scheme://host" from a full URL string.
func ExtractOrigin(rawURL string) string {
	u, err := url.Parse(rawURL)
	if err != nil {
		return ""
	}
	return u.Scheme + "://" + u.Host
}
