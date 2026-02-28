package rewriter

/*
#cgo LDFLAGS: -L${SRCDIR}/../../../internex_rewriter/target/release -linternex_rewriter
#cgo CFLAGS: -I${SRCDIR}/../../../internex_rewriter

#include <stdlib.h>

extern char* rewrite_html(const char* input);
extern char* rewrite_css(const char* input);
extern char* rewrite_js(const char* input);
extern void  free_string(char* ptr);
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"io"
	"strings"
	"unsafe"
)

// ContentKind identifies what type of content to rewrite.
type ContentKind int

const (
	HTML ContentKind = iota
	CSS
	JS
)

// rewriteInput is the JSON envelope sent to the Rust FFI functions.
type rewriteInput struct {
	ProxyOrigin string `json:"proxy_origin"`
	BaseURL     string `json:"base_url"`
	Content     string `json:"content"`
}

// RewriteHTML rewrites an HTML document through the Rust rewriter.
func RewriteHTML(proxyOrigin, baseURL, content string) string {
	return callRewrite("html", proxyOrigin, baseURL, content)
}

// RewriteCSS rewrites a CSS stylesheet through the Rust rewriter.
func RewriteCSS(proxyOrigin, baseURL, content string) string {
	return callRewrite("css", proxyOrigin, baseURL, content)
}

// RewriteJS rewrites JavaScript source through the Rust rewriter.
func RewriteJS(proxyOrigin, baseURL, content string) string {
	return callRewrite("js", proxyOrigin, baseURL, content)
}

// callRewrite marshals the input into JSON, calls the given Rust FFI function,
// converts the result back to a Go string, and frees the Rust-allocated memory.
func callRewrite(kind string, proxyOrigin, baseURL, content string) string {
	payload, err := json.Marshal(rewriteInput{
		ProxyOrigin: proxyOrigin,
		BaseURL:     baseURL,
		Content:     content,
	})
	if err != nil {
		return content
	}

	cInput := C.CString(string(payload))
	defer C.free(unsafe.Pointer(cInput))

	var cResult *C.char
	switch kind {
	case "html":
		cResult = C.rewrite_html(cInput)
	case "css":
		cResult = C.rewrite_css(cInput)
	case "js":
		cResult = C.rewrite_js(cInput)
	default:
		return content
	}
	if cResult == nil {
		return content
	}
	defer C.free_string(cResult)

	return C.GoString(cResult)
}

// Rewrite reads source content, transforms it according to kind, and returns
// a reader over the rewritten bytes.  This calls into the Rust shared library
// through CGo.
func Rewrite(kind ContentKind, src io.Reader) (io.Reader, error) {
	body, err := io.ReadAll(src)
	if err != nil {
		return nil, fmt.Errorf("rewriter: reading source: %w", err)
	}

	content := string(body)

	// TODO: plumb proxy_origin and base_url from the request context.
	proxyOrigin := "http://localhost:8080"
	baseURL := ""

	var result string
	switch kind {
	case HTML:
		result = callRewrite("html", proxyOrigin, baseURL, content)
	case CSS:
		result = callRewrite("css", proxyOrigin, baseURL, content)
	case JS:
		result = callRewrite("js", proxyOrigin, baseURL, content)
	default:
		result = content
	}

	return strings.NewReader(result), nil
}
