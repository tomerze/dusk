# Vendored Swagger UI

The gateway serves these files itself at `/v1/static/`, so `/v1/docs` renders on a
host with no route to the internet. They are the reason that page is not a CDN
link. Do not edit them; replace them wholesale as below.

## What is here

From [swagger-ui-dist](https://www.npmjs.com/package/swagger-ui-dist) **5.32.14**:

| File | Bytes | SHA-256 |
|------|-------|---------|
| `swagger-ui-bundle.js` | 1,553,809 | `16d93d5cc19e54c98fb0b81157dbb3bd90780aa36b914e128a643b31e54a93f4` |
| `swagger-ui.css` | 185,784 | `d7f39f764aa18c7b47dd05b9af5613e373e4ac0f3557c2693d52d0abc2464d76` |
| `LICENSE` | 11,358 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |

`swagger-ui-standalone-preset.js` is absent. The page renders `BaseLayout`, which
lives in the bundle; the preset supplies `StandaloneLayout`'s topbar and URL
explorer, which this page does not show. FastAPI's HTML still names
`SwaggerUIBundle.SwaggerUIStandalonePreset` in its presets list, where it resolves
to `undefined` and is ignored - this is the same pair of files FastAPI's own docs
page loads from a CDN, so it is the configuration Swagger UI is normally run in,
not one particular to Dusk.

Swagger UI is Apache-2.0; `LICENSE` is its licence text, kept beside the code it
covers.

The page also turns off Swagger UI's validator badge (`validatorUrl: null`),
because the default hands this API's document address to `validator.swagger.io`.
`grep validator.swagger.io swagger-ui-bundle.js` still matches - that is the
default sitting unused inside the bundle, not a request the page makes.

## Verifying what is here

```bash
cd dusk/src/dusk_py/python/dusk/gw/static
sha256sum swagger-ui-bundle.js swagger-ui.css LICENSE
```

The digests must match the table above. `tests/interfaces/gw/test_openapi.py` separately
asserts the served page references no external host, which is the property that
actually matters - these digests only say *which* copy is here.

## Updating

Pick the new version, then fetch exactly these three files and update the table:

```bash
VERSION=5.32.14   # set to the version you are moving to
cd dusk/src/dusk_py/python/dusk/gw/static
curl -fLO "https://cdn.jsdelivr.net/npm/swagger-ui-dist@${VERSION}/swagger-ui-bundle.js"
curl -fLO "https://cdn.jsdelivr.net/npm/swagger-ui-dist@${VERSION}/swagger-ui.css"
curl -fL -o LICENSE "https://raw.githubusercontent.com/swagger-api/swagger-ui/v${VERSION}/LICENSE"
sha256sum swagger-ui-bundle.js swagger-ui.css LICENSE
```

Then check that the new CSS still inlines its images rather than fetching them,
because a build that changed this would put the page back on the network without
anything failing:

```bash
grep -o 'url([^)]*)' swagger-ui.css | grep -v '^url(data:'
```

That must print nothing. Run the gateway's tests afterwards.
