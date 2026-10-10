package webui

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"testing/fstest"
)

func serve(test *testing.T, handler http.Handler, method, target string, header http.Header) *httptest.ResponseRecorder {
	test.Helper()
	request := httptest.NewRequest(method, target, nil)
	for name, values := range header {
		request.Header[name] = values
	}
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, request)
	return recorder
}

func TestUnbuiltUI(test *testing.T) {
	handler, failure := Handler(nil, false)
	if failure != nil {
		test.Fatal(failure)
	}
	response := serve(test, handler, http.MethodGet, "/campaigns", nil)
	if response.Code != http.StatusServiceUnavailable || strings.TrimSpace(response.Body.String()) != "UI not built" {
		test.Fatalf("%d %q", response.Code, response.Body.String())
	}
}

func TestBuiltUI(test *testing.T) {
	assets := fstest.MapFS{
		"index.html":           {Data: []byte("<!doctype html><title>twilight</title>")},
		"assets/index-Bx1.js":  {Data: []byte("console.log(1)")},
		"assets/index-Cy2.css": {Data: []byte("body{}")},
		"favicon.svg":          {Data: []byte("<svg xmlns=\"http://www.w3.org/2000/svg\"/>")},
	}
	handler, failure := Handler(assets, true)
	if failure != nil {
		test.Fatal(failure)
	}
	index := serve(test, handler, http.MethodGet, "/", nil)
	if index.Code != http.StatusOK || !strings.Contains(index.Body.String(), "twilight") || index.Header().Get("Cache-Control") != "no-cache" ||
		index.Header().Get("Content-Security-Policy") != ContentSecurityPolicy || !strings.HasPrefix(index.Header().Get("Content-Type"), "text/html") {
		test.Fatalf("index %d %v", index.Code, index.Header())
	}
	route := serve(test, handler, http.MethodGet, "/campaigns/0192f0c4-4c1a-7b8e-9d2f-3a4b5c6d7e8f", nil)
	if route.Code != http.StatusOK || route.Body.String() != index.Body.String() {
		test.Fatalf("a client route is not the app: %d", route.Code)
	}
	script := serve(test, handler, http.MethodGet, "/assets/index-Bx1.js", nil)
	if script.Code != http.StatusOK || script.Header().Get("Cache-Control") != "public, max-age=31536000, immutable" ||
		!strings.HasPrefix(script.Header().Get("Content-Type"), "text/javascript") || script.Header().Get("X-Content-Type-Options") != "nosniff" {
		test.Fatalf("script %d %v", script.Code, script.Header())
	}
	revalidated := serve(test, handler, http.MethodGet, "/assets/index-Bx1.js", http.Header{"If-None-Match": {script.Header().Get("ETag")}})
	if revalidated.Code != http.StatusNotModified {
		test.Fatalf("a matching ETag answered %d", revalidated.Code)
	}
	if missing := serve(test, handler, http.MethodGet, "/assets/missing.js", nil); missing.Code != http.StatusNotFound {
		test.Fatalf("a missing asset answered %d", missing.Code)
	}
	if escaped := serve(test, handler, http.MethodGet, "/../../etc/passwd", nil); escaped.Body.String() != index.Body.String() {
		test.Fatalf("a path outside the assets answered %d %q", escaped.Code, escaped.Body.String())
	}
	if posted := serve(test, handler, http.MethodPost, "/", nil); posted.Code != http.StatusMethodNotAllowed || posted.Header().Get("Allow") != "GET, HEAD" {
		test.Fatalf("POST answered %d", posted.Code)
	}
	if head := serve(test, handler, http.MethodHead, "/favicon.svg", nil); head.Code != http.StatusOK || head.Body.Len() != 0 || head.Header().Get("Cache-Control") != "no-cache" {
		test.Fatalf("HEAD answered %d with %d bytes", head.Code, head.Body.Len())
	}
	if _, failure := Handler(fstest.MapFS{"app.js": {Data: []byte("1")}}, true); failure == nil {
		test.Fatal("assets without index.html were accepted")
	}
}
