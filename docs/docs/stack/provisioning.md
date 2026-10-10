# Provisioning nodes

This page is for fleet operators: the people who decide which nodes may join a
fleet and who run the certificate authority that admits them. It covers the
credentials nightfall accepts, how to issue, cap and retire them, TPM
attestation, how to set up step-ca as the fleet-client certificate authority, the
certificate template, how certificates are renewed, and what to do when a
credential leaks.

The `step` commands on this page were run with step CLI 0.30.2 and step-ca
0.30.2, both in the `smallstep/step-ca:0.30.2` image; the others need OpenSSL 3
and GNU coreutils.

## How a node gets its certificate

nightfall assigns every node its installation id and derives its device id from
the hardware fingerprint the node reports. A node chooses neither id, but it does
choose the fingerprint it reports: whoever holds a valid credential and reports
another machine's fingerprint gets that machine's device id, with a new
installation id. [TPM attestation](#tpm-attestation) is what ties the device id
to the hardware.

A node generates a P-256 key pair locally and asks nightfall's provisioning
endpoint for a certificate in two calls:

1. **assign**, with a credential and a report about itself. nightfall checks the
   credential, derives the node's **device id**, assigns a new **installation
   id**, and returns both with a single-use challenge that expires after
   `[provision] challenge_ttl_ms` (five minutes by default).
2. **enroll**, with the same credential, the challenge and a certificate request
   (CSR) for its key. nightfall checks everything again, checks the CSR, and has
   step-ca sign it.

The certificate carries the node's identity as URI subject alternative names, and
nothing else identifies the node:

* `urn:dusk:device:<device id>`
* `urn:dusk:installation:<installation id>`
* `urn:dusk:tenant:<tenant>`, when the credential named a tenant.
* `urn:dusk:attestation:tpm`, when the node attested its key with a TPM.

Its subject is empty, its key usage is digital signature and its only extended
key usage is client authentication. nightfall also accepts a certificate request
whose subject is exactly the common name `<device id>.<installation id>`, which
nodes sent before; the certificate issued for it still has an empty subject.

The **device id** is 32 lowercase hex digits: the first 16 bytes of
HMAC-SHA256, keyed with the device id key, over `dusk-device-v1` followed by the
32-byte hardware fingerprint the node reports (the SHA-256 of its platform machine
id). The same machine gets the same device id every time it is installed, as
long as its machine id does not change. Two
machines cloned from one image with the same machine id get the same device id;
a nightfall instance warns when it sees one device id enroll more than 20
installations from more than 5 addresses within 24 hours.

The **installation id** is 32 lowercase hex digits, random and new for every
enrollment whatever the credential, so no credential can obtain a certificate
for an installation that already has one.

A node that already has a certificate renews it instead, presenting the current
certificate as its TLS client certificate (see
[Renewal and grace](#renewal-and-grace)).

nightfall refuses assign, enroll and renew for a device or an installation whose
lifecycle state is `revoked` or `retired`.

## The device id key

`[provision] device_id_key_file` holds the key every device id is derived from,
as at least 64 hexadecimal digits (32 bytes) on one line. Create it once:

```sh
openssl rand -hex 32 > device-id.key
```

This key is permanent. Changing it changes the device id of every machine in the
fleet, so back it up with your other secrets and never rotate it.

## Fleet tokens

A fleet token is a shared secret baked into the nodes you ship. Anyone who
extracts it from one node can enroll as many fake nodes as they like until you
retire it, and limiting enrollments per address does not stop an enrollment
spread over many addresses. Use one token per product or batch so that you can
retire one without the others, watch the enrollment rate, and prefer install
tokens where your installer can carry one per machine.

nightfall never stores a fleet token, only its SHA-256. `[provision]
fleet_tokens_file` is a TOML file with one `[[token]]` table per token:

```toml
[[token]]
name = "retail-eu-2026"
value_sha256 = "0e8ae4e5ae2bdd59c7b6a3fd1f4e5a9e15d2a0a6f78d5d1bfa1b0e7a5e3c3a2d"
tenant = "retail-eu"
max_installations = 50000

[[token]]
name = "retail-eu-2025"
value_sha256 = "9d4b2c7e1a3f5b8d0c6e4a2f7b9d1c3e5a8f0b2d4c6e9a1b3d5f7c0e2a4b6d8f"
tenant = "retail-eu"
retired = true

[[token]]
name = "lab"
value_sha256 = "6f2c7d0e4b1a9c8d3e5f7a2b4c6d8e0f1a3b5c7d9e2f4a6b8c0d1e3f5a7b9c2d"
```

* `name` (required, 1 to 256 bytes, unique) names the token in enrollment events
  as `credential_ref`. It is not a secret.
* `value_sha256` (required, unique) is the SHA-256 of the token as 64 lowercase
  hex digits.
* `tenant` (optional) is written into every certificate the token enrolls, as
  `urn:dusk:tenant:<tenant>`. It matches `[a-z0-9-]{1,63}`. A node cannot choose
  or change its tenant; it comes from the credential only.
* `max_installations` (optional, at least 1) is the most installations the token
  ever enrolls, across every nightfall instance; see
  [Capping installations](#capping-installations). Without it the token has no
  cap.
* `retired` (optional, default `false`): `true` refuses every assign and enroll
  with the token, with `denied: credential retired`. The address the token came
  from is not penalized for it.

Any other key, a repeated name or a repeated value makes nightfall refuse the
file. Create a token and its entry:

```sh
token=$(openssl rand -hex 32)
printf '%s' "$token" > fleet-token
printf '%s' "$token" | sha256sum | cut -d ' ' -f 1
```

`fleet-token` holds the token the nodes are built with (`DUSK_FLEET_TOKEN` when
the node is compiled); the printed digest is the entry's `value_sha256`. nightfall hashes the token exactly
as the node presents it, so `printf '%s'` writes it without a trailing
newline.

nightfall checks the file's modification time every 30 seconds and loads a
changed file without a restart: an added entry is accepted, a retired or
removed one is refused, and a changed tenant or cap applies to every enrollment
from then on. An enrollment between its assign and its enroll call when its
token is retired is refused at enroll. A file that does not load leaves the
previous entries in use; nightfall logs a warning and counts it in
`nightfall_credentials_reload_failures_total{file="fleet_tokens_file"}`. In
Kubernetes the file is a Secret mounted without `subPath`, so an updated Secret
reaches nightfall after the kubelet's next sync (one minute by default) plus its
cache delay.

To retire a token, set `retired = true` on its entry, or remove the entry.
Keeping the entry keeps naming the token in enrollment events, so twilight can
alert on nodes that still present it, and keeps it unpenalized; a removed
token is an unknown credential, which counts against the address that presents
it. Either way, nodes that already hold certificates keep them, keep their
sessions and keep renewing, since renewal needs only the certificate; to cut
those nodes off, revoke them (see [Revoking what a credential enrolled](#revoking-what-a-credential-enrolled)).

## Install tokens

An install token is a signed JSON Web Token made for one machine. It can be used
once across the whole fleet: whoever holds it, the machine it was made for or
someone who copied it, enrolls one machine with it, once. That enrollment gets a
new random installation id like any other.

`[provision] install_token_keys` is a JWKS file with the public keys whose
signatures nightfall accepts. Each key is an EC P-256 key (tokens signed `ES256`)
or an OKP Ed25519 key (tokens signed `EdDSA`). A key's id is its `kid`, or its
RFC 7638 thumbprint when it has none; two keys with the same id make nightfall
refuse the file. A file with `{"keys": []}` accepts no install tokens.

A token's header names its key (`kid`, required) and algorithm (`alg`). Its
claims:

| Claim | Required | Meaning |
|-------|----------|---------|
| `sub` | yes | Who or what the token was made for, 1 to 256 bytes, such as a serial number. Shown in enrollment events as `credential_ref`; it never becomes the installation id. |
| `iss` | yes | Who issued the token, 1 to 256 bytes. |
| `jti` | yes | The token's id, 1 to 256 bytes, unique per issuer. |
| `exp` | yes | Expiry, Unix seconds. |
| `nbf` | no | Not before, Unix seconds. |
| `tenant` | no | The tenant written into the certificate, `[a-z0-9-]{1,63}`. |

nightfall allows 60 seconds of clock difference on `exp` and `nbf` and ignores
other claims. The key that signed a token is its **issuer**: enrollment events
name it as `credential_issuer`, and caps, rate limits and bulk revocation group
a key's tokens under it. A token is single-use because nightfall passes
`hex(SHA-256(L("dusk-install-token") || L(iss) || L(jti)))`, where `L(x)` is the
length of `x` in 4 big-endian bytes followed by `x`, to step-ca as the id of the
one-time token it signs the certificate with, and step-ca refuses an id it has
seen before (see the database requirement under
[step-ca](#step-ca-the-fleet-client-ca)). The second enrollment with the same
token is denied with the reason `step_ca_unauthorized`.

Create a signing key and the JWKS nightfall reads, keeping `installer.jwk` with
whatever issues tokens:

```sh
step crypto jwk create installer.pub.json installer.jwk --kty OKP --crv Ed25519 --use sig --no-password --insecure
step crypto jwk keyset add install-token-jwks.json < installer.pub.json
```

Issue a token for one machine, valid for 30 days:

```sh
echo '{"tenant": "retail-eu"}' | step crypto jwt sign - --key installer.jwk \
  --aud dusk-provisioning --iss factory-eu --sub "kiosk-0042" \
  --jti "batch-7-unit-42" --exp $(( $(date +%s) + 30 * 86400 )) > install-token.jwt
```

The step CLI requires `--aud`; nightfall does not read it. Leave out the payload
(`echo '{}'`) for a token without a tenant.

A key in the JWKS may carry `max_installations`, a whole number of at least 1:
the most installations its tokens ever enroll together, across every nightfall
instance (see [Capping installations](#capping-installations)):

```json
{"keys": [{"kty": "OKP", "crv": "Ed25519", "x": "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", "kid": "factory-2026", "use": "sig", "max_installations": 50000}]}
```

`step crypto jwk keyset add` does not keep members it does not know, so add
`max_installations` after the last key is added. nightfall reloads a changed JWKS
the way it reloads the fleet tokens file. To stop accepting a key's tokens,
remove the key from the JWKS; its tokens are then refused as
`invalid_install_token`.

## Capping installations

A fleet token entry or an install token key with `max_installations` enrolls at
most that many installations, ever. The installation that would go beyond it is
refused with `denied: credential quota reached` and the reason
`credential_quota_reached`, at assign and at enroll, on every nightfall
instance. Renewals do not count and are never refused for it; revoking an
installation does not give its place back.

nightfall counts the installations on the Kafka topic `dusk.credential-quota`,
which every instance reads from the beginning. Just before it asks step-ca to
sign, nightfall writes a reservation there and waits until it reads that
reservation back: in the order the topic holds them, a reservation is granted
while fewer than the cap are granted before it, so two instances racing for the
last place cannot both get it. A reservation whose signing fails is given back.
A reservation that is not read back within 10 seconds is refused as
`credential_quota_unavailable`, and given back once it is read if it was
granted. A nightfall instance that crashes between a reservation and its
signing keeps that place used: the cap can be reached early, never passed.

nightfall counts the installations of credentials without a cap too: once a
certificate is issued, it records the installation on the same topic without
waiting for it. A cap set on a credential later therefore counts what the
credential enrolled before, and refuses at once when that is already the cap or
more. An installation Kafka does not take is logged and counted in
`nightfall_credential_quota_record_failures_total`, and a later cap does not
count it.

While an instance has not read the topic to its end - right after it starts -
and while Kafka does not take the reservation, it refuses capped credentials
with `overloaded` and the reason `credential_quota_unavailable`, and the node
retries. Credentials without a cap never wait for the topic.

`nightfall_credential_installations_remaining{credential_kind, credential}`
reports what is left of each cap, and
`nightfall_credential_quota_reservations_total{outcome}` counts reservations
`granted`, `refused`, `unconfirmed` and `unwritten`.

## TPM attestation

A node whose key lives in its TPM 2.0 can prove that the key was created in a
genuine TPM and never leaves it. Attestation comes on top of a fleet token or an
install token, never instead of one: nightfall checks the token first and the
TPM's evidence after it. A fleet token taken from one machine then enrolls only
as many attesting nodes as whoever took it has TPMs. nightfall still enrolls a
node that does not attest, so a copied token still enrolls nodes without a TPM;
`urn:dusk:attestation:tpm` in a certificate tells the attested ones apart.

A node attests when its key is in a TPM whose RSA endorsement key has a
certificate in NV (see [The TPM](../getting-started/guides/connect-to-nightfall.md#the-tpm)).
Its assign and enroll then carry the TPM's endorsement key, that key's
certificate, the certificates the TPM keeps beside it and the public area of the
node key, and its hardware fingerprint is the SHA-256 of the endorsement key's
public area, so its device id follows the TPM.

### The endorsement roots

`[provision] tpm_endorsement_roots` names a PEM file of the certificates
nightfall trusts to sign endorsement key certificates: the root certificates of
your TPMs' manufacturers, and any intermediate certificate your TPMs do not keep
in NV. Every certificate in the file is trusted as it is, so an intermediate
there needs no root above it. Dusk ships none; take them from your TPM
manufacturers' certificate pages. nightfall reads the file when it starts.

```toml
[provision]
tpm_endorsement_roots = "/etc/nightfall/pki/tpm-endorsement-roots.pem"
```

The default, `""`, trusts no manufacturer: nightfall refuses every node that
attests, with the reason `tpm_roots_missing`, and such a node enrolls only when
it runs with `--no-tpm`. To see which manufacturer's certificate authority a
machine's TPM needs, read its certificate's issuer with tpm2-tools and OpenSSL:

```sh
tpm2_nvread 0x1c00002 -o ek.der
openssl x509 -inform der -in ek.der -noout -issuer
```

A software TPM made with swtpm's `swtpm_setup --create-ek-cert` has its
endorsement key certificate signed by swtpm's local CA, whose root is
`/var/lib/swtpm-localca/swtpm-localca-rootca-cert.pem` on the machine that ran
`swtpm_setup`. Trust it in a development fleet only.

### What nightfall checks

On assign, after the token, nightfall checks that:

1. the endorsement key's public area is TCG template L-1 byte for byte (RSA
   2048, the endorsement hierarchy's policy, AES-128-CFB), with only the modulus
   the TPM's own;
2. the node key's public area is the node key template byte for byte, with only
   the point the TPM's own: an ECDSA P-256 signing key with SHA-256 that the TPM
   generated and can never export (`fixedTPM`, `fixedParent`,
   `sensitiveDataOrigin`), neither restricted nor able to decrypt, with no
   policy;
3. the hardware fingerprint is the SHA-256 of the endorsement key's public area;
4. the endorsement key has a certificate: nightfall refuses one that has none,
   since nothing vouches for it;
5. that certificate chains to a certificate in `tpm_endorsement_roots` through
   the certificates the node sent, carries the TCG endorsement key certificate
   usage `2.23.133.8.1`, and certifies the endorsement key itself.

Expiry is the one thing nightfall forgives. Endorsement key certificates
commonly outlive their validity on long-lived devices, so a chain that has
expired is checked as of the time the endorsement key certificate was issued.
Every other check is strict, a certificate that is not valid yet included.

nightfall then answers assign with a credential made for the node key, as
TPM2_MakeCredential makes it: only the TPM that holds that endorsement key, with
that very node key loaded, can decrypt the challenge in it. The challenge is
used once, expires after `challenge_ttl_ms`, and is bound to the token, the
device, the installation and the node key. enroll must carry the decrypted
challenge and a CSR for the node key itself, and step-ca adds
`urn:dusk:attestation:tpm` to the certificate it issues (see
[the template](#3-write-the-certificate-template)).

Each endorsement key has an enrollment rate of its own, beside its token's (see
[Limits](#limits)).

## step-ca, the fleet-client CA

step-ca signs node certificates. It is the fleet-client certificate authority:
its root is trusted for nothing but node certificates, it is reachable only from
nightfall, and its only provisioner is nightfall's JWK provisioner, registered by
its public key. nightfall checks identity; step-ca holds the signing key.

### 1. Create the CA

In the `smallstep/step-ca:0.30.2` image, with a persistent volume mounted at
`/home/step`:

```sh
mkdir -p /home/step/secrets
head -c 32 /dev/urandom | base64 > /home/step/secrets/password
step ca init --deployment-type standalone --name "Dusk fleet-client" \
  --dns step-ca,localhost --address :9000 \
  --provisioner initial --password-file /home/step/secrets/password
```

`--dns` must hold the host name of `[step_ca] url` in nightfall's configuration
(`step-ca` for `https://step-ca:9000`): nightfall checks step-ca's TLS certificate
against it, and step-ca accepts a one-time token only when its audience
(`<url>/1.0/sign`) names one of these hosts. The port does not matter for the
audience.

`step ca init` writes `"db": {"type": "badgerv2", "dataSource": "/home/step/db"}`
into `/home/step/config/ca.json`. Keep that database on persistent storage and
run one step-ca replica: it is where step-ca remembers the one-time token ids it
has accepted, and install tokens are single-use only as long as it does. In
production, keep the intermediate key in a KMS or HSM (`kms` in `ca.json`)
instead of `/home/step/secrets`.

### 2. Create nightfall's provisioner key

Where you keep nightfall's secrets:

```sh
step crypto jwk create nightfall.pub.json nightfall.jwk --kty EC --crv P-256 --use sig --no-password --insecure
```

`nightfall.jwk` is the private key: it becomes nightfall's
`[step_ca] provisioner_key_file` and never goes to the CA. nightfall needs it
unencrypted and refuses an encrypted one. Copy only `nightfall.pub.json` to the
CA.

### 3. Write the certificate template

Save this as `node.tpl` next to `nightfall.pub.json`:

```text
{{- $device := "" }}
{{- $installation := "" }}
{{- if ne (len .SANs) 2 }}{{ fail "a node certificate names exactly two SANs" }}{{ end }}
{{- range .SANs }}
  {{- if ne .Type "uri" }}{{ fail "a node certificate names URI SANs only" }}{{ end }}
  {{- if regexMatch "^urn:dusk:device:[0-9a-f]{32}$" .Value }}{{ $device = .Value }}
  {{- else if regexMatch "^urn:dusk:installation:[0-9a-f]{32}$" .Value }}{{ $installation = .Value }}
  {{- else }}{{ fail "a node certificate names one device and one installation URI" }}{{ end }}
{{- end }}
{{- if or (eq $device "") (eq $installation "") }}{{ fail "a node certificate names one device and one installation URI" }}{{ end }}
{{- $tenant := "" }}
{{- if .Token.tenant }}{{ $tenant = .Token.tenant }}{{ end }}
{{- if and $tenant (not (regexMatch "^[a-z0-9-]{1,63}$" $tenant)) }}{{ fail "the tenant claim is not a valid tenant" }}{{ end }}
{{- $attested := false }}
{{- if .Token.attestation }}{{ if ne .Token.attestation "tpm" }}{{ fail "the attestation claim is not tpm" }}{{ end }}{{ $attested = true }}{{ end }}
{
  "subject": {},
  "sans": [
    {"type": "uri", "value": {{ toJson $device }}},
    {"type": "uri", "value": {{ toJson $installation }}}{{ if $tenant }},
    {"type": "uri", "value": {{ toJson (printf "urn:dusk:tenant:%s" $tenant) }}}{{ end }}{{ if $attested }},
    {"type": "uri", "value": "urn:dusk:attestation:tpm"}{{ end }}
  ],
  "keyUsage": ["digitalSignature"],
  "extKeyUsage": ["clientAuth"]
}
```

It refuses to sign unless the subject alternative names nightfall asked for are
exactly one device URI and one installation URI, each with 32 lowercase hex
digits; it adds the tenant URI from the token's `tenant` claim when there is one
and refuses a malformed tenant; it adds `urn:dusk:attestation:tpm` when the
token's `attestation` claim is `tpm` and refuses any other value; and it allows
the key nothing but client authentication. nightfall checks all of this before it asks step-ca, so the
template is the second line of defence, at the component that holds the key.

### 4. Register the provisioner

The step CLI needs the CA running while it changes `ca.json`. Start step-ca
(the image's default command), copy `nightfall.pub.json` and `node.tpl` into
`/home/step`, and run in that directory inside the container:

```sh
step ca provisioner add nightfall --type JWK --public-key nightfall.pub.json \
  --x509-template node.tpl \
  --x509-min-dur 5m --x509-default-dur 168h --x509-max-dur 168h \
  --disable-renewal
step ca provisioner remove initial
```

Then restart step-ca. The provisioner now holds only the public key, the
template, a default and maximum certificate lifetime of 168 hours (set both to
nightfall's `[step_ca] certificate_lifetime` if you change it), and
`disableRenewal`, so step-ca refuses `/renew` and every certificate is renewed
through nightfall, which checks the node's state and limits first.

### 5. Point nightfall at it

```toml
[step_ca]
url = "https://step-ca:9000"
root = "/etc/nightfall/pki/fleet-client-root.crt"
provisioner = "nightfall"
provisioner_key_file = "/etc/nightfall/secrets/provisioner.jwk"
certificate_lifetime = "168h"
max_concurrent = 16
timeout_ms = 10000
```

`root` is the CA's `/home/step/certs/root_ca.crt`; the same file is
`[fleet] client_ca`, which nightfall uses to check the certificates nodes present.
`provisioner_key_file` is `nightfall.jwk`. nightfall sends at most
`max_concurrent` signing requests at once and answers a node `overloaded` beyond
that, and gives up on a request after `timeout_ms`.

## Renewal and grace

Every certificate nightfall issues comes with the time the node should renew it:
a random point between 55% and 75% of its lifetime, drawn anew for each
certificate, so a fleet enrolled at once does not renew at once.

A node renews by presenting its current certificate as its TLS client
certificate and sending a CSR for a **new** key. nightfall:

* checks the certificate against the fleet-client CA, ignoring only its expiry;
* accepts it until `[provision] renew_grace` (default `2160h`, 90 days) after it
  expired;
* accepts a renewal only after half of the certificate's lifetime has passed, and
  at most 4 renewals of one device and installation within 24 hours on one
  nightfall instance;
* requires the CSR to name exactly the device and installation of the current
  certificate and to carry a key other than the current one;
* issues the new certificate with the same device, installation and tenant.

A certificate with `urn:dusk:attestation:tpm` is renewed with the key it already
has instead: that key cannot leave its TPM, so nightfall refuses a CSR for any
other key, and the new certificate carries `urn:dusk:attestation:tpm` again.

Renewal is not an enrollment: it does not count against the enrollment limits and
needs no fleet token.

A node whose certificate expired more than `renew_grace` ago is refused with
`renew refused: the certificate expired beyond the renew grace` and enrolls again
with its fleet token. That enrollment creates a new installation id, so the node
appears in the fleet as a new installation of the same device.

## Limits

* `[limits] enrollments_per_second` (default 50) for the whole nightfall instance
  and `enrollments_per_second_per_credential` (default 10) per credential - one
  fleet token entry, or every install token one key signed - and as many again
  per TPM endorsement key for a node that attests. assign and enroll each take
  one, so one enrollment takes two: one credential enrolls at most 5 nodes a
  second through one instance, and that many times the number of instances
  across the stack. The credential is checked first, so a credential that does
  not verify takes nothing from these buckets and counts against its address
  instead. Beyond any of them, the node is told `overloaded` and retries later.
* `max_installations` caps what a credential enrolls in all; see
  [Capping installations](#capping-installations).
* A credential that fails counts against the address it came from; an address
  with more than `[limits] credential_failures_per_ip_per_hour` (default 20)
  failures is refused for `penalty_seconds` (default 60).
* `[[limits.cidr]] enrollments_per_hour` caps the enrollments a network starts in
  an hour, counted at assign only; beyond it assign is told `overloaded`.
* Above `[limits] enrollment_alert_per_minute` (default 600) assign and enroll
  calls in the last minute, nightfall logs a warning and sets the gauge
  `nightfall_enrollment_rate_alert` until the rate falls back.
* At most `[step_ca] max_concurrent` signing requests are in flight at once.

## What is recorded

Every assign, enroll and renew, whatever its outcome, produces one message on the
Kafka topic `dusk.enrollments` (defined by
`services/contracts/kafka/dusk.enrollments.schema.json` in the Dusk repository) with the operation, the outcome (`assigned`, `issued`, `denied`,
`rate_limited` or `error`), the device and installation ids when known, the
tenant, the credential kind and `credential_ref` (a fleet token's `name` or an
install token's `sub`, never the secret), `credential_issuer` (the key id of an
install token's key), the SHA-256 of the hardware fingerprint,
the address the call came from, the issued certificate's serial, fingerprint and
expiry, what the node reported about itself, and the nightfall instance.
`reason` says why a call did not succeed:

| `reason` | Outcome | What happened |
|----------|---------|---------------|
| `malformed_request` | `denied` | nightfall could not read the request, for example a credential that is not UTF-8. It counts as a failed credential. |
| `penalty_box` | `rate_limited` | The address is serving a penalty for failed credentials. |
| `network_enrollment_rate` | `rate_limited` | The address's `[[limits.cidr]]` network used up its `enrollments_per_hour`. |
| `enrollment_rate` | `rate_limited` | The instance's enrollment rate is exhausted. |
| `credential_rate` | `rate_limited` | This credential's enrollment rate is exhausted. |
| `endorsement_key_rate` | `rate_limited` | This TPM endorsement key's enrollment rate is exhausted. |
| `invalid_credential` | `denied` | No fleet token entry matches. |
| `credential_retired` | `denied` | The fleet token entry is `retired`. |
| `credential_quota_reached` | `denied` | The fleet token entry or the install token key enrolled its `max_installations`. |
| `credential_quota_unavailable` | `error` | The credential has a cap and the instance could not count against it: it has not read `dusk.credential-quota` to its end yet, or Kafka did not take or return the reservation within 10 seconds. |
| `invalid_install_token` | `denied` | The install token's signature, claims or validity period is wrong. |
| `certificate_credential` | `denied` | assign or enroll was called with the certificate credential, which only renew takes. |
| `tpm_roots_missing` | `denied` | The node attested and `[provision] tpm_endorsement_roots` is empty. |
| `tpm_key_template` | `denied` | The endorsement key or the node key does not follow its template. |
| `tpm_fingerprint_mismatch` | `denied` | The hardware fingerprint is not the SHA-256 of the endorsement key. |
| `tpm_certificate_missing` | `denied` | The node attested with an endorsement key that has no certificate. |
| `tpm_certificate_untrusted` | `denied` | The endorsement key certificate does not chain to `tpm_endorsement_roots`, is not valid yet, lacks the endorsement key certificate usage, or certifies another key. |
| `tpm_credential` | `error` | nightfall could not make the TPM credential. |
| `invalid_hardware_fingerprint` | `denied` | The hardware fingerprint is not 32 bytes. |
| `device_revoked`, `device_retired` | `denied` | The device's lifecycle state forbids certificates. |
| `installation_revoked`, `installation_retired` | `denied` | The installation's lifecycle state forbids certificates. |
| `invalid_challenge` | `denied` | The challenge is unknown, already used or expired. |
| `challenge_mismatch` | `denied` | The challenge was issued for another credential or hardware fingerprint. |
| `invalid_csr` | `denied` | The CSR is not a self-signed P-256 request for exactly the assigned identity. |
| `tpm_key_mismatch` | `denied` | enroll after attesting, with a CSR for another key than the attested node key. |
| `invalid_certificate` | `denied` | renew without a certificate of the fleet-client CA that names one device and one installation. |
| `beyond_renew_grace` | `denied` | renew with a certificate that expired more than `renew_grace` ago. |
| `renew_too_early` | `rate_limited` | renew before half of the certificate's lifetime. |
| `renew_key_reused` | `denied` | renew with the key of the current certificate. |
| `renew_key_changed` | `denied` | renew of a certificate with `urn:dusk:attestation:tpm` with another key than its own. |
| `renew_limit` | `rate_limited` | The fifth renewal within 24 hours. |
| `challenge_store_full` | `error` | Too many enrollments are between assign and enroll on this instance. |
| `step_ca_saturated` | `error` | `max_concurrent` signing requests were already in flight. |
| `step_ca_unauthorized` | `denied` or `error` | step-ca refused the one-time token. For an install token it is `denied`: the token was used before. Otherwise it is `error`: step-ca refused nightfall's own token, so the provisioner key, the host in `[step_ca] url` or a clock is wrong. |
| `step_ca_refused` | `error` | step-ca refused the request, for example because the template's checks failed. |
| `step_ca_error` | `error` | step-ca failed, did not answer within `timeout_ms`, or answered with something that is not a certificate chain. |
| `random_source` | `error` | The system's random number source failed. |

nightfall's log holds the detail of every refusal under the same reason.

## When a credential leaks

### What whoever holds it can do

A fleet token is in every binary built with it, and an install token key is
wherever tokens are minted. Whoever extracts either one can, until it is
retired:

* **Enroll new installations.** Each gets a certificate, a new installation id
  and the token's tenant, and a device id derived from whatever hardware
  fingerprint it reports, another machine's included. No credential gets a
  certificate for an installation that already exists, or renews another
  installation's certificate.
* **Link those installations to the stack.** They are ordinary nodes to dawn and
  twilight: their reported facts are believed, they join the campaigns those
  facts match and receive those campaigns' scripts, and the results they report
  count in the campaigns' health gates.

A single install token is good for one enrollment, by whoever presents it first.

### What stops it

* **Rates.** One credential enrolls at most 5 nodes a second through one
  nightfall instance (`enrollments_per_second_per_credential`, counted at assign
  and at enroll), each instance at most 25 (`enrollments_per_second`), and a
  network with a `[[limits.cidr]]` table at most its `enrollments_per_hour`; see
  [Limits](#limits). The rates are per instance, so they slow a leak down
  rather than end it.
* **A cap.** `max_installations` on the token's entry or the key is a hard limit
  across every instance and every restart; see
  [Capping installations](#capping-installations). Give every token and key the
  cap its batch needs.
* **Retiring it.** A retired or removed token is refused by every instance
  within 30 seconds of its file changing, with no restart and no session
  dropped.
* **Revoking what it enrolled.** One request to twilight revokes every
  installation the token enrolled; nightfall drops their sessions and refuses
  them from then on. See
  [Revoking what a credential enrolled](#revoking-what-a-credential-enrolled).

### What you are alerted on

twilight counts new enrollments only - an issued `enroll` and a refused
`assign` or `enroll`, never a renewal - for each credential, each network (a
/24 for IPv4, a /48 for IPv6) and the whole fleet, against the same hour of the
previous seven days, and raises:

| Alert | When |
|-------|------|
| `enrollment_rate` | More installations enrolled in five minutes, for one credential, one network or the fleet, than its floor and four times its baseline. |
| `denied_enrollments_spike` | The same for refused enrollments: someone is presenting a retired, removed, capped or wrong credential, or pressing against the rate limits. |
| `enrollment_from_new_network_burst` | The same for installations enrolled from networks none enrolled from in the seven days before: a leak used from many places at once. |
| `credential_quota_reached` | A credential's cap refused an enrollment. |

Every one names the credential, the networks, the count and the baseline, has
its own fingerprint for each credential or network so that a second leak opens a
second alert, notifies again when its count doubles, and resolves itself after
the rate stays below its threshold for 15 minutes. The thresholds are twilight's
`alerts.enrollments` settings ([twilight](twilight.md#alerts)); what to do about
each alert is in the [runbooks](runbooks/enrollment-from-new-network-burst.md).
Each nightfall instance also sets `nightfall_enrollment_rate_alert` above
`enrollment_alert_per_minute` and counts `device_id_collision`, on its own
enrollments only.

### Rotating and retiring a token

1. Create a new token and add its entry, with a cap, beside the old one;
   nightfall takes it within 30 seconds. Build the nodes you ship from then on
   with it.
2. Set `retired = true` on the old entry. New enrollments with it are refused at
   once; the nodes it enrolled keep their certificates and sessions.
3. If the old token leaked, revoke what it enrolled since the leak, as below.

A node rebuilt with another fleet token cannot read the persistent store the
earlier build wrote, since the token keys its encryption (see
[the security model](security.md#the-fleet-token)): such a node enrolls again
as a new installation of the same device.

### Revoking what a credential enrolled

twilight keeps the credential every node enrolled with: `credential_kind`,
`credential_ref` (the fleet token entry, or the install token's subject) and
`credential_issuer` (the install token's key), all selector fields. List the
nodes first:

```sh
curl -sS -H "Authorization: Bearer $TWILIGHT_TOKEN" -G \
  --data-urlencode 'selector=credential_kind == "fleet_token" and credential_ref == "retail-eu-2025"' \
  https://twilight.example.org/api/v1/nodes
```

Then count, and revoke, every installation one fleet token entry
(`"credential_kind": "fleet_token"` and its name) or one install token key
(`"credential_kind": "install_token"` and its key id) enrolled, optionally only
within `enrolled_after` and `enrolled_before`. `dry_run` counts without changing
anything:

```sh
curl -sS -X POST -H "Authorization: Bearer $TWILIGHT_TOKEN" -H 'Content-Type: application/json' \
  -d '{"credential_kind": "fleet_token", "credential": "retail-eu-2025", "enrolled_after": "2026-10-09T02:00:00Z", "reason": "token leaked", "dry_run": true}' \
  https://twilight.example.org/api/v1/revocations
```

The same request without `dry_run` needs the admin role and answers `202` with
the revocation. twilight revokes the installations at
`engine.revocations_per_second` (1000 by default). Each gets a
`dusk.node-state` record with the reason and your name, every nightfall instance
drops the installation's sessions when it reads the record, and twilight then
marks its inventory row `revoked`. A revocation carries on across a twilight
restart or a change of leader, and `GET /api/v1/revocations/{id}` shows how many
installations it revoked so far. A revoked installation is refused at every
handshake and every renewal. Retire the token before you revoke, so that
nothing enrolls with it in the meantime.
