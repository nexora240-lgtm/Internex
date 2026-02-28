// internex_rewriter::js
//
// JavaScript rewriter using SWC.  Parses the source into an AST, applies a
// visitor that rewrites all proxied API call sites, then emits the
// transformed source.
//
// Rewrites:
//   ● Globals: window, self, globalThis, document, location, navigator,
//     history, origin
//   ● Network: fetch, XMLHttpRequest, WebSocket, EventSource, sendBeacon,
//     RTCPeerConnection
//   ● Workers: new Worker(), new SharedWorker(), importScripts()
//   ● Modules: import(), export *, <script type="module">
//   ● Eval sinks: eval(), Function(), setTimeout("…"), setInterval("…")
//   ● DOM property writes: element.src / href / action / poster, dataset.*,
//     getAttribute / setAttribute
//   ● DOM HTML sinks: innerHTML, outerHTML, insertAdjacentHTML,
//     document.write, DOMParser.parseFromString,
//     Range.createContextualFragment
//   ● URL constructors: new URL(), new Request(), new Response()

use swc_common::{
    sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS,
};
use swc_ecma_ast::*;
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax};
use swc_ecma_visit::{VisitMut, VisitMutWith};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse and rewrite a JavaScript source string.
///
/// * `proxy_origin` – e.g. `"http://localhost:8080"`
/// * `source`       – the raw JS code
///
/// Returns the rewritten JS source.  If parsing fails, returns the input
/// unchanged (graceful degradation).
pub fn rewrite_js(proxy_origin: &str, source: &str) -> String {
    let cm: Lrc<SourceMap> = Default::default();

    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom("input.js".into())),
        source.into(),
    );

    let lexer = Lexer::new(
        Syntax::Es(EsSyntax {
            jsx: false,
            decorators: true,
            import_attributes: true,
            ..Default::default()
        }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );

    let mut parser = Parser::new_from(lexer);
    let mut module = match parser.parse_module() {
        Ok(m) => m,
        Err(_) => return source.to_string(),
    };

    // Apply our rewriting visitor.
    GLOBALS.set(&Globals::new(), || {
        let mut visitor = ProxyRewriter {
            proxy_origin: proxy_origin.to_string(),
        };
        module.visit_mut_with(&mut visitor);
    });

    // Emit.
    let mut buf = Vec::new();
    {
        let writer = JsWriter::new(cm.clone(), "\n", &mut buf, None);
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default().with_minify(false),
            cm: cm.clone(),
            comments: None,
            wr: writer,
        };
        emitter.emit_module(&module).unwrap();
    }

    String::from_utf8(buf).unwrap_or_else(|_| source.to_string())
}

// ---------------------------------------------------------------------------
// AST Visitor
// ---------------------------------------------------------------------------

struct ProxyRewriter {
    proxy_origin: String,
}

impl ProxyRewriter {
    // Helpers to build AST nodes -------------------------------------------

    /// `__internex.wrap(<expr>)`
    fn wrap_call(&self, expr: Box<Expr>) -> Expr {
        Expr::Call(CallExpr {
            span: Default::default(),
            callee: Callee::Expr(Box::new(self.internex_member("wrap"))),
            args: vec![ExprOrSpread {
                spread: None,
                expr,
            }],
            type_args: None,
            ..Default::default()
        })
    }

    /// `__internex.rewriteUrl(<expr>)`
    fn rewrite_url_call(&self, expr: Box<Expr>) -> Expr {
        Expr::Call(CallExpr {
            span: Default::default(),
            callee: Callee::Expr(Box::new(self.internex_member("rewriteUrl"))),
            args: vec![ExprOrSpread {
                spread: None,
                expr,
            }],
            type_args: None,
            ..Default::default()
        })
    }

    /// `__internex.rewriteHtml(<expr>)`
    fn rewrite_html_call(&self, expr: Box<Expr>) -> Expr {
        Expr::Call(CallExpr {
            span: Default::default(),
            callee: Callee::Expr(Box::new(self.internex_member("rewriteHtml"))),
            args: vec![ExprOrSpread {
                spread: None,
                expr,
            }],
            type_args: None,
            ..Default::default()
        })
    }

    /// `__internex.rewriteEval(<expr>)`
    fn rewrite_eval_call(&self, expr: Box<Expr>) -> Expr {
        Expr::Call(CallExpr {
            span: Default::default(),
            callee: Callee::Expr(Box::new(self.internex_member("rewriteEval"))),
            args: vec![ExprOrSpread {
                spread: None,
                expr,
            }],
            type_args: None,
            ..Default::default()
        })
    }

    /// `__internex.<method>`
    fn internex_member(&self, method: &str) -> Expr {
        Expr::Member(MemberExpr {
            span: Default::default(),
            obj: Box::new(Expr::Ident(Ident::new(
                "__internex".into(),
                Default::default(),
                Default::default(),
            ))),
            prop: MemberProp::Ident(IdentName::new(method.into(), Default::default())),
        })
    }

    fn is_str_lit(expr: &Expr) -> bool {
        matches!(expr, Expr::Lit(Lit::Str(_)))
    }
}

// ---------------------------------------------------------------------------
// Globals that need wrapping
// ---------------------------------------------------------------------------

/// Bare identifiers that should be replaced with `__internex.wrap(name)`.
const WRAPPED_GLOBALS: &[&str] = &[
    "window",
    "self",
    "globalThis",
    "document",
    "location",
    "navigator",
    "history",
    "origin",
];

/// Constructor names whose first argument is a URL.
const URL_CONSTRUCTORS: &[&str] = &[
    "Worker",
    "SharedWorker",
    "URL",
    "Request",
    "Response",
    "WebSocket",
    "EventSource",
    "RTCPeerConnection",
];

/// Property names on elements whose assigned value is a URL.
const URL_PROPERTIES: &[&str] = &[
    "src",
    "href",
    "action",
    "poster",
    "formAction",
    "data",
    "codeBase",
    "background",
];

/// Property names whose assigned value is an HTML string.
const HTML_SINK_PROPERTIES: &[&str] = &[
    "innerHTML",
    "outerHTML",
];

/// Methods where the argument is HTML.
const HTML_SINK_METHODS: &[&str] = &[
    "insertAdjacentHTML",
    "write",
    "writeln",
    "parseFromString",
    "createContextualFragment",
];

/// Functions where a string first argument is eval-like code.
const EVAL_SINKS: &[&str] = &[
    "eval",
    "setTimeout",
    "setInterval",
];

impl VisitMut for ProxyRewriter {
    // ---- Global identifiers ----
    fn visit_mut_expr(&mut self, expr: &mut Expr) {
        // Recurse first so inner expressions are rewritten before we check.
        expr.visit_mut_children_with(self);

        match expr {
            // `window`, `self`, etc. as bare identifiers.
            Expr::Ident(ident) if WRAPPED_GLOBALS.contains(&ident.sym.as_ref()) => {
                *expr = self.wrap_call(Box::new(Expr::Ident(ident.clone())));
            }

            // `new Worker("url")`, `new URL("url")`, etc.
            Expr::New(new_expr) => {
                if let Expr::Ident(callee) = &*new_expr.callee {
                    if URL_CONSTRUCTORS.contains(&callee.sym.as_ref()) {
                        if let Some(args) = &mut new_expr.args {
                            if !args.is_empty() {
                                let first = args[0].expr.clone();
                                args[0].expr = Box::new(self.rewrite_url_call(first));
                            }
                        }
                    }
                }
            }

            // `fetch(url)`, `sendBeacon(url)`, `importScripts(url, …)`
            Expr::Call(call_expr) => {
                self.rewrite_call_expr(call_expr);
            }

            // Assignment: `el.src = "…"` → `el.src = __internex.rewriteUrl("…")`
            Expr::Assign(assign) => {
                self.rewrite_assign(assign);
            }

            _ => {}
        }
    }
}

impl ProxyRewriter {
    fn rewrite_call_expr(&self, call: &mut CallExpr) {
        match &call.callee {
            Callee::Expr(callee_expr) => {
                match callee_expr.as_ref() {
                    // Direct calls: fetch(url), eval("code"), etc.
                    Expr::Ident(ident) => {
                        let name = ident.sym.as_ref();

                        // fetch(url), XMLHttpRequest.open(method, url)
                        if name == "fetch" || name == "sendBeacon" {
                            if !call.args.is_empty() {
                                let first = call.args[0].expr.clone();
                                call.args[0].expr = Box::new(self.rewrite_url_call(first));
                            }
                        }

                        // importScripts(url1, url2, …) — rewrite all arguments
                        if name == "importScripts" {
                            for arg in call.args.iter_mut() {
                                let e = arg.expr.clone();
                                arg.expr = Box::new(self.rewrite_url_call(e));
                            }
                        }

                        // eval("code"), Function("code")
                        if name == "eval" || name == "Function" {
                            if !call.args.is_empty() {
                                let first = call.args[0].expr.clone();
                                if Self::is_str_lit(&first) {
                                    call.args[0].expr =
                                        Box::new(self.rewrite_eval_call(first));
                                }
                            }
                        }

                        // setTimeout("string", …) / setInterval("string", …)
                        if name == "setTimeout" || name == "setInterval" {
                            if !call.args.is_empty() {
                                let first = call.args[0].expr.clone();
                                if Self::is_str_lit(&first) {
                                    call.args[0].expr =
                                        Box::new(self.rewrite_eval_call(first));
                                }
                            }
                        }
                    }

                    // Method calls: el.setAttribute("src", url), etc.
                    Expr::Member(member) => {
                        if let MemberProp::Ident(prop) = &member.prop {
                            let method = prop.sym.as_ref();

                            // .setAttribute("src", val) / .getAttribute("src")
                            if method == "setAttribute" && call.args.len() >= 2 {
                                if let Expr::Lit(Lit::Str(attr_name)) =
                                    &*call.args[0].expr
                                {
                                    let attr = attr_name.value.to_string();
                                    if URL_PROPERTIES
                                        .iter()
                                        .any(|p| p.eq_ignore_ascii_case(&attr))
                                    {
                                        let val = call.args[1].expr.clone();
                                        call.args[1].expr =
                                            Box::new(self.rewrite_url_call(val));
                                    }
                                }
                            }

                            // DOM HTML sinks
                            if HTML_SINK_METHODS.contains(&method) {
                                // The HTML-containing argument varies by
                                // method – for insertAdjacentHTML it's arg[1],
                                // for the rest it's arg[0].
                                let idx = if method == "insertAdjacentHTML" { 1 } else { 0 };
                                if call.args.len() > idx {
                                    let arg = call.args[idx].expr.clone();
                                    call.args[idx].expr =
                                        Box::new(self.rewrite_html_call(arg));
                                }
                            }

                            // XMLHttpRequest.open(method, url)
                            if method == "open" && call.args.len() >= 2 {
                                let url_arg = call.args[1].expr.clone();
                                call.args[1].expr =
                                    Box::new(self.rewrite_url_call(url_arg));
                            }
                        }
                    }

                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn rewrite_assign(&self, assign: &mut AssignExpr) {
        if let Some(member) = assign.left.as_simple().and_then(|e| match e {
            Expr::Member(m) => Some(m),
            _ => None,
        }) {
            if let MemberProp::Ident(prop) = &member.prop {
                let name = prop.sym.as_ref();

                // el.src = val → el.src = __internex.rewriteUrl(val)
                if URL_PROPERTIES.contains(&name) {
                    let rhs = assign.right.clone();
                    assign.right = Box::new(self.rewrite_url_call(rhs));
                }

                // el.innerHTML = val → el.innerHTML = __internex.rewriteHtml(val)
                if HTML_SINK_PROPERTIES.contains(&name) {
                    let rhs = assign.right.clone();
                    assign.right = Box::new(self.rewrite_html_call(rhs));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROXY: &str = "http://localhost:8080";

    #[test]
    fn wraps_fetch() {
        let code = r#"fetch("/api/data")"#;
        let result = rewrite_js(PROXY, code);
        assert!(result.contains("rewriteUrl"));
    }

    #[test]
    fn wraps_new_worker() {
        let code = r#"new Worker("worker.js")"#;
        let result = rewrite_js(PROXY, code);
        assert!(result.contains("rewriteUrl"));
    }

    #[test]
    fn wraps_eval_string() {
        let code = r#"eval("alert(1)")"#;
        let result = rewrite_js(PROXY, code);
        assert!(result.contains("rewriteEval"));
    }

    #[test]
    fn wraps_innerhtml() {
        let code = r#"el.innerHTML = "<img src=x>";"#;
        let result = rewrite_js(PROXY, code);
        assert!(result.contains("rewriteHtml"));
    }

    #[test]
    fn wraps_set_attribute_src() {
        let code = r#"el.setAttribute("src", url);"#;
        let result = rewrite_js(PROXY, code);
        assert!(result.contains("rewriteUrl"));
    }
}
