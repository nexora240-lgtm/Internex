// internex_rewriter::css
//
// CSS rewriter.  Parses CSS with `cssparser` and rewrites every URL reference
// so it routes through the proxy.  Handles:
//
//   url(…)  and  image-set(…)
//   @import url(…)  /  @import "…"
//   @font-face { src: url(…) }
//   @namespace url(…)
//   background, background-image, border-image, mask-image, filter,
//   cursor, clip-path, shape-outside, content, list-style
//   CSSOM sinks: insertRule, replace, replaceSync, cssRules

use cssparser::{
    Parser, ParserInput, Token, CowRcStr,
};

use crate::url::encode_url_with_base;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Rewrite a complete CSS stylesheet.
pub fn rewrite_css(proxy_origin: &str, base_url: &str, css: &str) -> String {
    rewrite_css_string(proxy_origin, base_url, css)
}

/// Rewrite an arbitrary CSS string (stylesheet, inline style, or fragment).
/// This is also called by the HTML rewriter for `style="…"` attributes and
/// `<style>` elements.
pub fn rewrite_css_string(proxy_origin: &str, base_url: &str, css: &str) -> String {
    // We walk through the CSS token stream and rebuild the output, replacing
    // url() and string tokens inside @import / @font-face / property values.
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut out = String::with_capacity(css.len());

    rewrite_token_stream(&mut parser, proxy_origin, base_url, &mut out);

    out
}

// ---------------------------------------------------------------------------
// Token-level rewriter
// ---------------------------------------------------------------------------

fn rewrite_token_stream(
    parser: &mut Parser<'_, '_>,
    proxy: &str,
    base: &str,
    out: &mut String,
) {
    // Track whether we are inside an @import or @font-face context so we
    // know that bare string tokens should be treated as URLs.
    let mut in_import = false;

    loop {
        let token = match parser.next_including_whitespace_and_comments() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };

        match token {
            // ---- url(…) ----
            Token::UnquotedUrl(ref url_val) => {
                let url_str: &str = url_val.as_ref();
                let rewritten = encode_url_with_base(proxy, base, url_str)
                    .unwrap_or_else(|| url_str.to_string());
                out.push_str(&format!("url({})", quote_css_url(&rewritten)));
            }

            Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                out.push_str("url(");
                // The next token(s) inside url() are the actual URL.
                rewrite_function_args(parser, proxy, base, out, true);
                out.push(')');
            }

            Token::Function(ref name) if name.eq_ignore_ascii_case("image-set") => {
                out.push_str("image-set(");
                rewrite_function_args(parser, proxy, base, out, true);
                out.push(')');
            }

            // ---- @import ----
            Token::AtKeyword(ref kw) if kw.eq_ignore_ascii_case("import") => {
                out.push_str("@import ");
                in_import = true;
            }

            // ---- @namespace ----
            Token::AtKeyword(ref kw) if kw.eq_ignore_ascii_case("namespace") => {
                out.push_str("@namespace ");
                // The url token will be handled by the url() branch above.
            }

            // ---- @font-face ----
            Token::AtKeyword(ref kw) if kw.eq_ignore_ascii_case("font-face") => {
                out.push_str("@font-face");
                // The block will be handled token-by-token; url() inside
                // src: is caught by the url() branch.
            }

            // ---- Other at-keywords ----
            Token::AtKeyword(ref kw) => {
                out.push('@');
                out.push_str(kw.as_ref());
            }

            // ---- Quoted strings (may be URLs in @import context) ----
            Token::QuotedString(ref s) => {
                let s_str: &str = s.as_ref();
                if in_import {
                    let rewritten = encode_url_with_base(proxy, base, s_str)
                        .unwrap_or_else(|| s_str.to_string());
                    out.push_str(&format!("\"{}\"", escape_css_string(&rewritten)));
                    in_import = false;
                } else {
                    out.push_str(&format!("\"{}\"", escape_css_string(s_str)));
                }
            }

            // ---- Blocks ----
            Token::CurlyBracketBlock => {
                out.push('{');
                let _ = parser.parse_nested_block(|inner| -> Result<(), ()> {
                    rewrite_token_stream(inner, proxy, base, out);
                    Ok(())
                });
                out.push('}');
            }

            Token::ParenthesisBlock => {
                out.push('(');
                let _ = parser.parse_nested_block(|inner| -> Result<(), ()> {
                    rewrite_token_stream(inner, proxy, base, out);
                    Ok(())
                });
                out.push(')');
            }

            Token::SquareBracketBlock => {
                out.push('[');
                let _ = parser.parse_nested_block(|inner| -> Result<(), ()> {
                    rewrite_token_stream(inner, proxy, base, out);
                    Ok(())
                });
                out.push(']');
            }

            // ---- Functions we don't specially handle (but recurse into) ----
            Token::Function(ref name) => {
                out.push_str(name.as_ref());
                out.push('(');
                let _ = parser.parse_nested_block(|inner| -> Result<(), ()> {
                    rewrite_token_stream(inner, proxy, base, out);
                    Ok(())
                });
                out.push(')');
            }

            // ---- Everything else: serialize back ----
            Token::Ident(ref v) => out.push_str(v.as_ref()),
            Token::Hash(ref v) | Token::IDHash(ref v) => {
                out.push('#');
                out.push_str(v.as_ref());
            }
            Token::Number { value, .. } => out.push_str(&format_number(value)),
            Token::Percentage { unit_value, .. } => {
                out.push_str(&format_number(unit_value * 100.0));
                out.push('%');
            }
            Token::Dimension { value, ref unit, .. } => {
                out.push_str(&format_number(value));
                out.push_str(unit.as_ref());
            }
            Token::WhiteSpace(ref _s) => out.push(' '),
            Token::Colon => out.push(':'),
            Token::Semicolon => {
                in_import = false;
                out.push(';');
            }
            Token::Comma => out.push(','),
            Token::Delim(c) => out.push(c),
            Token::IncludeMatch => out.push_str("~="),
            Token::DashMatch => out.push_str("|="),
            Token::PrefixMatch => out.push_str("^="),
            Token::SuffixMatch => out.push_str("$="),
            Token::SubstringMatch => out.push_str("*="),
            Token::CDO => out.push_str("<!--"),
            Token::CDC => out.push_str("-->"),
            Token::Comment(ref c) => {
                out.push_str("/*");
                out.push_str(c.as_ref());
                out.push_str("*/");
            }
            Token::BadString(ref s) => {
                out.push_str(s.as_ref());
            }
            Token::BadUrl(ref s) => {
                out.push_str("url(");
                out.push_str(s.as_ref());
                out.push(')');
            }
            Token::CloseParenthesis => out.push(')'),
            Token::CloseSquareBracket => out.push(']'),
            Token::CloseCurlyBracket => out.push('}'),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// url() / image-set() argument rewriter
// ---------------------------------------------------------------------------

fn rewrite_function_args(
    parser: &mut Parser<'_, '_>,
    proxy: &str,
    base: &str,
    out: &mut String,
    is_url_context: bool,
) {
    let _ = parser.parse_nested_block(|inner| -> Result<(), ()> {
        loop {
            let tok = match inner.next_including_whitespace_and_comments() {
                Ok(t) => t.clone(),
                Err(_) => break,
            };
            match tok {
                Token::QuotedString(ref s) if is_url_context => {
                    let s_str: &str = s.as_ref();
                    let rewritten = encode_url_with_base(proxy, base, s_str)
                        .unwrap_or_else(|| s_str.to_string());
                    out.push_str(&format!("\"{}\"", escape_css_string(&rewritten)));
                }
                Token::UnquotedUrl(ref s) => {
                    let s_str: &str = s.as_ref();
                    let rewritten = encode_url_with_base(proxy, base, s_str)
                        .unwrap_or_else(|| s_str.to_string());
                    out.push_str(&quote_css_url(&rewritten));
                }
                Token::Function(ref name) if name.eq_ignore_ascii_case("url") => {
                    out.push_str("url(");
                    rewrite_function_args(inner, proxy, base, out, true);
                    out.push(')');
                }
                Token::WhiteSpace(_) => out.push(' '),
                Token::Comma => out.push(','),
                Token::Number { value, .. } => out.push_str(&format_number(value)),
                Token::Dimension { value, ref unit, .. } => {
                    out.push_str(&format_number(value));
                    out.push_str(unit.as_ref());
                }
                Token::Ident(ref v) => out.push_str(v.as_ref()),
                Token::Delim(c) => out.push(c),
                _ => {}
            }
        }
        Ok(())
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn quote_css_url(url: &str) -> String {
    // Always double-quote for safety.
    format!("\"{}\"", escape_css_string(url))
}

fn escape_css_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\a ")
}

fn format_number(v: f32) -> String {
    if v == (v as i64) as f32 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

// ---------------------------------------------------------------------------
// CSSOM sink wrappers (used by the JS rewriter)
// ---------------------------------------------------------------------------

/// Rewrite a CSS rule string as would be passed to `CSSStyleSheet.insertRule()`.
pub fn rewrite_insert_rule(proxy_origin: &str, base_url: &str, rule: &str) -> String {
    rewrite_css_string(proxy_origin, base_url, rule)
}

/// Rewrite a full stylesheet string as would be passed to
/// `CSSStyleSheet.replace()` / `replaceSync()`.
pub fn rewrite_replace_sync(proxy_origin: &str, base_url: &str, css: &str) -> String {
    rewrite_css_string(proxy_origin, base_url, css)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY: &str = "http://localhost:8080";
    const BASE: &str = "https://example.com/style/";

    #[test]
    fn rewrites_url_function() {
        let css = r#"body { background: url(https://example.com/bg.png); }"#;
        let result = rewrite_css(PROXY, BASE, css);
        assert!(result.contains("/proxy?url="));
    }

    #[test]
    fn rewrites_import() {
        let css = r#"@import "https://example.com/reset.css";"#;
        let result = rewrite_css(PROXY, BASE, css);
        assert!(result.contains("/proxy?url="));
    }

    #[test]
    fn preserves_data_urls() {
        let css = r#"body { background: url(data:image/png;base64,abc); }"#;
        let result = rewrite_css(PROXY, BASE, css);
        assert!(result.contains("data:image/png;base64,abc"));
    }
}
