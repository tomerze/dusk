# The membrane

This page is for the people who operate nightfall and for the people who review
its security. It says what the membrane guarantees about every call that crosses
nightfall, which calls it admits against the processes twilight intended, what it
writes to the ledger, how `permissions.toml` is read, and what the membrane
cannot see.

The membrane is the part of nightfall that stands between a client on the inner
listener (dawn) and a node. A client never holds a capability of the node: it
holds a capability of nightfall that forwards to the node, and every call through
it is checked, limited and recorded first. It is built from the
`nightfall_membrane` crate in `services/nightfall/membrane/`, on capnp 0.21.7 and
capnp-rpc 0.21.0.

## What it guarantees

**One bootstrap per client and node.** Each inner connection gets its own
bootstrap, minted for the connection's principal (the name in its certificate's
`urn:dusk:principal:<name>` SAN) and the node it was routed to. The bootstrap
answers `Dusk` and nothing else: the methods the principal's roles allow are
forwarded to a fresh node-side `Dusk` that nightfall takes from the node's
`Dusk.dusk()`, so every client has a `Dusk` of its own. Any other interface on
the bootstrap answers `unimplemented`.

**Permissions decide what can be called at all.** A call the principal's roles
do not allow is answered `unimplemented` and never reaches the node. There is no
check after the fact.

**Only what twilight intended reaches a node.** A call the roles allow is held
against the processes twilight intends on that node before it is forwarded
([Admission](#admission)). A call no intended process allows fails with
`denied: not intended` and never reaches the node.

**Every capability is wrapped, in both directions.** A capability the node
returns is replaced by a nightfall capability before the client sees it, at any
depth of the result: in structs, groups, unions and lists. A capability the
client passes to the node (a `Stream` for output, a `Created` callback) is
wrapped the same way, so the node's calls back into the client are checked
against `reverse_allow` and recorded too.

**A capability going home is unwrapped.** When a client passes a capability it
got from the node back to the node (`Dusk.run(process)`), the node receives its
own object, not a wrapper, so the node's programs work exactly as they do
without nightfall. The call that carries it is still checked and recorded. Such a
capability cannot be revoked by the membrane, because its owner already holds it.

**Pipelining keeps working.** A client may call a method on a capability that a
call has not returned yet (`process().portal()`). The membrane serves those calls
for every place in the result where the schema declares an interface, and the
capability that arrives with the result is the same one, with the same cap id.
Such a capability may also be passed back to the node before its call has
returned (`Dusk.run(process().result)`), and the node receives its own object.
A pipelined call on any other place in a result fails with a null-capability
error.

**Commands are written before they are forwarded.** A client-to-node call whose
method does not stream is forwarded only after the ledger has committed its
`call` entry. If the ledger refuses the entry or does not commit it within
`write_ahead_timeout_ms` (5 s), the call fails `overloaded` and the node never
sees it. Streaming calls (`Stream.send`), node-to-client calls and result entries
are recorded without waiting.

**Calls on one capability stay in order.** Calls arrive at the node, or at the
client, in the order they were made on that capability, including while one of
them waits for the ledger or for a limit.

**Dropping a membrane kills everything derived from it.** When nightfall drops
a membrane (a kill from the admin API, the node session closing, a lifecycle
change, a permissions reload), every
capability the client holds through it starts failing with `disconnected:
session revoked`, calls already forwarded fail the same way at once, and the node
is told to release every capability the membrane held.

**Only level-one RPC passes.** Every Cap'n Proto connection nightfall speaks, to
nodes and to clients, goes through an allow-list filter before capnp-rpc reads a
message. It passes `unimplemented`, `abort`, `bootstrap`, `call` (results to the
caller or to the callee), `return` (results, exception, canceled,
resultsSentElsewhere, takeFromOtherQuestion), `finish`, `resolve`, `release` and
`disembargo` (sender or receiver loopback), with capability descriptors `none`,
`senderHosted`, `senderPromise`, `receiverHosted` and `receiverAnswer`. Anything
else - `provide`, `accept`, `join`, third-party handoff in any form, or a message
kind the schema does not know - aborts the connection with `rejected rpc
message: <kind>`, logged at `error` and recorded in the ledger as an
`rpc_rejected` event. capnp-rpc 0.21.0 refuses most of these
itself; the filter is there so that a future capnp-rpc that can hand a
capability to a third party fails loudly instead of letting it bypass the
membrane.

## Admission

twilight writes every process it intends - the pid, the node, until when, and
how many shell commands it may run - to the compacted Kafka topic
`dusk.intended-processes` before it asks dawn for the process, and a tombstone
once the process expires or is reaped.
Every nightfall instance reads that topic from the beginning, on every partition,
into a table of the intended processes of every node, and holds each
client-to-node call against the table of the call's node before forwarding it.

A call the rules refuse fails with `failed` and the message `denied: not
intended`, is never forwarded, is recorded as a `call` entry with result
`denied` and `event_detail` `{"rule": "<rule>"}`, is counted in
`nightfall_admission_refused_total{rule}`, and is logged at `info` with its
principal, session, action, pid and rule.

| rule | refuses |
|------|---------|
| `process_without_intent` | `Dusk.process` at a pid that no intended process of the node names - including a pid intended for another node. |
| `process_without_pid` | `Dusk.process` with no pid, but one: the read-only `kvs bind` process the `kvs` client starts to read the key names `kvs get` matches, while an intended process of the node is open. `ShPortal.sh` on a shell nightfall saw created without a pid is refused the same way. |
| `process_after_deadline` | any call under a pid whose intended process expired - more than `clock_skew_ms` past its `expires_at` - or that the table no longer holds, but `Dusk.kill` and `Dusk.waitpid`. |
| `command_budget` | `ShPortal.sh` under a pid once the shell at that pid has taken `max_commands` of them. |
| `default_shell_without_intent` | `Dusk.process` at the default shell's pid (`sh.capnp`'s `defaultPid`), and `ShPortal.sh` in the default shell, while no intended process of the node is open. A process is open from `clock_skew_ms` before its `created_at` to `clock_skew_ms` after its `expires_at`. |
| `default_shell_budget` | `ShPortal.sh` in the default shell beyond the `default_shell_commands` of the node's intended processes that overlap the current window. The window starts at the `created_at` of the earliest open process and starts again, with a fresh count, when that process closes. |
| `not_caught_up` | every call a rule above would judge, until this instance has read `dusk.intended-processes` up to the end it had at startup. The instance is not ready meanwhile. |

Calls under no process (pid 0: `Dusk.ps`, `Dusk.hostname` and the other calls on
the bootstrap), calls under the default shell's pid other than `ShPortal.sh`,
`Dusk.kill` and `Dusk.waitpid` at any pid, and node-to-client calls are not
judged.

**A call waits for the newest intent before it is refused.** twilight writes an
intended process and waits for Kafka to acknowledge it before it calls dawn, but
nightfall may not have read it yet when dawn's call arrives. So a call the rules
would refuse first waits, up to `intent_wait_ms`, for this instance to read
`dusk.intended-processes` up to the end it has when the call arrives, and is
then judged again. A process twilight intended is admitted however close behind
dawn's call its record arrives; a call nobody intended is refused
`intent_wait_ms` late.

**Budgets are counted per instance.** The `ShPortal.sh` calls of a pid and of a
node's default shell are counted by the instance that holds the node session,
and every client of that node shares the count. A node that reconnects to
another instance starts that instance's count from zero; twilight's reconcile
still holds the totals.

**The table is bounded.** It holds at most `max_intended_processes` processes
across every node; an expired one is evicted within 10 seconds. A record that
would take the table past its bound is dropped - the process stays unintended
and its calls are refused - and counted in
`nightfall_intended_processes_dropped_total`, with an `error` log. The number
held is `nightfall_intended_processes`, and how far behind the topic this
instance reads is `nightfall_intended_processes_lag_seconds`.

**Roles exempt from admission.** A role with `admission_exempt = true` is not
held to the intended processes: its calls are forwarded whatever the table
holds. This is for an operator's break-glass role. Every call of such a role
that a rule would have refused is recorded with an `admission_override` event
that names the role and the rule, and logged at `warn`; reconcile still raises
its alerts for those calls, since they reach the node.

## Limits

The `[limits]` section of `nightfall.toml` bounds each node session and the whole
instance. The per-session limits belong to the node session and are shared by
every client connected to it, so one principal can use up another's share.

Every call takes a token before nightfall reads its parameters: a client-to-node
call from the bucket of its principal on that node session, a node-to-client call
from the node session's own bucket, and both from the instance's bucket. Denied
calls and calls on an unknown interface take one too. A call that finds no token,
or that goes over another limit, fails `overloaded` with the name of the limit
and is recorded as one `call` entry with result `rate_limited`; a call refused
for want of a token is refused before its parameters are read, so its entry has
no `param_fields` and an empty `param_hash`.

A call counts against the limits from the moment it arrives. It first waits for
the calls before it on the same capability, then for a slot. Client-to-node and
node-to-client calls have separate slots, so the callbacks a command needs are
never starved by commands. A client-to-node call that does not stream fails if
no slot is free when its turn comes. A streaming call (a method returning
`stream`, such as `Stream.send` or `Sink.write`) and a node-to-client call wait
for a token and a slot instead, which holds back the sender, and fail only after
`stream_wait_timeout_ms` - or at once when the node session already holds
`max_waiting_calls_per_session` waiting calls or its byte bound, which a sender
that keeps to Cap'n Proto flow control does not reach.

nightfall writes the ledger entries of refused calls up to
`calls_per_second_per_principal_per_node` a second for each client connection.
Refusals past that are counted in `nightfall_unrecorded_refusals_total` and
logged at `debug` instead, so a flood of refused calls cannot fill the ledger.

| key | default | bounds |
|-----|---------|--------|
| `calls_per_second_per_principal_per_node` | 200 | client-to-node calls of one principal on one node session (burst: one second's worth) |
| `reverse_calls_per_second_per_node` | 10000 | node-to-client calls of one node session |
| `calls_per_second_instance` | 200000 | calls of the whole instance, both directions |
| `max_inflight_calls_per_session` | 256 | client-to-node calls forwarded and not yet answered, per node session |
| `max_inflight_reverse_calls_per_session` | 256 | node-to-client calls forwarded and not yet answered, per node session |
| `max_waiting_calls_per_session` | 4096 | calls received and not yet forwarded, per node session |
| `max_inflight_bytes_per_session` | 8388608 | bytes of parameters and results held for a node session's calls, waiting calls and copies included |
| `max_inflight_bytes_instance` | 4294967296 | the same for the whole instance |
| `max_live_caps_per_session` | 10000 | capabilities handed out on one node session and not yet released |
| `max_live_caps_instance` | 10000000 | the same for the whole instance |
| `write_ahead_timeout_ms` | 5000 | how long a command waits for its ledger entry to commit |
| `stream_wait_timeout_ms` | 60000 | how long a call waits for its turn on a capability, a token and a slot |

A call whose parameters or result would take a session or the instance past its
capability bound fails `overloaded`, streaming or not, and so does a call whose
parameters or result carry more than 10 000 capabilities, the most one ledger
entry lists. A rate of 0 is treated as
1 call per second.

A call whose parameters cannot be read is refused, recorded as a denied call
with an empty `param_hash`, and logged at `warn`.

## What it records

Every call that reaches the membrane is a ledger entry of kind `call`, and every
call it forwarded is followed by one of kind `result` with the same `call_id`. A
call that ends before the node answers it - the client cancelled it or its
connection closed - gets the result `error:disconnected`, or `revoked` when the
membrane was dropped.
The authoritative field list is `services/contracts/kafka/dusk.ledger.schema.json`;
the membrane fills these:

* `principal`, `device_id`, `installation_id`, `namespace_id`, `epoch` - who
  called which node session.
* `session_id` - one UUID per inner connection.
* `intent_campaign_id`, `intent_principal`, `intent_subject` - the campaign,
  the principal who asked and the subject (`campaign:<id>` or the operator) of
  the intended process at `pid`, as nightfall held it when it saw the call; null
  when it held none, as for pid `"0"` and the default shell. A result carries
  what its call carried. They let the ledger name who asked for a process after
  twilight has dropped its own record of it.
* `pid` - the process the call is about, as a decimal string. A process is the
  unit of work on a node, so this is how a call is tied to the work it belongs
  to. It is the fixed pid in the `ProgramArgs` of `Dusk.process`, the `pid` of
  `Dusk.kill` and `Dusk.waitpid`, and the pid of the process passed to
  `Dusk.run`; every capability a call returns or carries keeps the pid of that
  call, so the `Process`, its portal, `ShPortal.sh` and the `Stream` the script
  writes into all carry the pid of the process. `"0"` when the call is about no
  process, such as `Dusk.ps` on the bootstrap or a `Dusk.process` whose pid the
  node picks. A call refused for want of a token is refused before its
  parameters are read, so it carries the pid of the capability it was made on.
* `cap_id` and `parent_cap_id` - the capability called and the capability whose
  call produced it. The bootstrap is cap 0. Together they form the session's
  capability tree; the same capability crossing again keeps its cap id.
* `direction` - `client_to_node` or `node_to_client`.
* `action`, `interface_id`, `method_id` - the call, named from the schema bundle
  that matches the node (`ShPortal.sh`, `Stream.send`).
* `param_fields` - the names of the parameters, each with `redacted: true` when
  its value was left out of the hash.
* `param_cap_ids` and `result_cap_ids` - the cap ids of the capabilities passed
  and returned.
* `param_hash` - see below.
* `result_code` - `ok`, `error:failed`, `error:overloaded`,
  `error:disconnected`, `error:unimplemented`, `revoked` on results; `denied` or
  `rate_limited` on a call that was refused and never forwarded.

A call on an interface the node's schema bundle does not know is refused
(`unimplemented`) and recorded as a denied call whose action is
`unknown:<16 hex interface id>.<method id>`, with an empty `param_hash`. A call
refused by [admission](#admission) is a denied call whose `event_detail` names
the rule; one refused by permissions has a null `event_detail`.

Events the membrane writes:

* `quarantine_override` - a call on a quarantined node was let through by a role
  with `quarantine_override = true`; it carries the call's `call_id` and `pid`.
* `admission_override` - a call that no intended process allows was let through
  by a role with `admission_exempt = true`; it carries the call's `call_id`,
  `action` and `pid`, and `event_detail` `{"role", "rule"}` names the role and
  the rule the call broke.
* `membrane_dropped` - a membrane was dropped; `event_detail.reason` says why
  (`permissions_changed`, `principal_revoked`, or the reason nightfall gave), and
  `event_detail.by` names the admin principal when an admin killed it.

**The membrane never records a value.** Not a parameter, not a result, not a
line of output. `param_hash` is HMAC-SHA256, keyed with the ledger param key, of
the parameters in Cap'n Proto canonical form after every capability pointer and
every field marked `$Dusk.sensitive` in the schema has been cleared. The schema
is followed into program arguments: the `data` of a `ProgramArgs` is read with
the arguments schema of the program it names, so `kvs set`'s value is cleared
even inside `Dusk.process`. Two calls with the same parameters hash the same, and
a caller holding the param key and the parameters can recompute the hash; nobody
can read the parameters back from it. Content under an untyped pointer that
holds a capability cannot be put in canonical form; it is cleared and its field
is listed `redacted: true`.

The capability tree is also kept in memory while it is alive: an entry stays
while its capability is held or a capability derived from it is. Nightfall's
admin API serves it per session.

## What it cannot see

The membrane sees calls between a client and a node. It does not see what a
node does with them.

* **A script is one call.** `ShPortal.sh` carries a compiled script. The ledger
  records that call, its hash and the capabilities it carried, but the programs
  the script starts run inside the node and never cross nightfall. Only their
  calls back into the client (output written to the client's `Stream`, a `logs`
  stream opened on the client) appear, as `node_to_client` calls. **A role that
  allows `ShPortal.sh` or `Dusk.process` has full control of the node, whatever
  else it denies.**
* **Output is never recorded.** `Stream.send` entries carry the hash of the
  value, not the value.
* **A capability that went home cannot be revoked.** Once the node holds its own
  object again, dropping the membrane does not take it back.
* **Untyped content is opaque.** Two calls that differ only inside cleared
  content hash the same.
* **Admission does not read a script.** It counts `ShPortal.sh` calls; what a
  script runs inside an intended process's budget is not checked, and neither is
  which of a node's open processes a command in its default shell serves.
* **Interfaces newer than nightfall are refused.** A node running a program
  whose schema is in none of nightfall's bundles cannot be driven through that
  program's portal. Roll out nightfall, with the new bundle, before the nodes.

## The permissions file

`/etc/nightfall/permissions.toml` (the `[permissions] file` key of
`nightfall.toml`). A key the file does not know, a role defined twice, a
principal naming an undefined role and a pattern that is not
`<Interface>.<method>` are refused when the file is read, so a typo never widens
a role.

```toml
deny_certificates = []
deny_principals = []

[[role]]
name = "dawn"
allow = ["Dusk.process", "Dusk.run", "Dusk.ps", "Dusk.kill", "Dusk.hostname", "Dusk.waitpid", "Dusk.time", "Dusk.programs", "Dusk.namespaceId", "Dusk.dusk", "Process.*", "Portal.*", "OutputPortal.*", "ShPortal.*", "KvsPortal.get", "KvsPortal.exists", "KvsPortal.scan", "SignalBatch.Ack.ack"]
deny = ["Dusk.settime", "Dusk.fleetToken"]
reverse_allow = ["Stream.*", "Created.created", "Sink.*", "LogsArgs.Server.openStream", "LogsArgs.Stream.*", "CpArgs.Server.write", "CpArgs.Server.stat", "CpArgs.Server.hash", "KvsArgs.Server.transpose", "ProgramsArgs.Server.transpose", "ShStop.stop"]
quarantine_override = false

[[role]]
name = "quarantine"
allow = ["Dusk.hostname", "Dusk.programs", "Dusk.namespaceId", "Dusk.time"]
reverse_allow = []

[[principal]]
name = "dawn-*"
roles = ["dawn"]
```

| key | type | meaning |
|-----|------|---------|
| `deny_certificates` | list of strings | SHA-256 of a client certificate's DER, 64 hex digits, colons allowed. Checked when a client connects. |
| `deny_principals` | list of patterns | principals that get no membrane at all, whatever their roles. |
| `[[role]] name` | string | the role's name, unique in the file. |
| `allow` | list of patterns | client-to-node calls the role permits. |
| `deny` | list of patterns | client-to-node calls refused even when a role allows them. |
| `reverse_allow` | list of patterns | node-to-client calls the role permits; everything else the node calls on the client's capabilities is refused. |
| `quarantine_override` | bool, default false | the role's grants also apply on quarantined nodes, and each such call writes a `quarantine_override` event. |
| `admission_exempt` | bool, default false | the role's calls are not held to the intended processes ([Admission](#admission)), and each call a rule would have refused writes an `admission_override` event. |
| `[[principal]] name` | pattern | principal names this entry applies to. |
| `roles` | list of strings | the roles those principals get. |

**Patterns** are `<Interface>.<method>`, split at the last dot, so nested
interfaces keep their dots (`LogsArgs.Server.openStream`). The interface is the
schema's name without its file (`ShPortal`, `SignalBatch.Ack`). `*` matches any
run of characters in either part; names and patterns use letters, digits, `_`
and `.`. A role allowing `*.*` is accepted and logged as a warning.

**How a call is decided.** A principal's roles are those of every
`[[principal]]` entry whose pattern matches its name. A client-to-node call is
allowed when at least one of those roles allows it and none of them denies it;
deny always wins, and anything not allowed is denied. A node-to-client call is
allowed when at least one role's `reverse_allow` matches it.

**Quarantined nodes.** On a quarantined node a call must also be allowed by the
role named `quarantine` (its `deny` applies too); without a `quarantine` role,
nothing is allowed. A role with `quarantine_override = true` keeps its own
grants.

`Dusk.fleetToken` returns the secret every node of the fleet enrolls with; never
allow it to a role. `ShStop.stop` must be in `reverse_allow` of every role that
runs scripts with `ShPortal.sh`: the node treats a failed `stop` call as the
order to stop, so without it every script stops as soon as it starts.

**Reloading.** When nightfall reloads the file, every live membrane is checked
against it. A membrane whose principal would get the same permissions keeps
running. One whose permissions changed is dropped (`permissions_changed`), and
one whose principal is now denied or no longer matches any role is dropped
(`principal_revoked`). A connection made after the reload gets a membrane minted
from the new file.

## Schema bundles

The names in the ledger, the streaming methods, the sensitive fields and the
places where results hold capabilities all come from schema bundles that
nightfall loads at start from `/usr/share/nightfall/schemas/` (the `[schemas]
directory` key). Each bundle is `<dusk version>-<git revision>.capnp.bin`, a
`CodeGeneratorRequest` of every schema a node of that build speaks, next to a
`<dusk version>-<git revision>.json` manifest naming the version, the revision and
every program's id and version. `services/nightfall/schemas/build.sh <output
directory>` builds the bundle of the tree it is run in (it needs the `capnp`
compiler, from `CAPNP` or `PATH`, and the revision from `DUSK_GIT_REV` or git).
A node session uses the bundle whose programs match the node's `Dusk.programs`
best, and the newest bundle when none matches.
