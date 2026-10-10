# Nodes that dial out

This page is for node developers: you build the `dusk_node` artifact, and you
want every copy of it you ship to join your fleet on its own. It covers the init
script that makes a node dial out to the Dusk stack, the fleet token compiled
into it, where the node keeps its identity, what it does the first time it
starts and every time its certificate is renewed, and how it stays linked.

A node that dials out need not listen. Its init script runs the
[`nightfall`](../getting-started/concepts/base.md#nightfall) program in connect
mode instead of listen mode: `nightfall -c` dials out to the stack's nightfall
over TLS 1.3, proves who the node is with a client certificate, and serves the
node's `Dusk` capability over that connection - the node is the RPC server of
the link. Everything the stack asks of the node travels down that one link.

**None of the node's side is merged yet.** This page describes it as the
node's pull requests build it, which are in review on `master`: connect mode,
`nightfall -c` (pull request #174), the fleet token compiled into `dusk_core`
and `Dusk.fleetToken` (issue #143), sensitive kvs keys (issue #144) and
persistent kvs keys (issue #93). Until they are merged into the node the stack
ships with, the `nightfall` program only listens, and a node cannot dial out.

## Build a node that dials out

Three things are compiled into the node while it is built: the init script,
from
[`DUSK_NODE_INIT_SCRIPT`](../embedding/node-artifacts.md#dusk_node_init_script),
the fleet token, from `DUSK_FLEET_TOKEN`, and the file of its persistent kvs
keys, from `DUSK_NODE_KVS_PERSISTENT`. From the repository root, for the Linux
executable:

```sh
DUSK_FLEET_TOKEN="$(cat fleet-token)" \
DUSK_NODE_KVS_PERSISTENT=/var/lib/dusk/kvs \
DUSK_NODE_INIT_SCRIPT="nightfall -c fleet.example.com:443 --provision provision.example.com:443 --ca /etc/dusk/fleet-server-ca.pem" \
  cargo build --profile prod -p dusk_node_bin
```

The binary is `target/prod/dusk_node`. With CMake, pass the same script as
`-DDUSK_NODE_INIT_SCRIPT="nightfall -c ..."` and the file as
`-DDUSK_NODE_KVS_PERSISTENT=/var/lib/dusk/kvs` to `cmake --preset`, with
`DUSK_FLEET_TOKEN` in the environment.

* **The fleet token** is the shared secret nightfall checks when a node enrolls
  for the first time. `dusk_core` compiles in the value of `DUSK_FLEET_TOKEN`
  and never prints it. A build without the variable gets a random token of its
  own, kept until the variable changes or the build is cleaned - a node built
  that way can enroll only with an install token (below). Take the token from
  whoever runs the stack; in a container build pass it as a build secret, never
  as a build argument, so no image layer holds it (see
  [Deploying the Dusk stack](deploy.md#node-images)).
* **The fleet-server CA** is the one file every machine needs next to the
  binary, at the path `--ca` names: the certificates of the CA that signs
  nightfall's server certificates, PEM. Several `CERTIFICATE` blocks are
  allowed, for a rotation.
* **A persistent key-value store.** The node keeps its identity in persistent
  [kvs](../features/kvs.md) keys, which live in an encrypted file: the one
  `DUSK_NODE_KVS_PERSISTENT` names, which the node artifact passes to the kvs
  launcher. `nightfall -c` refuses to start on a node whose kvs has no
  persistent store. The file must outlive restarts and
  upgrades: losing it makes the node a new installation. The node image in
  `infra/node/Dockerfile` keeps it on a volume under `/var/lib/dusk`.

A node that dials out needs a correct wall clock: it checks nightfall's
certificates against it and times its own renewals by it. A machine that starts
with its clock far in the past - a board without a battery-backed clock that
boots at 1970 - can neither enroll nor link until something sets its clock.

### The fleet token caveat

The fleet token is in every binary built with it. **Whoever extracts it from one
copy can enroll as many nodes as they like**, and limiting enrollments by
address does not stop an enrollment spread over many addresses. The fleet token
is the first gate, not the last:

* nightfall limits the rate of enrollments, per instance and per credential,
  and alerts on the enrollment rate ([Provisioning](provisioning.md#limits));
* every enrollment is a new installation that shows in the inventory, and
  revoking it, or every installation of its device, is instant;
* per-install tokens enroll through the same certificate request path, and a
  node whose key is in a TPM attests it on top of its token. nightfall still
  enrolls a node that does not attest, so attestation marks the nodes that have
  a genuine TPM, with `urn:dusk:attestation:tpm`, rather than stopping a copied
  token ([TPM attestation](provisioning.md#tpm-attestation)).

Use one token per product or batch, so one can be retired without the others.
Retiring a token stops new enrollments only: nodes that already hold
certificates keep renewing them, so revoke those that should not. No role in nightfall's
shipped permissions allows `Dusk.fleetToken`.

### Install tokens

A stack can issue install tokens instead: each is good for one enrollment only.
Name its file with `--install-token-file` in the init script - e.g.
`--install-token-file /etc/dusk/install-token` - and install a different token
at that path on every machine. The node enrolls with it exactly as with the
fleet token: nightfall gives it a new installation id, and records the token's
subject with the enrollment. A node that has to enroll again needs a new token
in that file. There is no option
that takes an install token itself: the init script is in a binary every
installation shares.

## The connect options

```
nightfall -c <host>:<port> --provision <host>:<port> --ca <file>
          [--server-name <name>] [--provision-server-name <name>]
          [--install-token-file <file>] [--heartbeat-timeout <seconds>]
```

| Option | Meaning |
|--------|---------|
| `-c`, `--connect <host>:<port>` | Where nightfall's fleet listener is. |
| `--server-name <name>` | The TLS server name the node asks for and checks nightfall's certificate against. Defaults to the host of `-c`. |
| `--provision <host>:<port>` | Where nightfall's provisioning listener is: where the node enrolls and renews its certificate. |
| `--provision-server-name <name>` | The same as `--server-name`, for `--provision`. |
| `--ca <file>` | The fleet-server CA certificates, PEM. The node trusts no other CA, not even the system's. |
| `--install-token-file <file>` | The file holding an install token, to enroll with instead of the fleet token. |
| `--heartbeat-timeout <seconds>` | How long the link may stay silent before the node closes it: 90 by default. |

One `nightfall` process does one thing: `-l` (listen) and `-c` (connect) are
separate processes, and a node that should listen on several ports runs several
`nightfall -l`. `ps` on a node that dials out shows the process as
`nightfall[connect fleet.example.com:443]`.

## Where the node keeps its identity

Everything the node needs to link again after a restart is in persistent kvs
keys: the installation id nightfall assigned, the node's private key (ECDSA
P-256, PKCS#8) and the certificate chain nightfall issued for it. The keys are
sticky, so a plain `kvs set` refuses to overwrite them, and the key and
certificate are sensitive, so their values never reach a log or a trace -
`kvs get` still returns them, to anyone who can run it on the node.

The persistent store is encrypted with XChaCha20-Poly1305 under a key derived
from the fleet token and the machine's `dusk.device.id`. That keeps the identity
from someone who copies the file alone, not from someone who also holds the
binary and can read the machine id.

A node with no identity in its kvs enrolls. Every enrollment creates a new
installation, so wiping the persistent store is how a machine is
reinstalled as far as the stack is concerned. Its device id stays the same: that
comes from the machine, not from the store.

## First start

1. The node waits a random 0 to 5 seconds, so a fleet that boots together does
   not arrive together.
2. It finds no identity in its kvs, so it connects to `--provision` (TLS 1.2 or
   1.3) and calls `assign` with its credential - the fleet token, or the
   install token - and a report about the machine: its hardware fingerprint,
   the installation id of a previous enrollment when it still has one,
   hostname, Dusk version, impl and target. The hardware fingerprint is the
   SHA-256 of the machine id the node reads as `dusk.device.id`. A machine
   without one (no machine id at all, or an empty one, all zeros, or
   `uninitialized`) sends 32 random bytes instead, kept in a persistent kvs key;
   such a machine gets a new device id when it is reinstalled.
3. nightfall answers with the device id it derived, a new installation id and a
   one-time challenge that expires after five minutes.
4. The node generates its key and sends `enroll` the credential, the report,
   the challenge and a certificate request: an empty subject, and exactly two
   subject alternative names, `urn:dusk:device:<device id>` and
   `urn:dusk:installation:<installation id>`.
5. nightfall has step-ca sign it and returns the chain. The node stores the
   installation id, its key and the chain in its kvs.
6. The node connects to `-c` with TLS 1.3, presenting that certificate, and
   serves its `Dusk` capability over the connection.

If provisioning fails, the node tries again after a random wait that grows from
up to 1 second to up to 10 minutes. If nightfall refuses - a wrong token, or a
revoked device - the node waits the full 10 minutes before every new attempt and
logs the refusal at `error`.

Every later start finds the identity in the kvs: after the random wait of
step 1 it goes straight to step 6.

## Staying linked

Each connection attempt has 10 seconds to connect and 10 more for the TLS
handshake. The node verifies nightfall's certificate against `--ca`, under the
server name `--server-name` or the host of `-c`.

nightfall calls `Dusk.time` on every node about every 30 seconds. If nothing at
all arrives for `--heartbeat-timeout` seconds, the node closes the link, ends
the session that ran on it, and connects again. A link can also end from the
other side - nightfall restarting, draining, or closing it at the certificate's
expiry.

After a link ends, the node waits a random time before the next attempt, up to
1 second at first and doubling up to 5 minutes. A link that lived longer than a
minute starts that count again from 1 second.

## Renewal

nightfall's certificates are short-lived: 168 hours with its default
configuration. Every certificate comes with a renewal point drawn at random
between 55% and 75% of its lifetime, anew for each certificate, so a fleet
enrolled on the same day does not renew in the same minute.

At that point the node generates a new key, or keeps its key when the
certificate carries `urn:dusk:attestation:tpm`, and calls `renew` on
`--provision`, presenting its current certificate as its TLS client certificate.
When the new certificate comes back the node stores it with that key in its kvs
and links with them from then on.

A node that was off long enough for its certificate to expire renews it the same
way when it comes back: nightfall accepts an expired certificate for renewal for
a grace period after it expired, 90 days by default. Beyond that nightfall
refuses with an error naming `renewBeyondGrace`, and the node enrolls again with
its credential, as at its first start: it gets a new installation id, and an
install token that was already used is refused until a new one is in its file.

nightfall refuses assign, enroll and renew for a device or an installation that
is revoked or retired.

## Several namespaces in one process

Several Dusk nodes can run in one process: `dusk_spawn` starts one each time it
is called, each a namespace with a namespace id of its own. Namespaces that share
one persistent store share its identity: they are one installation, and each
links to the stack as a session of its own, `(device id, installation id,
namespace id)`. Nodes that should be separate installations each need a
persistent store of their own.

## Where the private key lives

On a Linux machine with a TPM 2.0 the node creates its key in the TPM, and the
key never leaves it: the persistent store keeps only the key's TPM private and
public areas, which only that TPM can load (see
[The TPM](../getting-started/guides/connect-to-nightfall.md#the-tpm)). Without a
TPM the key is in the encrypted persistent store described above. That is the
last resort, not the plan: the other key stores that keep a key in hardware -
the Secure Enclave, the platform's key storage, the Android Keystore - are not
yet available. A node whose TPM has an endorsement key certificate attests its
key when it enrolls (see [TPM attestation](provisioning.md#tpm-attestation)).
