package api

import (
	"bytes"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"

	"github.com/santhosh-tekuri/jsonschema/v6"
	"golang.org/x/text/language"
	"golang.org/x/text/message"
)

//go:embed openapi.json
var openAPIDocument []byte

const documentLocation = "https://twilight.invalid/api/openapi.json"

func OpenAPIDocument() []byte {
	return bytes.Clone(openAPIDocument)
}

func compiler() (*jsonschema.Compiler, error) {
	document, failure := jsonschema.UnmarshalJSON(bytes.NewReader(openAPIDocument))
	if failure != nil {
		return nil, fmt.Errorf("the OpenAPI document is not JSON: %w", failure)
	}
	compiler := jsonschema.NewCompiler()
	compiler.DefaultDraft(jsonschema.Draft2020)
	if failure := compiler.AddResource(documentLocation, document); failure != nil {
		return nil, failure
	}
	return compiler, nil
}

func pointer(segments ...string) string {
	escaped := make([]string, len(segments))
	for index, segment := range segments {
		segment = strings.ReplaceAll(strings.ReplaceAll(segment, "~", "~0"), "/", "~1")
		escaped[index] = strings.NewReplacer("{", "%7B", "}", "%7D").Replace(segment)
	}
	return documentLocation + "#/" + strings.Join(escaped, "/")
}

func requestSchemas(routes []route) (map[string]*jsonschema.Schema, error) {
	var document struct {
		Paths map[string]map[string]struct {
			RequestBody *struct {
				Content map[string]json.RawMessage `json:"content"`
			} `json:"requestBody"`
		} `json:"paths"`
	}
	if failure := json.Unmarshal(openAPIDocument, &document); failure != nil {
		return nil, failure
	}
	compiled, failure := compiler()
	if failure != nil {
		return nil, failure
	}
	schemas := map[string]*jsonschema.Schema{}
	for _, current := range routes {
		operation, found := document.Paths[current.Path][strings.ToLower(current.Method)]
		if !found {
			return nil, fmt.Errorf("the OpenAPI document has no %s", current.pattern())
		}
		if operation.RequestBody == nil {
			continue
		}
		schema, failure := compiled.Compile(pointer("paths", current.Path, strings.ToLower(current.Method), "requestBody", "content", "application/json", "schema"))
		if failure != nil {
			return nil, fmt.Errorf("the request schema of %s: %w", current.pattern(), failure)
		}
		schemas[current.pattern()] = schema
	}
	return schemas, nil
}

var englishPrinter = message.NewPrinter(language.English)

func violations(failure error) []map[string]string {
	var validation *jsonschema.ValidationError
	if !errors.As(failure, &validation) {
		return []map[string]string{{"at": "/", "message": failure.Error()}}
	}
	var found []map[string]string
	var visit func(current *jsonschema.ValidationError)
	visit = func(current *jsonschema.ValidationError) {
		if len(current.Causes) == 0 {
			found = append(found, map[string]string{"at": "/" + strings.Join(current.InstanceLocation, "/"), "message": current.ErrorKind.LocalizedString(englishPrinter)})
		}
		for _, cause := range current.Causes {
			visit(cause)
		}
	}
	visit(validation)
	return found
}

func (server *Server) openAPI(call *exchange) error {
	call.writer.Header().Set("Content-Type", "application/json")
	call.writer.Header().Set("Cache-Control", "public, max-age=300")
	call.writer.WriteHeader(http.StatusOK)
	_, _ = call.writer.Write(openAPIDocument)
	return nil
}
