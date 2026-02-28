// internex_rewriter::csp
//
// Content-Security-Policy rewriter.
//
// Rewrites CSP directives so that proxied resources are allowed through the
// policy the origin server set.  The proxy origin is injected into every
// source-list directive, nonces/hashes are preserved, and directives that
// would break mixed-content proxying are stripped.

use crate::url::encode_url;

/// All source-list directives that can contain URLs we need to extend.
const SOURCE_LIST_DIRECTIVES: &[&str] = &[
    "default-src",
    "script-src",
    "style-src",
    "img-src",
    "connect-src",
    "frame-src",
    "worker-src",
    "child-src",
    "manifest-src",
    "media-src",
    "font-src",
    "object-src",
    "base-uri",
    "form-action",
];

/// Directives that are removed outright because they interfere with proxying.
const STRIP_DIRECTIVES: &[&str] = &[
    "upgrade-insecure-requests",
    "block-all-mixed-content",
];

/// Rewrite a full Content-Security-Policy header value.
///
/// * `proxy_origin` – our proxy's origin, e.g. `"http://localhost:8080"`.
/// * `upstream_origin` – the origin of the page being proxied; used to
///   re-encode any absolute URLs that appear in directive values.
/// * `csp` – the raw CSP header value from upstream.
pub fn rewrite_csp(proxy_origin: &str, upstream_origin: &str, csp: &str) -> String {
    let mut out_directives: Vec<String> = Vec::new();

    for directive in csp.split(';') {
        let directive = directive.trim();
        if directive.is_empty() {
            continue;
        }

        let mut parts: Vec<&str> = directive.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        let name = parts[0].to_ascii_lowercase();

        // Strip directives that break proxying.
        if STRIP_DIRECTIVES.contains(&name.as_str()) {
            continue;
        }

        if SOURCE_LIST_DIRECTIVES.contains(&name.as_str()) {
            // Rewrite the source list.
            let values = &parts[1..];
            let rewritten = rewrite_source_list(proxy_origin, upstream_origin, values);
            out_directives.push(format!("{} {}", name, rewritten));
        } else {
            // report-uri, report-to, sandbox, etc. – pass through unchanged.
            out_directives.push(parts.join(" "));
        }
    }

    out_directives.join("; ")
}

/// Rewrite a single source-list (the values after the directive name).
///
/// Strategy:
/// 1. Keep keyword sources ('self', 'unsafe-inline', 'unsafe-eval', etc.)
/// 2. Keep nonces and hashes ('nonce-...', 'sha256-...')
/// 3. Rewrite absolute URL sources through the proxy
/// 4. Append the proxy origin so our own scripts/resources are allowed
fn rewrite_source_list(
    proxy_origin: &str,
    upstream_origin: &str,
    values: &[&str],
) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut has_proxy_origin = false;

    for &val in values {
        if val == "*" || val == "'none'" {
            out.push(val.to_string());
            continue;
        }

        // Keywords: 'self', 'unsafe-inline', 'unsafe-eval', 'wasm-unsafe-eval',
        // 'strict-dynamic', 'report-sample'
        if val.starts_with('\'') && val.ends_with('\'') {
            // Nonces and hashes – rewrite nonce value if desired, but the
            // simplest safe approach is to keep them so pages that rely on
            // nonce-based CSP keep working.
            out.push(rewrite_keyword_or_hash(proxy_origin, val));
            continue;
        }

        // Scheme sources: data:, blob:, https:, etc.
        if val.ends_with(':') && !val.contains('/') {
            out.push(val.to_string());
            continue;
        }

        // Assume anything else is a host-source or URL.
        // Try to proxy-encode it so the browser accepts our proxy URLs.
        if let Some(encoded) = encode_url(proxy_origin, val) {
            out.push(encoded);
        } else {
            out.push(val.to_string());
        }

        // Also keep the original value so that if we missed something the
        // page's own resources still load.
        out.push(val.to_string());
    }

    // Always allow the proxy's own origin.
    let proxy_host = proxy_origin
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if !has_proxy_origin {
        out.push(proxy_origin.to_string());
    }

    // Also keep upstream origin so inline references resolve.
    if !out.iter().any(|v| v == upstream_origin) {
        out.push(upstream_origin.to_string());
    }

    out.join(" ")
}

/// Rewrite a CSP keyword token (quoted).
///
/// Nonces:  'nonce-abc123'  → kept as-is (the proxy injects the same nonce
///          into rewritten script tags).
/// Hashes:  'sha256-...'    → kept as-is; will match because the rewriter
///          preserves inline script content when possible.
fn rewrite_keyword_or_hash(_proxy_origin: &str, token: &str) -> String {
    // For now, pass through all keywords unchanged.  A future iteration
    // can re-compute hashes after rewriting inline scripts.
    token.to_string()
}

/// Convenience: rewrite the nonce value itself (e.g. for script injection).
pub fn extract_nonce(csp: &str) -> Option<String> {
    for directive in csp.split(';') {
        let directive = directive.trim();
        for part in directive.split_whitespace() {
            if part.starts_with("'nonce-") && part.ends_with('\'') {
                let nonce = &part[7..part.len() - 1];
                return Some(nonce.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY: &str = "http://localhost:8080";
    const UPSTREAM: &str = "https://example.com";

    #[test]
    fn strips_upgrade_insecure() {
        let csp = "default-src 'self'; upgrade-insecure-requests; script-src 'none'";
        let result = rewrite_csp(PROXY, UPSTREAM, csp);
        assert!(!result.contains("upgrade-insecure-requests"));
        assert!(result.contains("default-src"));
        assert!(result.contains("script-src"));
    }

    #[test]
    fn strips_block_all_mixed() {
        let csp = "block-all-mixed-content; default-src *";
        let result = rewrite_csp(PROXY, UPSTREAM, csp);
        assert!(!result.contains("block-all-mixed-content"));
    }

    #[test]
    fn adds_proxy_origin() {
        let csp = "script-src 'self' https://cdn.example.com";
        let result = rewrite_csp(PROXY, UPSTREAM, csp);
        assert!(result.contains(PROXY));
    }

    #[test]
    fn preserves_nonces() {
        let csp = "script-src 'nonce-abc123' 'self'";
        let result = rewrite_csp(PROXY, UPSTREAM, csp);
        assert!(result.contains("'nonce-abc123'"));
    }

    #[test]
    fn extract_nonce_works() {
        let csp = "script-src 'nonce-r4nd0m' 'self'; style-src *";
        assert_eq!(extract_nonce(csp), Some("r4nd0m".to_string()));
    }
}
