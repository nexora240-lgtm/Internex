// internex_rewriter::url
//
// URL encoding / decoding for the proxy.  Every absolute URL that flows
// through the rewriter is converted into a proxy-safe form so the browser
// always talks through our server.
//
// Encoding scheme:  /proxy?url=<percent-encoded original>
//
// Supported inputs:
//   absolute        https://example.com/path
//   protocol-rel    //example.com/path
//   relative        /path  or  ../path
//   blob:           blob:https://...
//   data:           data:text/html,...
//   javascript:     javascript:...   (left as-is)
//   file:           file:///...      (BLOCKED)
//
// The proxy_origin is the origin of OUR proxy server, e.g.
// "http://localhost:8080".

use percent_encoding::{utf8_percent_encode, percent_decode_str, AsciiSet, CONTROLS};
use url::Url;

/// Characters that must be percent-encoded inside the `url=` query value.
const QUERY_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'&')
    .add(b'=')
    .add(b'+')
    .add(b'%');

/// Encode an arbitrary URL so it routes through our proxy.
///
/// Returns `None` for `file:` URLs (blocked) and for inputs that cannot be
/// meaningfully proxied (empty strings, bare fragments, etc.).
pub fn encode_url(proxy_origin: &str, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    // Block file: scheme outright.
    if trimmed.to_ascii_lowercase().starts_with("file:") {
        return None;
    }

    // javascript: URLs are not proxied – pass-through.
    if trimmed.to_ascii_lowercase().starts_with("javascript:") {
        return Some(trimmed.to_string());
    }

    // data: URLs are self-contained – pass-through.
    if trimmed.to_ascii_lowercase().starts_with("data:") {
        return Some(trimmed.to_string());
    }

    // blob: URLs – encode the inner URL portion.
    if trimmed.to_ascii_lowercase().starts_with("blob:") {
        let inner = &trimmed[5..];
        if let Some(encoded_inner) = encode_url(proxy_origin, inner) {
            return Some(format!("blob:{}", encoded_inner));
        }
        return Some(trimmed.to_string());
    }

    // Protocol-relative: //example.com/path  → https://example.com/path
    let absolute = if trimmed.starts_with("//") {
        format!("https:{}", trimmed)
    } else if trimmed.starts_with('/') || !trimmed.contains("://") {
        // Relative path – we cannot resolve it without a base, so we return
        // it as-is.  The caller (html / css rewriter) is responsible for
        // resolving against the page's base URL before calling encode_url.
        if Url::parse(trimmed).is_err() {
            // Truly relative – cannot proxy without a base URL.
            return Some(trimmed.to_string());
        }
        trimmed.to_string()
    } else {
        trimmed.to_string()
    };

    // Validate.
    if Url::parse(&absolute).is_err() {
        return Some(trimmed.to_string());
    }

    let encoded_target = utf8_percent_encode(&absolute, QUERY_ENCODE_SET).to_string();
    Some(format!("{}/proxy?url={}", proxy_origin.trim_end_matches('/'), encoded_target))
}

/// Encode a URL resolved against a known base.
pub fn encode_url_with_base(proxy_origin: &str, base: &str, raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    // Resolve relative URLs against the base.
    let resolved = match Url::parse(base) {
        Ok(base_url) => match base_url.join(trimmed) {
            Ok(full) => full.to_string(),
            Err(_) => return Some(trimmed.to_string()),
        },
        Err(_) => return encode_url(proxy_origin, trimmed),
    };

    encode_url(proxy_origin, &resolved)
}

/// Decode a proxied URL back to the original upstream URL.
/// Input is the `url` query-parameter value (already extracted).
pub fn decode_url(encoded: &str) -> Option<String> {
    let decoded = percent_decode_str(encoded).decode_utf8().ok()?;
    let decoded = decoded.as_ref();
    // Validate that it looks like a real URL.
    if Url::parse(decoded).is_ok() {
        Some(decoded.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "http://localhost:8080";

    #[test]
    fn absolute_url() {
        let result = encode_url(ORIGIN, "https://example.com/page").unwrap();
        assert!(result.starts_with("http://localhost:8080/proxy?url="));
        assert!(result.contains("example.com"));
    }

    #[test]
    fn protocol_relative() {
        let result = encode_url(ORIGIN, "//cdn.example.com/lib.js").unwrap();
        assert!(result.contains("proxy?url="));
    }

    #[test]
    fn data_url_passthrough() {
        let result = encode_url(ORIGIN, "data:text/html,<h1>hi</h1>").unwrap();
        assert!(result.starts_with("data:"));
    }

    #[test]
    fn javascript_passthrough() {
        let result = encode_url(ORIGIN, "javascript:void(0)").unwrap();
        assert_eq!(result, "javascript:void(0)");
    }

    #[test]
    fn file_blocked() {
        assert!(encode_url(ORIGIN, "file:///etc/passwd").is_none());
    }

    #[test]
    fn decode_roundtrip() {
        let encoded = encode_url(ORIGIN, "https://example.com/path?q=1").unwrap();
        let query = encoded.split("url=").nth(1).unwrap();
        let decoded = decode_url(query).unwrap();
        assert_eq!(decoded, "https://example.com/path?q=1");
    }

    #[test]
    fn empty_and_fragment_ignored() {
        assert!(encode_url(ORIGIN, "").is_none());
        assert!(encode_url(ORIGIN, "#top").is_none());
    }
}
