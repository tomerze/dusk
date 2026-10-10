# Vendored OpenAPI 3.1 schemas

`openapi_test.go` validates twilight's OpenAPI document against the OpenAPI
Initiative's published JSON Schemas for OpenAPI 3.1, read from here so the test
needs no network.

| File | Published at | SHA-256 of this copy |
|------|--------------|----------------------|
| `oas-3.1-schema.json` | <https://spec.openapis.org/oas/3.1/schema/2022-10-07> | `affe20a11f2c448cd19da7efb88a42b3758367c53a9b07d75d88dcaf04f45abe` |
| `oas-3.1-schema-base.json` | <https://spec.openapis.org/oas/3.1/schema-base/2022-10-07> | `3d572d5696f604cb91dbc2ea21aa9292bd08cafa69d6a31c1ae4bfee8dd01ece` |
| `oas-3.1-dialect-base.json` | <https://spec.openapis.org/oas/3.1/dialect/base> | `3bb8a27b34af76661a01873ae130c6870cb2ac0fd71fa863db47d3ea44353ce3` |
| `oas-3.1-meta-base.json` | <https://spec.openapis.org/oas/3.1/meta/base> | `1101ff73113d6dd468581c813226c9d894aa74a754bda2d6feef8b61003620c8` |

They are the published documents of the 2022-10-07 release, reformatted, and
`oas-3.1-schema.json` without the published `$comment` keywords, which link
each definition to its section of the specification and play no part in
validation. Compared as JSON, the other three equal the published documents.

The schemas are the OpenAPI Initiative's, under the Apache License 2.0;
`LICENSE` is that licence's text.

## Updating

```bash
cd services/twilight/internal/api/testdata
curl -fsSL -o oas-3.1-schema.json https://spec.openapis.org/oas/3.1/schema/2022-10-07
curl -fsSL -o oas-3.1-schema-base.json https://spec.openapis.org/oas/3.1/schema-base/2022-10-07
curl -fsSL -o oas-3.1-dialect-base.json https://spec.openapis.org/oas/3.1/dialect/base
curl -fsSL -o oas-3.1-meta-base.json https://spec.openapis.org/oas/3.1/meta/base
```

Then remove the `$comment` keywords, update the table above and run
`go test ./internal/api/ -run TestTheDocumentIsAValidOpenAPI31Document`.
