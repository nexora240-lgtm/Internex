// internex_rewriter::js
//
// Lightweight JavaScript rewriter (regex-based). This avoids heavy AST
// dependencies to keep builds working on Windows without toolchain issues.
// It rewrites common URL-bearing call sites and constructors. It is NOT
// a full JS parser; the client runtime still provides full interception.

use crate::url::encode_url_with_base;

pub fn rewrite_js(proxy_origin: &str, base_url: &str, js: &str) -> String {
    if js.is_empty() {
        return js.to_string();
    }

    // Replace common constructors: new Worker("url"), new WebSocket("url"), etc.
    let mut out = js.to_string();
    for ctor in ["Worker", "SharedWorker", "WebSocket", "EventSource", "URL"] {
        out = rewrite_call_first_arg(proxy_origin, base_url, &out, &format!("new {}", ctor));
    }

    // Replace common functions: fetch("url"), importScripts("url"), sendBeacon("url")
    for func in ["fetch", "importScripts", "sendBeacon"] {
        out = rewrite_call_first_arg(proxy_origin, base_url, &out, func);
    }

    // XHR.open("GET", "url")
    out = rewrite_open_second_arg(proxy_origin, base_url, &out);

    out
}

fn rewrite_call_first_arg(proxy_origin: &str, base_url: &str, src: &str, callee: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let needle = format!("{}(", callee);
    let mut i = 0;
    while let Some(pos) = src[i..].find(&needle) {
        let start = i + pos;
        out.push_str(&src[i..start + needle.len()]);
        let mut j = start + needle.len();
        // Skip whitespace
        while j < src.len() && src.as_bytes()[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= src.len() {
            break;
        }
        let quote = src.as_bytes()[j];
        if quote == b'\'' || quote == b'"' {
            j += 1;
            let end = src[j..].find(quote as char).map(|k| j + k);
            if let Some(end_idx) = end {
                let raw = &src[j..end_idx];
                let rewritten = encode_url_with_base(proxy_origin, base_url, raw)
                    .unwrap_or_else(|| raw.to_string());
                out.push_str(&rewritten);
                out.push(quote as char);
                i = end_idx + 1;
                continue;
            }
        }
        i = start + needle.len();
    }
    out.push_str(&src[i..]);
    out
}

fn rewrite_open_second_arg(proxy_origin: &str, base_url: &str, src: &str) -> String {
    // Matches: .open("GET", "url") or open('GET', 'url')
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while let Some(pos) = src[i..].find("open(") {
        let start = i + pos;
        out.push_str(&src[i..start + 5]);
        let mut j = start + 5;
        // Skip to first comma
        while j < src.len() && src.as_bytes()[j] != b',' {
            j += 1;
        }
        if j >= src.len() {
            break;
        }
        out.push_str(&src[start + 5..j + 1]);
        j += 1;
        while j < src.len() && src.as_bytes()[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= src.len() {
            break;
        }
        let quote = src.as_bytes()[j];
        if quote == b'\'' || quote == b'"' {
            j += 1;
            let end = src[j..].find(quote as char).map(|k| j + k);
            if let Some(end_idx) = end {
                let raw = &src[j..end_idx];
                let rewritten = encode_url_with_base(proxy_origin, base_url, raw)
                    .unwrap_or_else(|| raw.to_string());
                out.push_str(&rewritten);
                out.push(quote as char);
                i = end_idx + 1;
                continue;
            }
        }
        i = j;
    }
    out.push_str(&src[i..]);
    out
}
