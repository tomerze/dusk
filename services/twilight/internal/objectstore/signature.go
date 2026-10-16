package objectstore

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"time"
)

const signingAlgorithm = "AWS4-HMAC-SHA256"

func hashHex(content []byte) string {
	digest := sha256.Sum256(content)
	return hex.EncodeToString(digest[:])
}

func keyedHash(key []byte, content string) []byte {
	mac := hmac.New(sha256.New, key)
	mac.Write([]byte(content))
	return mac.Sum(nil)
}

func encodeComponent(text string, keepSlash bool) string {
	var encoded strings.Builder
	for _, character := range []byte(text) {
		switch {
		case 'A' <= character && character <= 'Z', 'a' <= character && character <= 'z', '0' <= character && character <= '9',
			character == '-', character == '_', character == '.', character == '~', keepSlash && character == '/':
			encoded.WriteByte(character)
		default:
			encoded.WriteString("%" + strings.ToUpper(hex.EncodeToString([]byte{character})))
		}
	}
	return encoded.String()
}

func canonicalQuery(values url.Values) string {
	keys := make([]string, 0, len(values))
	for key := range values {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	var pairs []string
	for _, key := range keys {
		entries := append([]string(nil), values[key]...)
		sort.Strings(entries)
		for _, value := range entries {
			pairs = append(pairs, encodeComponent(key, false)+"="+encodeComponent(value, false))
		}
	}
	return strings.Join(pairs, "&")
}

func sign(request *http.Request, accessKey, secretKey, region, payloadHash string, at time.Time) {
	stamp := at.UTC().Format("20060102T150405Z")
	day := stamp[:8]
	request.Header.Set("X-Amz-Date", stamp)
	request.Header.Set("X-Amz-Content-Sha256", payloadHash)
	headers := map[string]string{"host": request.URL.Host}
	for name, values := range request.Header {
		headers[strings.ToLower(name)] = strings.Join(values, ",")
	}
	names := make([]string, 0, len(headers))
	for name := range headers {
		names = append(names, name)
	}
	sort.Strings(names)
	var canonicalHeaders strings.Builder
	for _, name := range names {
		canonicalHeaders.WriteString(name + ":" + strings.TrimSpace(headers[name]) + "\n")
	}
	signedHeaders := strings.Join(names, ";")
	path := request.URL.EscapedPath()
	if unescaped, failure := url.PathUnescape(path); failure == nil {
		path = encodeComponent(unescaped, true)
	}
	if path == "" {
		path = "/"
	}
	canonical := strings.Join([]string{request.Method, path, canonicalQuery(request.URL.Query()), canonicalHeaders.String(), signedHeaders, payloadHash}, "\n")
	scope := day + "/" + region + "/s3/aws4_request"
	toSign := strings.Join([]string{signingAlgorithm, stamp, scope, hashHex([]byte(canonical))}, "\n")
	key := keyedHash(keyedHash(keyedHash(keyedHash([]byte("AWS4"+secretKey), day), region), "s3"), "aws4_request")
	signature := hex.EncodeToString(keyedHash(key, toSign))
	request.Header.Set("Authorization", signingAlgorithm+" Credential="+accessKey+"/"+scope+", SignedHeaders="+signedHeaders+", Signature="+signature)
}
