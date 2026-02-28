// internex_rewriter::html
//
// Full HTML rewriter.  Walks the DOM produced by html5ever / kuchikiki and
// rewrites every URL-bearing attribute, inline style, inline event handler,
// meta refresh, SVG link, <template> content, and DOM-manipulation sink so
// that all traffic flows through the proxy.

use kuchikiki::traits::*;
use kuchikiki::{parse_html, NodeRef, NodeData};
use html5ever::serialize::{serialize, SerializeOpts};
use markup5ever::{ns, namespace_url};
use serde_json;

use crate::url::encode_url_with_base;
use crate::css::rewrite_css_string;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Rewrite a full HTML document so every URL routes through the proxy.
///
/// * `proxy_origin` – e.g. `"http://localhost:8080"`
/// * `base_url`     – the original page URL (for resolving relative paths)
/// * `html`         – raw HTML source
pub fn rewrite_html(proxy_origin: &str, base_url: &str, html: &str) -> String {
    let doc = parse_html().one(html);

    // Determine <base href> if present – it overrides the page URL for
    // relative resolution.
    let effective_base = find_base_href(&doc).unwrap_or_else(|| base_url.to_string());

    walk(&doc, proxy_origin, &effective_base);
    inject_client_script(&doc, proxy_origin, &effective_base);

    let mut buf = Vec::new();
    serialize(
        &mut buf,
        &doc,
        SerializeOpts {
            scripting_enabled: true,
            traversal_scope: html5ever::serialize::TraversalScope::IncludeNode,
            create_missing_parent: false,
        },
    )
    .expect("serialization failed");

    String::from_utf8(buf).unwrap_or_else(|_| html.to_string())
}

// ---------------------------------------------------------------------------
// DOM walker
// ---------------------------------------------------------------------------

fn walk(node: &NodeRef, proxy: &str, base: &str) {
    if let NodeData::Element(ref el) = *node.data() {
        let tag = el.name.local.to_string().to_ascii_lowercase();
        let mut attrs = el.attributes.borrow_mut();

        // ---- URL attributes ----
        rewrite_url_attrs(&tag, &mut attrs, proxy, base);

        // ---- srcset / imagesrcset ----
        rewrite_srcset_attr(&mut attrs, "srcset", proxy, base);
        rewrite_srcset_attr(&mut attrs, "imagesrcset", proxy, base);

        // ---- <meta http-equiv="refresh"> ----
        if tag == "meta" {
            rewrite_meta_refresh(&mut attrs, proxy, base);
        }

        // ---- Inline styles ----
        if let Some(style) = attrs.get("style").map(|s| s.to_string()) {
            let rewritten = rewrite_css_string(proxy, base, &style);
            attrs.set("style", rewritten);
        }

        // ---- Inline event handlers ----
        rewrite_event_handlers(&mut attrs, proxy, base);

        // ---- SVG attributes ----
        rewrite_svg_attrs(&tag, &mut attrs, proxy, base);

        // ---- <style> element: rewrite the text content ----
        drop(attrs); // release borrow
        if tag == "style" {
            rewrite_inline_style_element(node, proxy, base);
        }

        // ---- <script>: wrap dangerous sinks ----
        if tag == "script" {
            rewrite_inline_script(node, proxy, base);
        }
    }

    // Recurse into children (handles <template> content automatically
    // because kuchikiki exposes template contents as children).
    for child in node.children() {
        walk(&child, proxy, base);
    }
}

// ---------------------------------------------------------------------------
// URL-bearing attributes
// ---------------------------------------------------------------------------

/// Standard element attributes that contain a single URL.
const URL_ATTRS: &[&str] = &[
    "href", "src", "action", "formaction", "poster", "data", "manifest",
    "background", "ping", "cite", "longdesc", "usemap", "archive",
    "codebase", "classid",
];

fn rewrite_url_attrs(
    tag: &str,
    attrs: &mut kuchikiki::Attributes,
    proxy: &str,
    base: &str,
) {
    for &attr in URL_ATTRS {
        if let Some(val) = attrs.get(attr).map(|s| s.to_string()) {
            if let Some(encoded) = encode_url_with_base(proxy, base, &val) {
                attrs.set(attr, encoded);
            }
        }
    }

    // Special: <link rel="stylesheet" href="…"> is already covered by href
    // above, but <link rel="icon"> etc. also use href – all handled.

    // <object> and <embed> also may have "type" – no rewriting needed there.
}

// ---------------------------------------------------------------------------
// srcset / imagesrcset
// ---------------------------------------------------------------------------

fn rewrite_srcset_attr(
    attrs: &mut kuchikiki::Attributes,
    attr: &str,
    proxy: &str,
    base: &str,
) {
    if let Some(val) = attrs.get(attr).map(|s| s.to_string()) {
        let rewritten = rewrite_srcset(proxy, base, &val);
        attrs.set(attr, rewritten);
    }
}

/// Parse and rewrite a `srcset` value.  Format:
///   url1 1x, url2 2x, url3 300w
fn rewrite_srcset(proxy: &str, base: &str, srcset: &str) -> String {
    srcset
        .split(',')
        .map(|entry| {
            let parts: Vec<&str> = entry.trim().splitn(2, char::is_whitespace).collect();
            match parts.as_slice() {
                [url, descriptor] => {
                    let encoded = encode_url_with_base(proxy, base, url)
                        .unwrap_or_else(|| url.to_string());
                    format!("{} {}", encoded, descriptor)
                }
                [url] => encode_url_with_base(proxy, base, url)
                    .unwrap_or_else(|| url.to_string()),
                _ => entry.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// <meta http-equiv="refresh" content="0;url=…">
// ---------------------------------------------------------------------------

fn rewrite_meta_refresh(
    attrs: &mut kuchikiki::Attributes,
    proxy: &str,
    base: &str,
) {
    let is_refresh = attrs
        .get("http-equiv")
        .map(|v| v.eq_ignore_ascii_case("refresh"))
        .unwrap_or(false);

    if !is_refresh {
        return;
    }

    if let Some(content) = attrs.get("content").map(|s| s.to_string()) {
        if let Some(idx) = content.to_ascii_lowercase().find("url=") {
            let (prefix, url_part) = content.split_at(idx + 4);
            if let Some(encoded) = encode_url_with_base(proxy, base, url_part.trim()) {
                attrs.set("content", format!("{}{}", prefix, encoded));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inline event handlers  (onclick, onerror, onload, …)
// ---------------------------------------------------------------------------

/// All HTML event handler attribute names.
const EVENT_ATTRS: &[&str] = &[
    "onabort", "onblur", "oncanplay", "oncanplaythrough", "onchange",
    "onclick", "oncontextmenu", "ondblclick", "ondrag", "ondragend",
    "ondragenter", "ondragleave", "ondragover", "ondragstart", "ondrop",
    "ondurationchange", "onemptied", "onended", "onerror", "onfocus",
    "oninput", "oninvalid", "onkeydown", "onkeypress", "onkeyup", "onload",
    "onloadeddata", "onloadedmetadata", "onloadstart", "onmousedown",
    "onmouseenter", "onmouseleave", "onmousemove", "onmouseout",
    "onmouseover", "onmouseup", "onpause", "onplay", "onplaying",
    "onprogress", "onratechange", "onreset", "onresize", "onscroll",
    "onseeked", "onseeking", "onselect", "onshow", "onstalled", "onsubmit",
    "onsuspend", "ontimeupdate", "ontoggle", "onvolumechange", "onwaiting",
    "onmessage", "onmessageerror", "onbeforeunload", "onhashchange",
    "onpopstate",
];

fn rewrite_event_handlers(
    attrs: &mut kuchikiki::Attributes,
    proxy: &str,
    _base: &str,
) {
    for &attr in EVENT_ATTRS {
        if let Some(val) = attrs.get(attr).map(|s| s.to_string()) {
            // Wrap the handler body so that runtime URL references go
            // through our client-side hook.  A lightweight approach:
            // prefix with a scope setter.
            let wrapped = format!(
                "__internex.scope(this,function(){{ {} }}).call(this,event)",
                val,
            );
            attrs.set(attr, wrapped);
        }
    }
}

// ---------------------------------------------------------------------------
// SVG-specific attributes
// ---------------------------------------------------------------------------

const SVG_URL_ATTRS: &[&str] = &[
    "xlink:href", "href", "clip-path", "mask", "filter",
    "fill", "stroke", "marker-start", "marker-mid", "marker-end",
];

fn rewrite_svg_attrs(
    tag: &str,
    attrs: &mut kuchikiki::Attributes,
    proxy: &str,
    base: &str,
) {
    // Only process known SVG elements or if xlink:href is present.
    let svg_tags = [
        "svg", "use", "image", "a", "pattern", "mask", "clippath",
        "filter", "fegaussianblur", "feimage", "lineargradient",
        "radialgradient", "marker", "symbol", "defs",
    ];
    if !svg_tags.contains(&tag) {
        return;
    }

    for &attr in SVG_URL_ATTRS {
        if let Some(val) = attrs.get(attr).map(|s| s.to_string()) {
            // url(#local) references should be left alone.
            if val.starts_with("url(#") || val.starts_with('#') {
                continue;
            }
            // Strip url(...) wrapper if present.
            let inner = if val.starts_with("url(") && val.ends_with(')') {
                let raw = &val[4..val.len() - 1];
                raw.trim().trim_matches(|c| c == '\'' || c == '"')
            } else {
                val.as_str()
            };
            if inner.starts_with('#') {
                continue;
            }
            if let Some(encoded) = encode_url_with_base(proxy, base, inner) {
                if val.starts_with("url(") {
                    attrs.set(attr, format!("url({})", encoded));
                } else {
                    attrs.set(attr, encoded);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// <style> element body
// ---------------------------------------------------------------------------

fn rewrite_inline_style_element(node: &NodeRef, proxy: &str, base: &str) {
    let mut text_content = String::new();
    for child in node.children() {
        if let NodeData::Text(ref t) = *child.data() {
            text_content.push_str(&t.borrow());
        }
    }
    if text_content.is_empty() {
        return;
    }
    let rewritten = rewrite_css_string(proxy, base, &text_content);
    // Replace all text children with the rewritten content.
    for child in node.children() {
        child.detach();
    }
    node.append(NodeRef::new_text(&rewritten));
}

// ---------------------------------------------------------------------------
// <script> inline: wrap dangerous sinks
// ---------------------------------------------------------------------------

fn rewrite_inline_script(node: &NodeRef, proxy: &str, _base: &str) {
    let mut text_content = String::new();
    for child in node.children() {
        if let NodeData::Text(ref t) = *child.data() {
            text_content.push_str(&t.borrow());
        }
    }
    if text_content.is_empty() {
        return;
    }

    // Wrap the script body in our runtime scope so that dynamic DOM
    // manipulation APIs (innerHTML, document.write, etc.) are intercepted
    // by the client-side runtime hooks.
    //
    // The actual JS AST rewriting for `eval`, `Function()`, `setTimeout`,
    // `innerHTML`, etc. happens in js.rs when the server rewrites
    // standalone JS resources.  For inline scripts we inject a scope
    // wrapper and rely on the client runtime.
    let wrapped = format!(
        "(function(__internex_proxy){{\n{}\n}})(window.__internex);",
        text_content,
    );

    for child in node.children() {
        child.detach();
    }
    node.append(NodeRef::new_text(&wrapped));
}

// ---------------------------------------------------------------------------
// <base href> detection
// ---------------------------------------------------------------------------

fn find_base_href(doc: &NodeRef) -> Option<String> {
    for node in doc.inclusive_descendants() {
        if let NodeData::Element(ref el) = *node.data() {
            if el.name.local.to_string() == "base" {
                let attrs = el.attributes.borrow();
                return attrs.get("href").map(|s| s.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Client-side runtime injection
// ---------------------------------------------------------------------------

/// Inject a tiny <script> at the top of <head> that sets up the runtime
/// hooks the rewritten inline scripts and event handlers depend on.
fn inject_client_script(doc: &NodeRef, proxy_origin: &str, base_url: &str) {
    let script_src = format!("{}/internex.runtime.js", proxy_origin);
    let base_json = serde_json::to_string(base_url).unwrap_or_else(|_| "\"\"".to_string());
    let script_html = format!(
        r#"<script>window.__internex_base = {};</script><script src="{}"></script>"#,
        base_json,
        script_src,
    );

    // Find <head> (or create one).
    for node in doc.inclusive_descendants() {
        if let NodeData::Element(ref el) = *node.data() {
            if el.name.local.to_string() == "head" {
                let frag = parse_html().one(script_html.clone());
                // Insert as first child of <head>.
                if let Some(first) = node.children().next() {
                    first.insert_before(frag);
                } else {
                    node.append(frag);
                }
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Trait impls to make kuchikiki::Attributes easier to work with
// ---------------------------------------------------------------------------

trait AttrsExt {
    fn get(&self, name: &str) -> Option<&str>;
    fn set(&mut self, name: &str, value: String);
}

impl AttrsExt for kuchikiki::Attributes {
    fn get(&self, name: &str) -> Option<&str> {
        self.map.get(&kuchikiki::ExpandedName::new(ns!(), markup5ever::LocalName::from(name)))
            .map(|a| a.value.as_str())
    }

    fn set(&mut self, name: &str, value: String) {
        let key = kuchikiki::ExpandedName::new(ns!(), markup5ever::LocalName::from(name));
        if let Some(attr) = self.map.get_mut(&key) {
            attr.value = value.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY: &str = "http://localhost:8080";
    const BASE: &str = "https://example.com/page";

    #[test]
    fn rewrites_anchor_href() {
        let html = r#"<html><head></head><body><a href="https://example.com/other">link</a></body></html>"#;
        let result = rewrite_html(PROXY, BASE, html);
        assert!(result.contains("/proxy?url="));
    }

    #[test]
    fn rewrites_img_src() {
        let html = r#"<html><head></head><body><img src="https://example.com/img.png"></body></html>"#;
        let result = rewrite_html(PROXY, BASE, html);
        assert!(result.contains("/proxy?url="));
    }

    #[test]
    fn rewrites_meta_refresh() {
        let html = r#"<html><head><meta http-equiv="refresh" content="5;url=https://example.com/new"></head><body></body></html>"#;
        let result = rewrite_html(PROXY, BASE, html);
        assert!(result.contains("/proxy?url="));
    }

    #[test]
    fn injects_runtime_script() {
        let html = "<html><head></head><body></body></html>";
        let result = rewrite_html(PROXY, BASE, html);
        assert!(result.contains("internex.runtime.js"));
    }
}
