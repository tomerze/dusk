package webui

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
	"io/fs"
	"mime"
	"net/http"
	"path"
	"strings"
	"time"
)

const ContentSecurityPolicy = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; " +
	"connect-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'"

type asset struct {
	content     []byte
	contentType string
	entityTag   string
}

type handler struct {
	assets map[string]asset
}

func Handler(assets fs.FS, available bool) (http.Handler, error) {
	if !available {
		return http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) {
			writer.Header().Set("Cache-Control", "no-store")
			http.Error(writer, "UI not built", http.StatusServiceUnavailable)
		}), nil
	}
	loaded := map[string]asset{}
	failure := fs.WalkDir(assets, ".", func(name string, entry fs.DirEntry, failure error) error {
		if failure != nil || entry.IsDir() {
			return failure
		}
		content, failure := fs.ReadFile(assets, name)
		if failure != nil {
			return failure
		}
		contentType := mime.TypeByExtension(path.Ext(name))
		if contentType == "" {
			contentType = http.DetectContentType(content)
		}
		sum := sha256.Sum256(content)
		loaded["/"+name] = asset{content: content, contentType: contentType, entityTag: `"` + base64.RawURLEncoding.EncodeToString(sum[:18]) + `"`}
		return nil
	})
	if failure != nil {
		return nil, fmt.Errorf("read the UI assets: %w", failure)
	}
	if _, found := loaded["/index.html"]; !found {
		return nil, errors.New("the UI assets hold no index.html")
	}
	return &handler{assets: loaded}, nil
}

func (served *handler) ServeHTTP(writer http.ResponseWriter, request *http.Request) {
	if request.Method != http.MethodGet && request.Method != http.MethodHead {
		writer.Header().Set("Allow", "GET, HEAD")
		http.Error(writer, "method not allowed", http.StatusMethodNotAllowed)
		return
	}
	name := path.Clean("/" + request.URL.Path)
	found, exists := served.assets[name]
	header := writer.Header()
	switch {
	case exists && strings.HasPrefix(name, "/assets/"):
		header.Set("Cache-Control", "public, max-age=31536000, immutable")
	case exists && name != "/index.html":
		header.Set("Cache-Control", "no-cache")
	case strings.Contains(path.Base(name), "."):
		http.NotFound(writer, request)
		return
	default:
		found = served.assets["/index.html"]
		header.Set("Cache-Control", "no-cache")
	}
	if strings.HasPrefix(found.contentType, "text/html") {
		header.Set("Content-Security-Policy", ContentSecurityPolicy)
		header.Set("Cache-Control", "no-cache")
	}
	header.Set("Content-Type", found.contentType)
	header.Set("ETag", found.entityTag)
	header.Set("X-Content-Type-Options", "nosniff")
	header.Set("Referrer-Policy", "same-origin")
	header.Set("X-Frame-Options", "DENY")
	http.ServeContent(writer, request, name, time.Time{}, bytes.NewReader(found.content))
}
