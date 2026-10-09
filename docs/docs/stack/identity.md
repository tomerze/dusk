# Identity and trust

This page is for the people who run the Dusk stack and for security reviewers.
It defines the three identifiers every part of the stack uses for a node, says
who makes each one and what it can be trusted for, describes the certificate a
node holds and the certificates the services hold, and lays out the three
certificate authorities and what each one is trusted for.

Where it says what a node does or keeps - its `--ca`, its persistent kvs keys -
it describes the node's side as the node's pull requests build it (connect
mode, pull request #174; persistent kvs keys, issue #93), which are in review
on `master` and not merged yet
([Nodes that dial out](nodes.md)).

## The three identifiers

A node is named by two durable identifiers and one that lives only as long as
one run of Dusk.

| Identifier | What it names | Format | Who makes it |
|------------|---------------|--------|--------------|
| device id | The machine. The same across reinstalls. | 32 lowercase hex digits | nightfall derives it at enrollment from what the node reports. |
| installation id | One installation of Dusk on that machine. Gone when the installation is. | 32 lowercase hex digits | nightfall assigns it at enrollment. |
| namespace id | One running Dusk instance (a node's namespace). New every time Dusk starts. | A 64-bit integer; 16 lowercase zero-padded hex digits outside Cap'n Proto | The node, at random, each time it starts. |

### Device id

The device id is the first 16 bytes, as hex, of
`HMAC-SHA256(device id key, "dusk-device-v1" || hardware fingerprint)`.

* The **hardware fingerprint** is 32 bytes the node sends when it enrolls: the
  SHA-256 of the platform's machine id - the value the node reads as
  `dusk.device.id`. Enrollment sends nightfall the fingerprint, never the
  machine id; enrollment events carry only a hash of the fingerprint, and dawn
  drops `dusk.device.id` from every facts read. A machine with no usable
  machine id (none, empty, all zeros or `uninitialized`) sends 32 random bytes
  instead and keeps them in a persistent kvs key, so it gets a new device id
  when it is reinstalled.
* The **device id key** is a secret only nightfall holds
  (`[provision] device_id_key_file`). It is permanent: it is backed up and never
  rotated, since changing it would change the device id of every machine. A new
  key would only ever apply under a new label, `dusk-device-v2`.

The device id is only as good as the machine id behind it. Machines cloned from
one image share a machine id and so share a device id - each nightfall
instance warns when one device id enrolls more than 20 installations through
it from more than 5 addresses within 24 hours, counting in memory, so
enrollments spread over instances or across a restart are not counted
together - and the fingerprint is whatever the enrolling node sends, so
the device id names a machine id, not proven hardware. A node that attests with
a TPM reports the SHA-256 of its TPM's endorsement key instead, which nightfall
checks against that key's certificate, so its device id names that TPM (see
[TPM attestation](provisioning.md#tpm-attestation)).

### Installation id

nightfall assigns the installation id when a node enrolls: 128 random bits,
whatever the credential. The node keeps it in a persistent kvs key
([Nodes that dial out](nodes.md#where-the-node-keeps-its-identity)). An
enrollment never gets a certificate for an installation that already has one:
every enrollment creates a new installation, and an install token is good for
one enrollment. A node whose persistent store is gone, or whose certificate
expired more than the renew grace ago, enrolls again as a new installation of
the same device.

### Namespace id

Every Dusk instance - every node namespace - draws a random 64-bit namespace id
when it starts, and reports it with `Dusk.namespaceId`. Several Dusk nodes can
run in one process; when they share a persistent store they are one installation
with one certificate, and each links to the stack with a namespace id of its
own. The namespace id is not in the certificate.

### What keys on what

| Use | Keyed on |
|-----|----------|
| The inventory, campaign rows, phase buckets, intended processes, ledger queries, node state (revocation, quarantine) | device id + installation id |
| Reaching a node now: the server name dawn connects with, the `dusk.connections` key, the census | namespace id |
| A secondary handle when the namespace id is not known | the certificate fingerprint (SHA-256 of its DER), which changes on every renewal |

## Sessions

A node's session is the triple **(device id, installation id, namespace id)**.
When a node connects, nightfall reads the device and installation from its
certificate, asks the node for its namespace id, and binds the namespace id to
that identity for the session. Several namespaces of one installation are
distinct sessions, and they coexist.

* **Binding conflict.** A namespace id already bound to a different device and
  installation, on this nightfall instance or on another one, is refused: the
  session is closed, logged at `error`, written to the ledger as a
  `binding_conflict` event and counted. While a session holds a namespace id,
  no other identity can claim it, so the server name `<namespace id>.<suffix>`
  reaches the node whose certificate claimed that namespace id, and no other.
* **Reconnect.** The same identity with the same namespace id, on any instance,
  is the node reconnecting. The new session replaces the old one, which is
  closed with the reason `replaced`.
* **Epochs.** Every session gets an epoch: the largest of the current time in
  microseconds, one more than the last epoch this nightfall instance issued,
  and one more than the last epoch known for that namespace and identity. So a
  newer session has a larger epoch whenever the instance knows the earlier
  one. An instance that knows nothing of the namespace issues the clock's
  value, which an older session's epoch from an instance with a clock ahead of
  its own can exceed; when that higher epoch shows up, it wins, and the local
  session is closed as `replaced`, with a warning logged. Epochs order sessions in nightfall,
  its directory of sessions and twilight's online view - twilight discards
  connection events older than the epoch it already knows for a namespace. No
  command carries one.

## The node certificate

A node generates its key pair itself and never sends the private key anywhere.
nightfall checks who the node is; step-ca signs the certificate.

| Property | Value |
|----------|-------|
| Key | ECDSA P-256, generated on the node, in its TPM where it has one. See [where the private key lives](nodes.md#where-the-private-key-lives). |
| Subject | Empty. Nodes send certificate requests with an empty subject - `<device id>.<installation id>` is 65 characters, over the 64 a common name may hold - and the certificate template issues an empty subject. nightfall also accepts a request whose only subject is `CN=<device id>.<installation id>`. |
| Subject alternative names | Exactly `urn:dusk:device:<device id>` and `urn:dusk:installation:<installation id>`, plus `urn:dusk:tenant:<tenant>` when the credential the node enrolled with names a tenant and `urn:dusk:attestation:tpm` when the node attested its key with a TPM. No DNS, IP or email names. These URIs are the node's identity: the CA's signature over them is what binds the key to the node. |
| Key usage | Digital signature; extended key usage client authentication only. |
| Lifetime | 168 hours, nightfall's `[step_ca] certificate_lifetime`, which is also the maximum step-ca's provisioner allows. |
| Renewal | At a random point between 55% and 75% of the lifetime, drawn anew for each certificate. The node presents its current certificate and a request for a new key with the same identity, or for the same key when the certificate carries `urn:dusk:attestation:tpm`; nightfall accepts that up to `[provision] renew_grace` (90 days) after the certificate expired. |

The **tenant** comes from the credential only - a fleet token's entry or an
install token's claim, `[a-z0-9-]{1,63}` - and a renewal carries over the
tenant of the certificate it renews. A node cannot choose its tenant.

How a node enrolls and renews, step by step, is on
[Nodes that dial out](nodes.md) and [Provisioning nodes](provisioning.md).

## Principal certificates

Everything that connects to nightfall's inner listener or its admin API - dawn,
operators' tools - and every service that calls dawn or twilight with a client
certificate presents a **principal certificate** from the internal CA. It must
carry exactly one URI `urn:dusk:principal:<name>` and no `urn:dusk:device:` or
`urn:dusk:installation:` URI; a certificate that breaks either rule is refused,
so a node certificate is never accepted as a principal. In nightfall a name is
at most 128 bytes.

Each instance has a name of its own - `dawn-0`, `dawn-1` - and roles are given
by name pattern: in nightfall's `permissions.toml`, `dawn-*` gets the role
`dawn`; in dawn's configuration, `twilight-*` gets the role `dispatcher`.
Internal client certificates live 24 hours; cert-manager renews them in the
prod overlay.

## The three trust domains

Three certificate authorities, which never share a root:

| Authority | Signs | Trusted by | Key held by |
|-----------|-------|------------|-------------|
| **fleet-server** | nightfall's fleet and provisioning server certificates, and nothing else. | Nodes only: a node's `--ca` holds this CA and nothing else, not even the system's roots. | cert-manager in the prod overlay; the pki-init job in development. |
| **fleet-client** | Node certificates, and step-ca's own TLS certificate. | nightfall only: `[fleet] client_ca` for node certificates, and `[step_ca] root` for step-ca's own TLS certificate. | step-ca, a deployment of its own, reachable only from nightfall. Its only provisioner is nightfall's JWK provisioner, registered by its public key; nightfall holds the private half. In production, keep its intermediate key in a KMS or an HSM (`kms` in its `ca.json`). |
| **internal** | Principal and service certificates: nightfall's inner and admin listeners, dawn, twilight, operators. | nightfall's `[inner] client_ca` and `[admin] client_ca`; dawn and twilight for their callers and for the services they call. | cert-manager in the prod overlay; the pki-init job in development. nightfall holds no key that can sign on it. |

* nightfall refuses to start when `[fleet] client_ca` and `[inner] client_ca`
  share a certificate, compared by public key, so no node can ever be taken
  for a principal or a principal for a node.
* step-ca's certificate template refuses to sign unless the request names
  exactly one device URI and one installation URI of 32 hex digits each, and
  issues client authentication only. nightfall checks all of that before it
  asks; the template is the second check, at the component that holds the key.
* step-ca's provisioner has renewal disabled, so every renewal goes through
  nightfall, which checks the node's state and its renewal limits first.
* step-ca keeps the ids of the one-time tokens it accepted in a persistent
  database, which is what makes an install token usable once across the fleet.

What each key gives whoever holds it:

| Key | Lets its holder |
|-----|-----------------|
| fleet-server CA | Pose as nightfall to every node. Changing it means reinstalling every node with the new trust anchor. |
| fleet-client CA (step-ca's intermediate) | Issue a certificate for any device and installation, and so connect as any node whose lifecycle state does not refuse it. |
| nightfall's step-ca provisioner key | The same, through step-ca, within its template. |
| internal CA | Issue a certificate for any principal, and so connect to nightfall's inner listener with that principal's roles. |
| install token signing key | Sign install tokens, each good for one enrollment of a new installation, with any tenant it names. |
| device id key | Compute the device id of any machine id. |
| a node's own key | Act as that node until its installation is revoked. The key is in the node's persistent kvs store, readable by anyone who can run `kvs get` on the node. |

## Server names

| Name | Who asks for it | Served by |
|------|-----------------|-----------|
| `fleet.<domain>` | Nodes, for the fleet link | nightfall's fleet certificate (fleet-server CA) |
| `provision.<domain>` | Nodes, to enroll and renew | nightfall's provisioning certificate (fleet-server CA) |
| `<namespace id>.<suffix>` | dawn, to reach one node | nightfall's inner certificate, `*.<suffix>` (internal CA) |

nightfall reads the server name a client sends before any TLS work and picks
the configuration by it; a name it does not serve gets the TLS
`unrecognized_name` alert. `<suffix>` is nightfall's `[inner]
server_name_suffix`.
