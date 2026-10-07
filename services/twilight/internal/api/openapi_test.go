package api

import (
	"bytes"
	"encoding/json"
	"net/http"
	"os"
	"sort"
	"strings"
	"testing"

	"github.com/santhosh-tekuri/jsonschema/v6"
)

func TestTheDocumentIsAValidOpenAPI31Document(test *testing.T) {
	compiler := jsonschema.NewCompiler()
	for name, location := range map[string]string{
		"oas-3.1-schema.json":       "https://spec.openapis.org/oas/3.1/schema/2022-10-07",
		"oas-3.1-schema-base.json":  "https://spec.openapis.org/oas/3.1/schema-base/2022-10-07",
		"oas-3.1-dialect-base.json": "https://spec.openapis.org/oas/3.1/dialect/base",
		"oas-3.1-meta-base.json":    "https://spec.openapis.org/oas/3.1/meta/base",
	} {
		content, failure := os.ReadFile("testdata/" + name)
		if failure != nil {
			test.Fatal(failure)
		}
		document, failure := jsonschema.UnmarshalJSON(bytes.NewReader(content))
		if failure != nil {
			test.Fatal(failure)
		}
		if failure := compiler.AddResource(location, document); failure != nil {
			test.Fatal(failure)
		}
	}
	schema, failure := compiler.Compile("https://spec.openapis.org/oas/3.1/schema-base/2022-10-07")
	if failure != nil {
		test.Fatal(failure)
	}
	instance, failure := jsonschema.UnmarshalJSON(bytes.NewReader(OpenAPIDocument()))
	if failure != nil {
		test.Fatal(failure)
	}
	if failure := schema.Validate(instance); failure != nil {
		test.Fatalf("the document is not valid OpenAPI 3.1: %v", failure)
	}
	for _, broken := range []string{
		strings.Replace(string(openAPIDocument), `"openapi": "3.1.1"`, `"openapi": "3.0.3"`, 1),
		strings.Replace(string(openAPIDocument), `"type": "integer"`, `"type": "whole number"`, 1),
		strings.Replace(string(openAPIDocument), `"in": "path"`, `"in": "body"`, 1),
	} {
		instance, failure := jsonschema.UnmarshalJSON(strings.NewReader(broken))
		if failure != nil {
			test.Fatal(failure)
		}
		if schema.Validate(instance) == nil {
			test.Error("a broken document passed")
		}
	}
}

func TestEveryRouteIsDocumentedAndEveryOperationIsRouted(test *testing.T) {
	var document struct {
		Paths map[string]map[string]struct {
			OperationID  string `json:"operationId"`
			RequiredRole string `json:"x-required-role"`
			Security     *[]any `json:"security"`
		} `json:"paths"`
		Components struct {
			Schemas map[string]json.RawMessage `json:"schemas"`
		} `json:"components"`
	}
	if failure := json.Unmarshal(openAPIDocument, &document); failure != nil {
		test.Fatal(failure)
	}
	routed := map[string]route{}
	for _, current := range routeTable() {
		routed[strings.ToLower(current.Method)+" "+current.Path] = current
	}
	documented := map[string]bool{}
	identifiers := map[string]bool{}
	for path, operations := range document.Paths {
		for method, operation := range operations {
			key := method + " " + path
			documented[key] = true
			current, found := routed[key]
			if !found {
				test.Errorf("%s is documented but not routed", key)
				continue
			}
			if operation.RequiredRole != current.Role.String() {
				test.Errorf("%s documents x-required-role %q but requires %s", key, operation.RequiredRole, current.Role)
			}
			public := operation.Security != nil && len(*operation.Security) == 0
			if public != (current.Role == RolePublic) {
				test.Errorf("%s: documented as public %v, routed as %s", key, public, current.Role)
			}
			if operation.OperationID == "" || identifiers[operation.OperationID] {
				test.Errorf("%s has a missing or repeated operationId %q", key, operation.OperationID)
			}
			identifiers[operation.OperationID] = true
		}
	}
	var missing []string
	for key := range routed {
		if !documented[key] {
			missing = append(missing, key)
		}
	}
	sort.Strings(missing)
	if len(missing) > 0 {
		test.Errorf("routed but not documented: %s", strings.Join(missing, ", "))
	}
	if len(routed) != 34 {
		test.Errorf("%d routes; spec 7.5, the device lifecycle and the auth routes make 34", len(routed))
	}
	references := map[string]bool{}
	var collect func(value any)
	collect = func(value any) {
		switch typed := value.(type) {
		case map[string]any:
			if reference, found := typed["$ref"].(string); found {
				references[reference] = true
			}
			for _, item := range typed {
				collect(item)
			}
		case []any:
			for _, item := range typed {
				collect(item)
			}
		}
	}
	var whole any
	_ = json.Unmarshal(openAPIDocument, &whole)
	collect(whole)
	for name := range document.Components.Schemas {
		if !references["#/components/schemas/"+name] {
			test.Errorf("the schema %s is never referenced", name)
		}
	}
}

func TestTheDocumentIsServed(test *testing.T) {
	current := newHarness(test, nil)
	served := current.call(http.MethodGet, "/api/openapi.json", nil, "")
	if served.Code != http.StatusOK || !bytes.Equal(served.Body.Bytes(), openAPIDocument) || served.Header().Get("Content-Type") != "application/json" {
		test.Fatalf("%d %s", served.Code, served.Header())
	}
	if field(served, "openapi") != "3.1.1" {
		test.Fatalf("openapi %v", field(served, "openapi"))
	}
}

func TestEveryRequestSchemaCompiles(test *testing.T) {
	schemas, failure := requestSchemas(routeTable())
	if failure != nil {
		test.Fatal(failure)
	}
	for _, name := range []string{"POST /api/v1/campaigns", "PUT /api/v1/campaigns/{id}", "POST /api/v1/campaigns/{id}/resume", "POST /api/v1/nodes/{device}/{installation}/lifecycle"} {
		if schemas[name] == nil {
			test.Errorf("%s has no request schema", name)
		}
	}
	if schemas["GET /api/v1/me"] != nil {
		test.Error("GET /api/v1/me has a request schema")
	}
}
