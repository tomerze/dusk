# Connect a node to nightfall

This page is for node developers: you build the `dusk_node` artifact, and you
want every copy of it you ship to dial out to a nightfall server instead of
waiting for clients to connect to it - a node behind NAT, or on a network that
lets nothing in. It covers the build, the options, what the node keeps, what it
does the first time it starts, how it stays connected, how it renews its
certificate and how it keeps its key in a TPM.

A node that dials out runs the [`nightfall`](../concepts/base.md#nightfall)
program in connect mode instead of listen mode, and need not listen at all.
`nightfall -c` connects to nightfall over TLS 1.3, proves who it is with a client
certificate, and serves the node's `Dusk` capability over that connection, so
the server on the other end drives the node exactly as a
[client](connect-a-client.md) would.

## Build a node that dials out

Three settings are compiled into the node, all while it is built:

- the init script, [`DUSK_NODE_INIT_SCRIPT`](../../embedding/node-artifacts.md#dusk_node_init_script),
  set to a `nightfall -c` command;
- the node's persistent kvs file,
  [`DUSK_NODE_KVS_PERSISTENT`](../../embedding/node-artifacts.md#dusk_node_kvs_persistent),
  where the node keeps its identity;
- the [fleet token](../../embedding/node-artifacts.md#the-fleet-token),
  `DUSK_FLEET_TOKEN`, which the node enrolls with the first time it starts.

From the repository root, for the Linux executable:

```sh
DUSK_NODE_INIT_SCRIPT="nightfall -c fleet.example.com:443 --provision provision.example.com:443 --ca /etc/dusk/fleet-server-ca.pem" \
DUSK_NODE_KVS_PERSISTENT=/var/lib/dusk/kvs \
DUSK_FLEET_TOKEN="$(cat fleet-token)" \
  cargo build --profile prod -p dusk_node_bin
```

The binary is `target/prod/dusk_node`. With CMake, pass the first two as
`-DDUSK_NODE_INIT_SCRIPT="nightfall -c ..."` and
`-DDUSK_NODE_KVS_PERSISTENT=/var/lib/dusk/kvs` to `cmake --preset`, and set
`DUSK_FLEET_TOKEN` in the environment of `cmake --build`.

Then, on every machine:

| Path | What it holds | Who provides it |
|------|---------------|-----------------|
| `/etc/dusk/fleet-server-ca.pem` | The certificates of the CAs that sign nightfall's server certificates, PEM. Several `CERTIFICATE` blocks are allowed, for a rotation. | Whoever runs nightfall |
| `/var/lib/dusk/` | The directory of the persistent kvs file. It must exist and be writable by the account the node runs as; the node creates the file. | You |

A node built without a persistent kvs file cannot dial out: `nightfall -c`
stops at once, with
`nightfall can't link to the fleet: this node keeps no persistent kvs keys to hold its identity`
in the node's logs. So does a node whose file does not open - a file another
device or another build wrote, see [The file](../../features/kvs.md#the-file).

A node that dials out needs a correct wall clock: it checks nightfall's
certificates against it and times its renewals by it. A machine that starts with
its clock far in the past - a board without a battery-backed clock that boots at
1970 - can neither enroll nor connect until something sets its clock.

### Install tokens

Whoever runs nightfall can issue install tokens instead of relying on the fleet
token: a token for one installation, accepted for one enrollment. To enroll with
one, add `--install-token-file /etc/dusk/install-token` to the `nightfall -c`
command and install a different token at that path on every machine. The node
enrolls with it instead of the fleet token. A node that has to enroll again -
its persistent kvs file deleted, or its certificate expired beyond nightfall's
renew grace - needs a new token in that file; the node reads the file again for
every attempt, so replacing it is enough. There is no option that takes the
token itself: the init script is compiled into a binary every installation
shares.

## The options

```
nightfall -c <host>:<port> --provision <host>:<port> --ca <file>
          [--server-name <name>] [--provision-server-name <name>]
          [--install-token-file <file>] [--heartbeat-timeout <seconds>]
          [--tpm <path> | --no-tpm]
```

| Option | Meaning |
|--------|---------|
| `-c`, `--connect <host>:<port>` | Where nightfall's node listener is. `<host>` is a host name, an IPv4 address, or an IPv6 address in brackets, e.g. `[2001:db8::1]:443`. |
| `--server-name <name>` | The TLS server name the node asks for and checks nightfall's certificate against. Defaults to the host of `-c`. |
| `--provision <host>:<port>` | Where nightfall's provisioning listener is: where the node enrolls and renews its certificate. |
| `--provision-server-name <name>` | The same as `--server-name`, for `--provision`. |
| `--ca <file>` | The certificates of the CAs that sign nightfall's server certificates, PEM, at most 1 MiB. The node trusts no other CA, not even the system's. It reads the file again for every connection, so a replaced file takes effect without a restart. |
| `--install-token-file <file>` | A file holding an install token, to enroll with instead of the fleet token. The node reads at most 64 KiB of it, which must be UTF-8, and ignores the whitespace around the token. |
| `--heartbeat-timeout <seconds>` | How long the connection may stay silent before the node closes it: 90 by default, from 1 to 86400. |
| `--tpm <path>` | The TPM to keep the node key in: `/dev/tpmrm0` by default. A path that does not exist means the machine has no TPM. The path can also be the Unix socket of a software TPM, such as swtpm's. See [The TPM](#the-tpm). |
| `--no-tpm` | Keep the node key in the kvs even on a machine with a TPM. |

`-l` (listen) and `-c` (connect) cannot be combined: one `nightfall` process
does one thing. A node can run a `nightfall -l` beside its `nightfall -c` -
`sh -d "nightfall -l 127.0.0.1:9090"; nightfall -c ...` - when it should take
local clients too. `ps` shows a connecting process as
`nightfall[connect fleet.example.com:443]`.

## What the node keeps

The node keeps its identity in [persistent](../../features/kvs.md#persistent-keys),
[sticky](../../features/kvs.md#sticky-keys) kvs keys under `nightfall.`, so
`kvs get nightfall` reads them:

| Key | Value |
|-----|-------|
| `nightfall.installation_id` | The installation id nightfall assigned when the node enrolled: 32 lowercase hex digits. |
| `nightfall.private_key` | The node's private key: ECDSA P-256, PKCS#8 DER - or, for a key kept in a TPM, a list of the key's TPM private and public areas (see [The TPM](#the-tpm)). [Sensitive](../../features/kvs.md#sensitive-keys). |
| `nightfall.certificate_chain` | A list of the certificates nightfall issued for that key, DER, the node's own certificate first. |
| `nightfall.staged_private_key` | A new key between its creation and the moment its certificate is stored. Sensitive. |
| `nightfall.hardware_fingerprint` | Only on a machine that reports no machine id: 32 random bytes standing in for one (see [First start](#first-start)). |

Sensitive keeps the private key out of the node's logs, not from a client:
every client the node serves can read it with `kvs get`, as it can read the
fleet token with `Dusk.fleetToken`.

A key that holds something the node cannot use - a value of the wrong kind, a
key that is not a PKCS#8 ECDSA P-256 key, a TPM key the TPM does not load, a
certificate that is not one - is logged at `error` and treated as missing. A
missing or unusable identity makes the node enroll again, and an enrollment
always creates a new installation - so deleting the persistent kvs file is how a
machine is reinstalled as far as nightfall is concerned. Its device id stays the
same: that comes from the machine, not from the file.

Every node keeps its own persistent kvs file, so every node has its own
identity: two nodes on one device, or in one process, are two installations.

## First start

1. The node waits a random 0 to 5 seconds, so machines that boot together do not
   arrive together.
2. It finds no identity in the kvs, so it generates a key - in the TPM, on a
   machine with one (see [The TPM](#the-tpm)) - and keeps it in
   `nightfall.staged_private_key` - which also proves it can write the kvs
   before it asks nightfall for anything. It connects to `--provision` (TLS 1.2
   or 1.3) and calls `assign` with its fleet token, or its install token, and a
   report about the machine: its hardware fingerprint, hostname, Dusk version,
   impl and target, and the installation id of an earlier enrollment if
   `nightfall.installation_id` still holds one. The hardware fingerprint is the
   SHA-256 of the device's id, the
   [`dusk.device.id`](../../features/kvs.md#what-the-impl-records) the impl
   records. A machine without one (no device id at all, or one that is all zeros
   or `uninitialized`) gets 32 random bytes, kept in
   `nightfall.hardware_fingerprint`; such a machine gets a new device id when it
   is reinstalled, and the node says so in its logs at `warn`.
   A node that attests its key with a TPM reports instead the SHA-256 of the
   TPM's endorsement key public area, the `TPMT_PUBLIC` it sends as
   `TpmAttestation.endorsementKey` (see [The TPM](#the-tpm)).
3. nightfall answers with the device id it derived, a new installation id and a
   one-time challenge.
4. The node calls `enroll` with a certificate request for its new key: an empty
   subject, and exactly two subject alternative names,
   `urn:dusk:device:<device id>` and `urn:dusk:installation:<installation id>`.
5. nightfall returns the signed chain. The node checks that the certificate is
   for the ids it was assigned and for its new key, then stores the chain, the
   installation id and the key, and removes the staged key.
6. The node connects to `-c` with TLS 1.3, presenting that certificate, and
   serves its `Dusk` capability over the connection.

If provisioning fails, the node tries again after a random wait that grows from
up to 1 second to up to 10 minutes. If nightfall refuses - a wrong token, or a
revoked device - the node waits between 5 and 10 minutes before every new
attempt and logs the refusal at `error`.

Every later start finds the identity in the kvs: after the random wait of step 1
it goes straight to step 6.

The schema of the provisioning calls is `provision.capnp`, in the `nightfall`
program's crate, `dusk_program_nightfall::provision_capnp` in Rust.

## Staying connected

Each connection attempt has 10 seconds to resolve the host name, 10 seconds to
connect to each address it resolves to, and 10 more for the TLS handshake. The
socket uses TCP keepalive (first probe after 60 seconds idle, then every 10
seconds, 3 probes) and, on Linux and Android, a 60 second `TCP_USER_TIMEOUT`, so
a connection through a dead network path is noticed even when nothing is being
sent. An option the platform refuses is logged at `warn`, and the connection
goes on without it.

nightfall is expected to call the node regularly - every 30 seconds or so. If
nothing at all arrives for `--heartbeat-timeout` seconds, the node closes the
connection, waits up to 10 seconds for the session on it to end, and only then
connects again. A connection can also end from the other side - nightfall
restarting, or closing it at the certificate's expiry.

After a connection ends, the node waits a random time before the next attempt,
up to 1 second at first and doubling up to 5 minutes. A connection on which
nightfall was still sending more than a minute after it came up starts that
count again from 1 second; one that only stayed open, with nothing arriving,
does not.

## Renewal

For every certificate it is issued, the node draws a renewal point at random
between 55% and 75% of the certificate's lifetime - between 92.4 and 126 hours
after it was issued, for a 168 hour certificate - so machines enrolled on the
same day do not renew in the same minute.

At that point, while connected, the node generates a new key, keeps it in
`nightfall.staged_private_key`, and calls `renew` on `--provision`, presenting its
current certificate as its TLS client certificate. When the new certificate
comes back the node stores it with the new key and connects again with them.
Until the new certificate is stored the old key stays in
`nightfall.private_key`, and a renewal cut short by a crash between storing the
certificate and storing its key is finished from the staged key at the next
start, so the node always has a working identity. A certificate already past its
renewal point when the node starts, or when it is about to connect again, is
renewed before connecting.
A certificate issued to a key nightfall attested is renewed with that same key;
see [The TPM](#the-tpm).

A failed renewal is retried after a random wait growing up to 10 minutes, and
the node stays connected while it retries.

A node that was off long enough for its certificate to expire renews it the same
way when it comes back, if nightfall still accepts the expired certificate. If
nightfall refuses because the certificate expired too long ago, the node enrolls
again with its token, as at its first start: the fleet token gives it a new
installation, and an install token that was already used is refused until a new
one is in its file.

## The TPM

On a machine with a TPM 2.0, the node creates its key in the TPM, where it
stays: the TPM signs the node's certificate requests and every TLS handshake,
and nothing outside the TPM ever holds the private key, so a copy of the node's
persistent kvs file is no copy of its identity.
When it enrolls, the node also proves to nightfall that the key is in a genuine
TPM. A nightfall that refuses nodes that do not prove it lets a fleet token
taken from one machine enroll only as many nodes as whoever took it has TPMs.

The node uses the TPM at `--tpm`, `/dev/tpmrm0` by default - the Linux kernel's
TPM resource manager. The account the node runs as must be able to read and
write it: on most distributions, root or a member of the `tss` group.
`--no-tpm` keeps the key in the kvs even on a machine with a TPM.

The key is an ECDSA P-256 signing key that the TPM generates under its storage
key and will not export. `nightfall.private_key` then holds a list of two byte
strings: the key's TPM private area, which only that TPM can load, and its
public area. The node loads it into the TPM again every time it starts.

A TPM the node cannot use - one it cannot open, or one whose owner hierarchy has
a password - leaves the key in the kvs, as on a machine without a TPM, and the
node logs why at `warn`.

**Enrolling.** The node reads the TPM's RSA endorsement key certificate from NV
index `0x01C00002`, and its issuers' certificates from NV indices `0x01C00100`
onward when the TPM keeps any, and recreates the TPM's RSA 2048 endorsement key
from the TCG template. `assign` carries them in the device report's `tpm` field
with the node key's public area, and the hardware fingerprint is the SHA-256 of
the endorsement key's public area, its `TPMT_PUBLIC` as the report carries it,
instead of the device's id, so the device id follows the TPM. nightfall answers with a credential that only that TPM can decrypt, and
only while it holds that node key; the node has the TPM decrypt it, and sends
the result to `enroll` with a certificate request the TPM signed. A nightfall
that asks for no attestation answers with a plain challenge, and gets a node key
kept in the TPM all the same.

**What nightfall needs.** nightfall checks the endorsement key certificate
against the certificate authorities of TPM manufacturers, and Dusk ships none:
whoever runs nightfall gives it a bundle of the manufacturers' root
certificates, with the intermediates their TPMs do not keep. Without a bundle,
nightfall refuses every node that attests; with one, it refuses a TPM whose
certificate does not chain to it. To see which manufacturer's authority a
machine's TPM needs, read its certificate's issuer with tpm2-tools and OpenSSL:

```sh
tpm2_nvread 0x1c00002 -o ek.der
openssl x509 -inform der -in ek.der -noout -issuer
```

**A TPM without a certificate.** nightfall refuses an endorsement key that has no
certificate: such a key proves nothing about the TPM it is in. Some firmware
TPMs serve their certificate from the manufacturer's web service instead of
keeping it in NV, and many virtual machines' TPMs have none. The node does not
offer nightfall such a key: it keeps its key in the TPM all the same, enrolls
with its token alone, as with a nightfall that asks for no attestation, and logs
at `warn` that it did not attest. So it does with a TPM whose endorsement
hierarchy has a password.

**Renewing.** A key kept in the TPM is renewed with a new key, created in the
same TPM.
A certificate nightfall issued to an attested key, though, carries a third
subject alternative name, `urn:dusk:attestation:tpm`, and is renewed with the
same key: the key never leaves the TPM, so there is nothing to replace.

**A key that does not load.** When the stored TPM key does not load as the node
starts, the node logs that at `error` and enrolls again, as a new installation -
whatever the reason: the TPM was cleared, which changes its storage key; the
persistent kvs file came from another machine; the TPM did not open on that
start; or the node now runs with `--no-tpm`. When no TPM was opened, the new
installation's key is a software key.
A node that attests keeps its device id through a cleared TPM: clearing a TPM
leaves its endorsement key as it was.

## Several `nightfall -c` on one node

Several `nightfall -c` processes on one node share its identity: they read and
store the same kvs keys. Only one of them enrolls or renews at a time - and only
one in the whole process, across every node it runs - so the first to start
enrolls and the rest find its identity. One that reaches its renewal point and
finds that another has already renewed the certificate connects again with that
certificate instead of renewing it a second time.
