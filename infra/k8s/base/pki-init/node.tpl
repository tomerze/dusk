{{- $device := "" -}}
{{- $installation := "" -}}
{{- $devices := 0 -}}
{{- $installations := 0 -}}
{{- range .SANs -}}
{{- if and (eq .Type "uri") (regexMatch "^urn:dusk:device:[0-9a-f]{32}$" .Value) -}}
{{- $device = trimPrefix "urn:dusk:device:" .Value -}}
{{- $devices = add1 $devices -}}
{{- else if and (eq .Type "uri") (regexMatch "^urn:dusk:installation:[0-9a-f]{32}$" .Value) -}}
{{- $installation = trimPrefix "urn:dusk:installation:" .Value -}}
{{- $installations = add1 $installations -}}
{{- else -}}
{{- fail (printf "subject alternative name %s:%s is neither a dusk device nor a dusk installation URI" .Type .Value) -}}
{{- end -}}
{{- end -}}
{{- if ne $devices 1 -}}
{{- fail "the request must carry exactly one urn:dusk:device URI" -}}
{{- end -}}
{{- if ne $installations 1 -}}
{{- fail "the request must carry exactly one urn:dusk:installation URI" -}}
{{- end -}}
{{- $commonName := printf "%s.%s" $device $installation -}}
{{- if and .Insecure.CR.Subject.CommonName (ne .Insecure.CR.Subject.CommonName $commonName) -}}
{{- fail "the request common name must be empty or <device id>.<installation id>" -}}
{{- end -}}
{{- $tenant := "" -}}
{{- if hasKey .Token "tenant" -}}
{{- $tenant = toString .Token.tenant -}}
{{- if not (regexMatch "^[a-z0-9-]{1,63}$" $tenant) -}}
{{- fail "the tenant claim must match [a-z0-9-]{1,63}" -}}
{{- end -}}
{{- end -}}
{{- $attested := false -}}
{{- if hasKey .Token "attestation" -}}
{{- if ne (toString .Token.attestation) "tpm" -}}
{{- fail "the attestation claim must be tpm" -}}
{{- end -}}
{{- $attested = true -}}
{{- end -}}
{
  "subject": {},
  "sans": [
    {"type": "uri", "value": {{ toJson (printf "urn:dusk:device:%s" $device) }}},
    {"type": "uri", "value": {{ toJson (printf "urn:dusk:installation:%s" $installation) }}}
{{- if $tenant }},
    {"type": "uri", "value": {{ toJson (printf "urn:dusk:tenant:%s" $tenant) }}}
{{- end }}
{{- if $attested }},
    {"type": "uri", "value": "urn:dusk:attestation:tpm"}
{{- end }}
  ],
  "keyUsage": ["digitalSignature"],
  "extKeyUsage": ["clientAuth"]
}
