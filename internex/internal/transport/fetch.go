package transport

import (
	"crypto/tls"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// streamTransport is tuned for long-lived / streaming connections.
// ResponseHeaderTimeout is intentionally zero so streamed bodies are
// never cut short.
var streamTransport = &http.Transport{
	DialContext: (&net.Dialer{
		Timeout:   15 * time.Second,
		KeepAlive: 30 * time.Second,
	}).DialContext,
	TLSHandshakeTimeout: 10 * time.Second,
	TLSClientConfig:     &tls.Config{},
	DisableCompression:  true,
	MaxIdleConns:        100,
	IdleConnTimeout:     90 * time.Second,
}

// httpClient is used for regular (non-upgrade) requests.
// Timeout is 0 so streaming bodies are not truncated; dial / TLS
// timeouts are enforced by the transport above.
var httpClient = &http.Client{
	Transport: streamTransport,
	CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) >= 10 {
			return fmt.Errorf("too many redirects")
		}
		return nil
	},
}

// FetchUpstream sends an HTTP GET to targetURL, forwarding only safe
// headers and rewriting Host, Origin, and Referer to match the
// upstream target.  It supports streaming responses and WebSocket
// upgrade requests.
func FetchUpstream(targetURL string, headers http.Header) (*http.Response, error) {
	return fetchInternal(targetURL, headers, "")
}

// FetchUpstreamWithCookies is like FetchUpstream but additionally
// attaches the provided cookie header from the session store.
func FetchUpstreamWithCookies(targetURL string, headers http.Header, cookieHeader string) (*http.Response, error) {
	return fetchInternal(targetURL, headers, cookieHeader)
}

func fetchInternal(targetURL string, headers http.Header, cookieHeader string) (*http.Response, error) {
	parsed, err := url.Parse(targetURL)
	if err != nil {
		return nil, fmt.Errorf("parsing target URL: %w", err)
	}

	req, err := http.NewRequest(http.MethodGet, targetURL, nil)
	if err != nil {
		return nil, fmt.Errorf("building request: %w", err)
	}

	// ---- safe headers ----
	forwardHeaders(req.Header, headers)

	// ---- rewrite Host / Origin / Referer to upstream ----
	req.Host = parsed.Host
	req.Header.Set("Host", parsed.Host)

	upstreamOrigin := parsed.Scheme + "://" + parsed.Host
	if headers.Get("Origin") != "" {
		req.Header.Set("Origin", upstreamOrigin)
	}
	if headers.Get("Referer") != "" {
		req.Header.Set("Referer", targetURL)
	}

	// ---- session cookies ----
	injectCookies(req, cookieHeader)

	// ---- WebSocket upgrade ----
	if isWebSocketUpgrade(headers) {
		req.Header.Set("Connection", "Upgrade")
		req.Header.Set("Upgrade", "websocket")

		for _, k := range []string{
			"Sec-WebSocket-Key",
			"Sec-WebSocket-Version",
			"Sec-WebSocket-Extensions",
			"Sec-WebSocket-Protocol",
		} {
			if v := headers.Get(k); v != "" {
				req.Header.Set(k, v)
			}
		}

		// RoundTrip preserves the 101 Switching Protocols response and
		// keeps the underlying connection open for bidirectional I/O.
		return streamTransport.RoundTrip(req)
	}

	// ---- regular streaming fetch ----
	return httpClient.Do(req)
}

// injectCookies merges per-origin cookies from the session store into
// the outbound request.
func injectCookies(req *http.Request, cookieHeader string) {
	if cookieHeader == "" {
		return
	}
	existing := req.Header.Get("Cookie")
	if existing != "" {
		req.Header.Set("Cookie", existing+"; "+cookieHeader)
	} else {
		req.Header.Set("Cookie", cookieHeader)
	}
}

// isWebSocketUpgrade returns true when the headers carry a WS upgrade.
func isWebSocketUpgrade(h http.Header) bool {
	return strings.EqualFold(h.Get("Upgrade"), "websocket") &&
		strings.Contains(strings.ToLower(h.Get("Connection")), "upgrade")
}
