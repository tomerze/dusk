# The security model

This page is for security reviewers and for the people who run the Dusk stack.
It says what the stack protects, which checks stand between an operator's
request and a node, how nodes are admitted and cut off, what the ledger proves,
how reconcile holds the ledger against the processes twilight intended, what an
attacker who holds one component can and cannot do, and what no part of the
stack can detect. It describes the services as they are built in this
repository and the shipped Kubernetes overlays; the identifiers and certificate
authorities it refers to are on [Identity and trust](identity.md).

## What is protected

* **Control of the nodes.** Anything that can call `ShPortal.sh` or
  `Dusk.process` on a node can run any program the node has, as the account
  the node runs as. Every other protection serves to decide who reaches that
  call, on which node, and to make a record of it.
* **Who is in the fleet.** A node is admitted only with a credential, gets its
  identity from nightfall rather than choosing it, and can be cut off at once.
* **The record of what was done.** Every call that crosses nightfall is written
  to a hash-chained, signed ledger before or as it happens, and copied to an
  object-locked bucket.

## The path of a process

Work on a node is a process at a pid ([Processes](processes.md)). Every command
to a node passes these checks, in this order. A check that fails stops the
command before it reaches the next.

| Where | Check |
|-------|-------|
| twilight | The caller is authenticated (OpenID Connect, an API token, or a client certificate from the internal CA) and has the role the route needs: `operator` to run campaigns, open interactive sessions, stream logs, collect files or set a node `active` or `quarantined`; `admin` to set a node `retired` or `revoked`, to lift either, and to revoke or retire a whole device. |
| twilight | It records the pid in `intended_processes` - the node, the campaign and attempt, the kind of work, who asked, until when, how many shell commands - and commits it before it calls dawn. |
| dawn | The caller is authenticated and its role allows the route: twilight is a `dispatcher`, people are `operator`s. dawn never takes identity from a proxy header. The nightfall instance the request names must be one dawn's configuration allows. |
| nightfall | dawn's TLS certificate chains to the internal CA, carries exactly one principal and no node identity, is not in `deny_certificates`, and its principal is not denied and has a role in `permissions.toml`. The server name `<namespace id>.<suffix>` reaches the node whose certificate claimed that namespace id. |
| nightfall | The [membrane](membrane.md) allows only the calls the principal's roles allow, applies the rate, in-flight and capability limits, and writes the call to the ledger with the pid it works under. A command that does not stream is forwarded only once its ledger entry is committed in Kafka; if the ledger cannot take it within 5 seconds, the command fails and the node never sees it. |
| dawn, node | dawn looks for the pid in the node's `ps` and runs nothing when a process is there; otherwise the work runs in a shell server of its own at that pid. |

Three consequences reviewers should keep in mind:

* **A role that allows `ShPortal.sh` or `Dusk.process` has full control of the
  node, whatever else it denies.** A script is one call; the programs it starts
  run inside the node and never cross nightfall. The shipped `dawn` role allows
  both, because work on a node is a shell script.
* **Nothing ties a call to an intended process before it reaches the node.**
  There is no per-process credential: a principal with the `dawn` role can
  reach every node connected to nightfall, at any pid. The intended processes
  are checked after the fact, by reconcile, against a ledger written before
  each command is forwarded. The ledger is complete; the alerts are only as
  complete as the rules below.
* **A command given to dawn runs partly inside dawn.** A campaign's script, a
  command sent to dawn's `/v1/sh` and a call to its MCP tools are dusk shell
  commands, and the client side of a command - the part that builds its
  arguments, the local end of a `cp` - runs in dawn's process. It can read
  every file dawn can read: dawn's keys and certificates, its Kafka and object
  store credentials, its output key. **Whoever writes campaign scripts or uses
  dawn's shell holds dawn's credentials.** What limits where they can send them
  is dawn's NetworkPolicy: in the shipped overlays, nightfall's inner listener,
  Kafka, the object store, the OTel collector and DNS.

## Enrollment

### The fleet token

The fleet token compiled into the node, `Dusk.fleetToken` and the node's
persistent kvs store are the node's side as its pull requests build it (issues
#143 and #93), not merged yet.

A fleet token is a shared secret compiled into every node built with it.
**Whoever extracts it from one binary can enroll as many nodes as they like** -
each enrollment creates a new installation, and the device id comes from a
hardware fingerprint the enrolling node chooses - until the token is retired.
Limiting by address does not stop that: an enrollment spread over many
addresses passes every per-address limit. Nodes enrolled that way are ordinary
installations to the rest of the stack: they appear in the inventory, join
campaigns their reported facts match, and the results they report count in
those campaigns' health gates.

What bounds and reveals it:

* Every assign and enroll takes a token from a bucket for the nightfall
  instance (50 per second by default) and one for the credential (10 per
  second); beyond either the node is told `overloaded`.
* A failed credential counts against its address; more than 20 failures in an
  hour put the address in the penalty box.
* Above 600 assign and enroll calls a minute nightfall logs a warning and sets
  `nightfall_enrollment_rate_alert`, and twilight raises a high
  `enrollment_rate` alert when `dusk.enrollments` exceeds its threshold.
* Each nightfall instance warns and counts `device_id_collision` when one
  device id enrolls more than 20 installations through it from more than 5
  addresses within 24 hours. It counts in memory, so enrollments spread over
  instances or across a restart are not counted together.
* Every outcome is a message on `dusk.enrollments`, naming a fleet token by
  its entry name and an install token by its subject, never by its value.

The token is in the binary because the node needs it, so treat every binary as
holding it: pass it to container builds as a build secret, so no image layer
holds it, and use one token per product or batch so one can be retired without
the others. Retiring a token (removing its entry and restarting nightfall) stops
new enrollments only: nodes that already hold certificates keep renewing them,
so revoke those that should not. No role in nightfall's shipped permissions
allows `Dusk.fleetToken`.

The fleet token also keys the encryption of each node's persistent kvs store,
together with the machine's `dusk.device.id`. Whoever holds the binary and can
read a machine's id can decrypt that machine's store, the node's private key
included.

### Install tokens

An install token is a JSON Web Token, signed with a key nightfall knows only the
public half of. An enrollment with one gets a new installation id, as one with
a fleet token does, and the token's subject (`sub`) is recorded as the
enrollment's `credential_ref`: no credential can get a certificate for an
installation that already exists. It can be used once across the whole fleet:
nightfall derives step-ca's one-time token id from the install token's issuer
and id, and step-ca refuses an id it has accepted before. That holds only while step-ca keeps its database on persistent storage
and runs as one replica. A copied install token is still good for one
enrollment by whoever presents it first. Prefer install tokens wherever the
installer can carry one per machine.

### TPM attestation

A node whose key lives in a TPM 2.0 proves it when it enrolls, on top of its
token: nightfall checks the TPM's endorsement key certificate against the TPM
manufacturer roots the operator configures, and only that TPM, holding that key,
can answer the challenge. The certificate then carries
`urn:dusk:attestation:tpm` and is renewed with the same key. nightfall still
enrolls nodes that do not attest, so a copied fleet token still enrolls nodes
without a TPM; the marker tells them apart. See
[TPM attestation](provisioning.md#tpm-attestation).

### The certificate authority stays behind nightfall

nightfall checks the credential, the node's lifecycle state and the certificate
request, then asks step-ca to sign it with a one-time token it mints itself.
step-ca holds the signing key and admits only nightfall's provisioner; its
template refuses any request that is not exactly one device and one
installation URI; its renewal endpoint is disabled, so every renewal passes
through nightfall's checks. See [Provisioning nodes](provisioning.md).

## Revocation and quarantine

A node's lifecycle - `enrolled`, `active`, `quarantined`, `retired`,
`revoked` - is set in twilight and published as a record on the compacted
topic `dusk.node-state`, one per installation. An admin can also revoke or
retire a whole device (`POST /api/v1/devices/{device}/lifecycle`): that record
blocks every installation of the device, those enrolled after it included, and
setting the device `active` again clears it. **Revocation is a status flip plus
a dropped connection. There are no certificate revocation lists and no OCSP**:
nightfall is the only component that accepts a node certificate, and it checks
the node's state itself.

When an installation or its device is `revoked` or `retired`:

* nightfall closes its live sessions within a second of reading the record,
  with the reason `revoked`;
* nightfall refuses its new sessions after the handshake, writing a
  `session_refused` event to the ledger;
* nightfall refuses its assign, enroll and renew calls, so it cannot get a new
  certificate either;
* a nightfall instance that starts is not ready until it has read
  `dusk.node-state` to the end, so a restart never lets a revoked node in;
* twilight raises a critical `revocation_not_enforced` alert when a node it has
  revoked or retired still appears online two census intervals after the
  change.

The certificate itself stays valid until it expires, at most 168 hours later,
and is useless meanwhile. Lifting a revocation is the same flip back to
`active`.

**Revoking an installation does not keep the machine out.** A machine that
holds a valid credential - the fleet token in its binary included - enrolls
again as a new installation and gets a certificate for it. Revoking its device
stops that for that device id, but the device id comes from a fingerprint the
enrolling node chooses, so whoever holds the credential can enroll as another
device. Retiring the credential is the remedy.

A nightfall instance that is stopped drains: it stops accepting node links and
enrollments at once, keeps its inner and relay listeners open for the sessions
it still holds, so dawn reaches those nodes until each one closes, closes them
in random order over `drain_seconds` with the reason `shutdown`, and keeps
listing them in its census until the last one has closed
([Draining](nightfall.md#draining)).

**Quarantine** keeps the node connected and drops every client's access to it:
when nightfall reads the flip it drops every client membrane on the node's
sessions, and from then on a client's calls are limited to what both its own
roles and the role named `quarantine` allow. The shipped `quarantine` role
allows only `Dusk.hostname`, `Dusk.programs`, `Dusk.namespaceId` and
`Dusk.time` - no process can be created or run on a quarantined node. A role
with `quarantine_override = true` keeps its own grants, and each of its calls
on a quarantined node writes a `quarantine_override` event that twilight raises
as a high alert.

## The membrane

Between dawn and a node, nightfall holds every capability: dawn never holds one
of the node's own, and the node never holds one of dawn's. Every capability
that crosses is wrapped, in both directions and at any depth, so every later
call on it is checked and recorded too; a call the principal's roles do not
allow is answered `unimplemented` and never reaches the node; the node's calls
back into dawn are limited to the role's `reverse_allow`. Dropping a membrane -
an admin kill, the node session closing, a lifecycle change, a permissions
reload - kills every capability derived from it at once. Every Cap'n Proto
connection nightfall speaks passes an allow-list of level-one RPC messages, so
a future capnp-rpc that could hand a capability to a third party fails loudly
instead of letting it bypass the membrane. The details are on
[the membrane](membrane.md).

## The ledger

nightfall writes an entry for every call it forwards, every result, every call
it refuses, its own setup calls on a node, and every session event, to the
Kafka topic `dusk.ledger`, one partition per instance, in transactions. Each
call carries its principal, its node and the pid of the process it works under.
Entries hold field names, a keyed hash of the parameters and result codes -
never a parameter value, a result or command output.

* **Hash chain.** Each entry carries the SHA-256 of its own canonical JSON
  (RFC 8785) and the hash of the entry before it, so an edited, removed,
  reordered or inserted entry breaks the chain.
* **Signed checkpoints.** Every second (`checkpoint_interval_ms`), and at a
  clean shutdown, nightfall signs the chain's head with its Ed25519 ledger key.
  A chain rewritten from some entry on is caught at the first checkpoint after
  it, unless the rewriter holds that key.
* **Write-ahead.** A command is forwarded only after its entry is committed:
  when Kafka cannot take the ledger, nightfall forwards no commands.
* **Evidence copy.** Vector copies every committed ledger record into the
  bucket `dusk-ledger-evidence`, which is object-locked (COMPLIANCE retention,
  400 days, in the prod overlay; the development stacks use a 1-day GOVERNANCE
  retention) and written with credentials that may only add objects.
* **Continuous verification.** twilight's reconcile verifies every partition's
  chain and checkpoint signatures as entries arrive and stores the head it has
  reached, so a tail removed later shows. `nightfall verify-ledger` verifies any
  copy ([the ledger](ledger.md#verifying)).

## Reconcile

twilight's reconcile reads every call in the ledger and holds it against the
processes twilight intended. These are the most important alerts in the stack:
a process no campaign or operator asked for means a stolen client certificate,
someone driving nodes with the SDK directly, or a compromised dawn. What to do
about each is in [Campaigns](campaigns.md#reconcile-alerts).

| Alert | Severity | Raised when |
|-------|----------|-------------|
| `process_without_intent` | critical | A client created a process at a pid twilight never intended for that node. |
| `default_shell_without_intent` | critical | A shell command ran in a node's default shell while no process twilight intended for that node was open. |
| `target_mismatch` | critical | A call under a pid reached a node other than the one its intended process names. |
| `result_without_ledger` | critical | dawn reported a process started at a pid, and the ledger shows no `Dusk.process` for that pid within ten minutes. |
| `ledger_chain_broken` | critical | A chain or a checkpoint signature does not verify, a chain starts without naming the one before it, or an instance wrote for more than twice its checkpoint interval without signing. |
| `process_after_deadline` | high | A call under a pid arrived after its intended process expired; killing and reaping it excepted. |
| `pid_reused` | high | A process at one pid was created on more than one client session. |
| `process_after_result` | high | Calls arrived under a pid more than a minute after its final result. |
| `process_shape` | high | A process ran more shell commands in its own shell than it was intended to, or a node's default shell ran more than the budgets of the node's open intended processes allow together. |
| `quarantine_override` | high | A role allowed past quarantine sent a command to a quarantined node. |

A process a client creates without naming a pid is counted per principal
(`twilight_unattributed_processes_total`), not alerted: client-side argument
builders start helper processes that way. twilight also raises
`enrollment_rate` (high), `revocation_not_enforced` (critical) and
`campaign_conflict`. Every new critical and high alert is posted to
`alerts.webhook_url` when one is set.

## Compromised components

What an attacker who controls one component can and cannot do, in the prod
overlay. The local stack and the dev overlay are not hardened: Kafka there
runs without TLS or access control lists.

### A node

**Can:** act as that node, with its certificate, until its installation is
revoked; report anything about itself - facts, reported version and
configuration, its `ps`, its logs, the output and outcome of every script it
runs - so it can make work look succeeded, failed, already there (`duplicate`)
or still running, and make an `ensure_*` campaign look converged; send output
values back to dawn, which caps what it passes on per process; choose the
hardware fingerprint it enrolls with, and so the device id of a new
installation, if it holds an enrollment credential.

**Cannot:** reach any other node - nightfall serves nodes nothing, and never
connects one node to another; call into dawn except through the capabilities
dawn passed it, and only the calls the role's `reverse_allow` names; claim a
namespace id another live session holds; renew as another identity, since a
renewal must name exactly the identity of the certificate presented; get a
certificate for its installation or device once that is revoked - though
with an enrollment credential it can enroll again as a new installation, or,
with another fingerprint, as another device.

### dawn

**Can:** with its principal's `dawn` role, reach every node connected to
nightfall and make every call that role allows - which, through `ShPortal.sh`
and `Dusk.process`, means running anything on any of those nodes; read a node's
identity - its private key and certificate - with `kvs get` or a script, and
connect as that node until it is revoked; read everything that passes through
it: script output, collected files, streamed node logs, facts; write false
records to `dusk.process-results`, `dusk.process-output` and `dusk.files`, and
upload objects to the files bucket; tell twilight false facts.

**Cannot:** make calls the `dawn` role does not allow, `Dusk.settime` and
`Dusk.fleetToken` among them; make a call nightfall does not record, with its
principal, node and pid; write the ledger, `dusk.node-state` or the connection
topics; read or delete objects in the files bucket, whose policy gives dawn's
user only uploads.

**Detected:** a process at a pid twilight never intended
(`process_without_intent`), on another node than intended (`target_mismatch`),
reused (`pid_reused`), past its deadline or its result
(`process_after_deadline`, `process_after_result`), running more commands than
intended (`process_shape`), results for processes the ledger never saw
(`result_without_ledger`), a command in a node's default shell while none of
the node's intended processes is open (`default_shell_without_intent`) or
beyond their budgets (`process_shape`). **Not detected:** what it runs in a
node's default shell within those budgets; processes it creates without
naming a pid, which are only counted; different content run inside one intended process's shape; false
outcomes reported for processes that ran.

### nightfall

An instance terminates every node link it holds and holds the stack's front
door secrets. In the shipped overlays every instance mounts the same Secrets -
the device id key, the step-ca provisioner key, the ledger signing and param
keys, the fleet tokens' digests and the install token keys - and in the prod
overlay all instances share one Kafka user, so one compromised instance holds
what all of them hold.

**Can:** make any call on every node connected to it, without a membrane and
without writing it to the ledger; have step-ca sign a certificate for any
device and installation, and so connect to other instances as any node whose
state does not refuse it; sign ledger checkpoints, and write ledger entries to
any partition of `dusk.ledger`; write false connection, census and enrollment
events; test guesses of call parameters against `param_hash`, since it holds
the param key; derive the device id of any machine id.

**Cannot:** write `dusk.node-state`, which only twilight may write; change
ledger entries already in Kafka or in the evidence bucket; issue a certificate
outside the template's shape.

**Detected:** a `Dusk.process` it hides for a process dawn ran
(`result_without_ledger`). **Not detected:** calls it makes on its own and never
records. The only other trace is the node's own log buffer
(`logs dump --replay-only` on the node lists every process created, with its
pid), which nothing in the stack compares with the ledger.

### Kafka

Whoever controls the brokers, or holds an account that may administer them.

**Can:** read every topic - including `dusk.process-output`, which holds the
values scripts produced, and the node logs and telemetry on the `dusk.otel-*`
topics; write any record to any topic: lift a revocation or a quarantine with a
forged `dusk.node-state` record, which nightfall follows; forge presence, and so
make twilight send a node's work to the namespace id of another node; make
twilight's view of a campaign's results wrong; withhold records - when it
withholds the ledger, nightfall forwards no commands.

**Cannot:** issue certificates or sign ledger checkpoints; reach a node; change
an entry undetected once reconcile has read it or Vector has copied it to the
evidence bucket.

**Detected:** a node revoked in twilight that comes back online
(`revocation_not_enforced`); work that reached another node than intended,
after it ran (`target_mismatch`); results for processes the ledger never saw
(`result_without_ledger`); every edited, removed, reordered or inserted ledger
entry, by the chain. **Not detected:** what it reads; false results for
processes that ran; ledger entries removed after the last checkpoint before
anything read them.

### twilight

**Can:** intend any process on any node and have dawn run it; set any node's
lifecycle; read the inventory; and since reconcile runs inside twilight, it can
hide its own alerts.

**Cannot:** reach a node itself - nightfall's permissions give twilight's
principal no role, so only dawn reaches nodes; avoid the ledger: every call its
work causes is recorded by nightfall, with the pid, and copied to the evidence
bucket.

## What stays undetectable

By design, nothing in the stack detects:

* **The content of commands in a node's default shell, within their budget.**
  dawn's `ps`, reads, kills and reaps run there. Reconcile counts them per node
  against the budgets of the node's open intended processes, and alerts on one
  while none is open or on more than the budgets allow, but the ledger keeps
  only a keyed hash of a script, so a `ps` and any other script within the
  budget look alike. Each such call is still in the ledger, with its
  principal, node and time.
* **Processes created without a pid.** They are counted per principal, never
  alerted.
* **Calls a compromised nightfall makes on its own** and never writes to the
  ledger.
* **The newest ledger entries removed** after the last checkpoint, before
  reconcile read them or Vector copied them: at most one checkpoint interval,
  one second by default.
* **A chain forged by a holder of the ledger signing key.** A checkpoint proves
  the key holder wrote the chain, not that the chain is complete.
* **Different content inside one intended process's shape**, run by a
  compromised dawn or caused by a compromised twilight.
* **What a script does on the node.** The ledger records the `ShPortal.sh` call
  and its parameter hash, not the programs the script starts.
* **Values.** Parameters, results and output stay out of the ledger; two calls
  with the same parameters have the same hash.
* **What a node says about itself.** Facts, reported state, `ps`, its logs and
  the hardware fingerprint come from the node.
* **Nodes enrolled with a leaked fleet token** below the enrollment alert's
  rate. They are ordinary installations to the stack.
* **A capability returned to its owner.** When a client passes a node's own
  capability back to the node, the node holds its own object again, and
  dropping the membrane cannot take it back.

## Reporting a vulnerability

Report a vulnerability privately, as `SECURITY.md` at the root of the
repository describes.
