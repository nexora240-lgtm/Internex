package transport

import (
	"net/url"
	"strings"
)

// ProxyOrigin is the base URL of *our* proxy server.
// Set once at startup from the PORT env or a config flag.
var ProxyOrigin = "http://localhost:8080"

// EncodeProxyURL encodes a target URL into our proxy form:
//
//	/proxy?url=<percent-encoded target>
//
// Returns the full proxy URL (with ProxyOrigin prepended).
func EncodeProxyURL(targetURL string) string {
	return ProxyOrigin + "/proxy?url=" + url.QueryEscape(targetURL)
}

// EncodeProxyPath returns the path-only version for internal use:
//
//	/proxy?url=<percent-encoded target>
func EncodeProxyPath(targetURL string) string {
	return "/proxy?url=" + url.QueryEscape(targetURL)
}

// DecodeProxyURL extracts the original target URL from the `url`
// query parameter value.  Returns the decoded URL and true on success.
func DecodeProxyURL(encoded string) (string, bool) {
	decoded, err := url.QueryUnescape(encoded)
	if err != nil {
		return "", false
	}
	parsed, err := url.Parse(decoded)
	if err != nil {
		return "", false
	}
	if parsed.Scheme != "http" && parsed.Scheme != "https" {
		return "", false
	}
	return decoded, true
}

// RewriteLocationHeader rewrites an upstream `Location` header value
// so it routes through the proxy.  Relative URLs are resolved against
// the upstream base first.
func RewriteLocationHeader(upstreamBase, location string) string {
	if location == "" {
		return ""
	}

	// Resolve relative redirect against the upstream base URL.
	base, err := url.Parse(upstreamBase)
	if err != nil {
		return EncodeProxyPath(location)
	}
	resolved, err := base.Parse(location)
	if err != nil {
		return EncodeProxyPath(location)
	}

	return EncodeProxyPath(resolved.String())
}

// RewriteSetCookieDomain rewrites the Domain attribute of a Set-Cookie
// header so the cookie is scoped to the proxy's own host rather than
// the upstream origin.
func RewriteSetCookieDomain(setCookie string, proxyHost string) string {
	// Quick approach: remove the existing Domain= so the browser
	// defaults to the proxy's host, and strip Secure when the proxy
	// is plain HTTP.
	out := removeCookieAttr(setCookie, "Domain")
	out = removeCookieAttr(out, "SameSite")
	if strings.HasPrefix(ProxyOrigin, "http://") {
		out = removeCookieAttr(out, "Secure")
	}
	// Append SameSite=None so cross-origin fetch still works inside
	// the proxied page.
	out += "; SameSite=None"
	if strings.HasPrefix(ProxyOrigin, "https://") {
		out += "; Secure"
	}
	return out
}

// removeCookieAttr strips an attribute (and its value) from a
// Set-Cookie header string.
func removeCookieAttr(cookie, attr string) string {
	lower := strings.ToLower(cookie)
	target := strings.ToLower(attr)

	var parts []string
	for _, seg := range strings.Split(cookie, ";") {
		trimmed := strings.TrimSpace(seg)
		trimmedLower := strings.ToLower(trimmed)
		if strings.HasPrefix(trimmedLower, target+"=") || trimmedLower == target {
			continue
		}
		parts = append(parts, trimmed)
	}
	_ = lower // suppress unused
	return strings.Join(parts, "; ")
}
