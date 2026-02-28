// internex_rewriter
//
// C ABI boundary for the Internex rewriter.  This crate is compiled as a
// `cdylib` so the Go server can call into it via CGo / FFI.
//
// Exposed functions:
//   rewrite_html(input: *const c_char) -> *mut c_char
//   rewrite_css(input: *const c_char) -> *mut c_char
//   rewrite_js(input: *const c_char) -> *mut c_char
//
// Input is a JSON-encoded object:
//   { "proxy_origin": "…", "base_url": "…", "content": "…" }
//
// Return value is a NUL-terminated C string allocated with CString.
// The caller MUST free it by calling `free_string`.

pub mod url;
pub mod csp;
pub mod html;
pub mod css;
pub mod js;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse the JSON envelope and return (proxy_origin, base_url, content).
fn parse_input(json: &str) -> Option<(String, String, String)> {
    let v: Value = serde_json::from_str(json).ok()?;
    let proxy_origin = v.get("proxy_origin")?.as_str()?.to_string();
    let base_url = v.get("base_url")?.as_str()?.to_string();
    let content = v.get("content")?.as_str()?.to_string();
    Some((proxy_origin, base_url, content))
}

/// Convert a Rust String into a heap-allocated C string.
fn to_c_string(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Read a `*const c_char` into a `&str`.  Returns `None` on null or invalid
/// UTF-8.
unsafe fn read_c_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

// ---------------------------------------------------------------------------
// C ABI exports
// ---------------------------------------------------------------------------

/// Rewrite an HTML document.
///
/// Input: JSON `{ "proxy_origin": "…", "base_url": "…", "content": "…" }`
/// Returns: rewritten HTML as a NUL-terminated C string, or null on error.
#[no_mangle]
pub unsafe extern "C" fn rewrite_html(input: *const c_char) -> *mut c_char {
    let json = match read_c_str(input) {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let (proxy_origin, base_url, content) = match parse_input(json) {
        Some(t) => t,
        None => return ptr::null_mut(),
    };

    let result = html::rewrite_html(&proxy_origin, &base_url, &content);
    to_c_string(result)
}

/// Rewrite a CSS stylesheet / fragment.
///
/// Input: JSON `{ "proxy_origin": "…", "base_url": "…", "content": "…" }`
/// Returns: rewritten CSS as a NUL-terminated C string, or null on error.
#[no_mangle]
pub unsafe extern "C" fn rewrite_css(input: *const c_char) -> *mut c_char {
    let json = match read_c_str(input) {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let (proxy_origin, base_url, content) = match parse_input(json) {
        Some(t) => t,
        None => return ptr::null_mut(),
    };

    let result = css::rewrite_css(&proxy_origin, &base_url, &content);
    to_c_string(result)
}

/// Rewrite a JavaScript source file.
///
/// Input: JSON `{ "proxy_origin": "…", "base_url": "…", "content": "…" }`
/// Returns: rewritten JS as a NUL-terminated C string, or null on error.
#[no_mangle]
pub unsafe extern "C" fn rewrite_js(input: *const c_char) -> *mut c_char {
    let json = match read_c_str(input) {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let (proxy_origin, _base_url, content) = match parse_input(json) {
        Some(t) => t,
        None => return ptr::null_mut(),
    };

    let result = js::rewrite_js(&proxy_origin, &content);
    to_c_string(result)
}

/// Free a C string previously returned by one of the rewrite_* functions.
///
/// The Go side MUST call this to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}
