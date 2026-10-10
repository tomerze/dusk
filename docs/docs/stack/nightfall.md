# Running nightfall

This page is for the people who deploy and operate nightfall, the service every
node of a fleet connects to and every dawn instance reaches nodes through. It
covers what nightfall needs to start, every key of its configuration with its
default, how to size a host for it, its admin API and metrics, and how it shuts
down. It describes nightfall 0.1.0, built from `services/nightfall/` (package
`nightfall`, binary `nightfall`).

What nightfall does with a call once a client holds a node is on
[the membrane](membrane.md) page, the record it keeps on [the ledger](ledger.md)
page, and how nodes get their certificates on [provisioning](provisioning.md).

## What it serves

nightfall listens on four ports:

| port (default) | key | who connects | TLS |
|----------------|-----|--------------|-----|
| 8443 | `fleet.listen`, `provision.listen` | nodes | TLS 1.3 and a client certificate from the fleet-client CA for a name in `fleet.server_names`; TLS 1.2 or 1.3 and an optional client certificate for a name in `provision.server_names` |
| 8444 | `inner.listen` | dawn | TLS 1.3 and a client certificate from the internal CA |
| 8445 | `inner.relay_listen` | other nightfall instances | the client's own TLS, relayed |
| 9100 | `admin.listen` | probes, Prometheus, admins | none, or TLS when `[admin]` names a certificate |

The fleet and provision names usually share one port, so one load balancer
address serves both; nightfall reads the server name (SNI) a client sends
before it does any TLS work and picks the configuration by it. A connection that
names a server nightfall does not serve gets the TLS `unrecognized_name` alert
and is closed.

**A node** connects to a name in `fleet.server_names` with its node certificate
and stays connected. nightfall asks it for its namespace id, binds that id to
the device and installation in the certificate, and announces the session on
`dusk.connections`. A namespace id that another identity already holds, on this
instance or another one, is refused.

**A client** connects to `<namespace id>.<inner.server_name_suffix>`, where the
namespace id is 16 lowercase hex digits from `dusk.connections`, at the
`inner_address` that message names. When the node is on another instance (it
reconnected since), the instance the client reached relays the connection,
untouched, to the instance holding the node, as long as it knows that instance
from the census and `dusk.connections` and runs fewer than
`max_relayed_connections` relays; past that bound the client is refused. When no
instance holds the node, or this instance has not yet learned which one does,
the client still completes TLS and every call fails with `disconnected: node
<namespace id> is not connected`. The inner certificate therefore needs the DNS name
`*.<inner.server_name_suffix>`.

## Starting it

```sh
nightfall --config /etc/nightfall/nightfall.toml
```

`--config` defaults to `/etc/nightfall/nightfall.toml`; `nightfall serve` is the
same command. nightfall writes JSON logs, one object per line, to standard
output; `NIGHTFALL_LOG` filters them with the `tracing` directive syntax (for
example `NIGHTFALL_LOG=info,nightfall::listener=debug`), and the default is
`info`. It exits with status 0 after draining and 1 when it cannot start or when
a newer nightfall with the same `instance` took over its ledger.

It refuses to start when the configuration does not validate, when a
certificate, key, CA bundle, permissions file, schema bundle or secret it names
cannot be loaded, when `fleet.client_ca` and `inner.client_ca` share a
certificate (nodes and principals must come from separate CAs), or when the
ledger topic has fewer partitions than the ledger partition + 1.

### What it needs

| what | default path | key |
|------|--------------|-----|
| fleet server certificate and key (fleet-server CA) | `/etc/nightfall/tls/fleet.crt`, `fleet.key` | `fleet.certificate`, `fleet.key` |
| fleet-client CA bundle, the CA that signs node certificates | `/etc/nightfall/pki/fleet-client-ca.crt` | `fleet.client_ca` |
| provision server certificate and key (fleet-server CA) | `/etc/nightfall/tls/provision.crt`, `provision.key` | `provision.certificate`, `provision.key` |
| fleet tokens, install token keys, device id key | `/etc/nightfall/secrets/` | `[provision]`, see [provisioning](provisioning.md) |
| inner server certificate and key (internal CA, DNS `*.<suffix>`) | `/etc/nightfall/tls/inner.crt`, `inner.key` | `inner.certificate`, `inner.key` |
| internal CA bundle, the CA that signs principals | `/etc/nightfall/pki/internal-ca.crt` | `inner.client_ca` |
| step-ca root and nightfall's provisioner JWK | `/etc/nightfall/pki/fleet-client-root.crt`, `/etc/nightfall/secrets/provisioner.jwk` | `[step_ca]` |
| ledger signing key (Ed25519 PKCS#8 PEM) and param key (hex) | `/etc/nightfall/secrets/ledger-signing.key`, `ledger-param.key` | `[ledger]`, see [the ledger](ledger.md) |
| schema bundles | `/usr/share/nightfall/schemas/` | `schemas.directory` |
| permissions | `/etc/nightfall/permissions.toml` | `permissions.file`, see [the membrane](membrane.md#the-permissions-file) |

nightfall runs as an unprivileged user: its default ports are above 1023 and it
needs no capability. It needs read access to every file above and nothing
written to disk.

### Kafka topics

nightfall never creates topics. At start, and every 30 seconds after, it
describes:

| key | default | cleanup.policy |
|-----|---------|----------------|
| `kafka.topics.connections` | `dusk.connections` | `delete` |
| `kafka.topics.census` | `dusk.census` | `compact` |
| `kafka.topics.ledger` | `dusk.ledger` | `delete` |
| `kafka.topics.enrollments` | `dusk.enrollments` | `delete` |
| `kafka.topics.node_state` | `dusk.node-state` | `compact` |
| `kafka.topics.intended_processes` | `dusk.intended-processes` | `compact` |

It stays not ready while one is missing or its `cleanup.policy` is not exactly
the one above (`compact,delete` on `dusk.node-state` would let Kafka delete the
records of revoked nodes, and on `dusk.intended-processes` the records of
processes still intended). Each instance writes its ledger to partition `ledger.partition`
with the transactional id `nightfall-ledger-<instance>`, and writes its census
to the partition the Java default partitioner gives its instance name. It reads
`dusk.node-state`, `dusk.intended-processes` and `dusk.census` from the beginning and `dusk.connections` from
60 seconds before the oldest census it applied was taken, with partitions assigned directly (no
consumer group rebalancing). Every message it reads is checked against its
contract in `services/contracts/kafka/`; an invalid one is dropped, counted in
`nightfall_invalid_messages_total` and logged at `warn` with its topic,
partition and offset.

The ledger, the census and the other events each have a producer of their own,
so a census burst never fills the ledger's queue.

### Readiness

`GET /readyz` answers 200 only when all of these hold:

* `dusk.node-state` was read up to the end it had when nightfall started. Until
  then nightfall closes provisioning connections before TLS, and once it is read
  it closes every node session the states revoke or retire;
* `dusk.intended-processes` was read up to the end it had when nightfall
  started. Until then [admission](membrane.md#admission) refuses every call it
  judges with `not_caught_up`;
* `dusk.census` was read to its end and nightfall reads `dusk.connections`, so it
  knows which namespace ids other instances hold;
* every topic above exists with its cleanup policy;
* the ledger can take entries;
* nightfall is not draining.

## Configuration

`/etc/nightfall/nightfall.toml` is TOML. A key nightfall does not know is refused,
and a key left out takes its default. Every key can be set from the environment
as `NIGHTFALL__<SECTION>__<KEY>` (`NIGHTFALL__INSTANCE` for a top-level key),
which wins over the file. The value is read as the type of the key it replaces:
a string stays a string, numbers and `true`/`false` are parsed, and arrays and
tables are TOML literals:

```sh
NIGHTFALL__INSTANCE=nightfall-3
NIGHTFALL__INNER__ADVERTISE=nightfall-3.nightfall-inner.dusk.svc:8444
NIGHTFALL__FLEET__SERVER_NAMES='["fleet.example.com"]'
NIGHTFALL__KAFKA__PROPERTIES='{ "security.protocol" = "SASL_SSL", "sasl.mechanism" = "SCRAM-SHA-512" }'
```

A Kafka property name holds dots, so it can only be set through the whole
`kafka.properties` table, as above.

### Top level

| key | default | meaning |
|-----|---------|---------|
| `instance` | `"nightfall-0"` | This instance's name in every message and ledger entry. No `/`. In Kubernetes, the pod name. |
| `shards` | `0` | Worker threads that hold sessions; 0 is one per available core. |
| `drain_seconds` | `300` | How long closing the sessions takes on SIGTERM. |

### `[fleet]`

| key | default | meaning |
|-----|---------|---------|
| `listen` | `"0.0.0.0:8443"` | Address nodes connect to. |
| `server_names` | `["fleet.dusk.example"]` | Names nodes connect to; lowercase DNS names. |
| `certificate`, `key` | `/etc/nightfall/tls/fleet.crt`, `/etc/nightfall/tls/fleet.key` | Server certificate chain and key, PEM. |
| `client_ca` | `/etc/nightfall/pki/fleet-client-ca.crt` | The fleet-client CA; node certificates must chain to it. |
| `handshake_timeout_ms` | `10000` | Bound on each of reading the ClientHello, reading a PROXY header and the TLS handshake, on every listener, so a connection may take up to three times this before its session starts. |
| `heartbeat_seconds` | `30` | How often nightfall calls `Dusk.time` on each node, ±20 %. Two misses in a row close the session (`idle_timeout`); each call waits at most 10 s, or the period when that is shorter. |
| `session_setup_timeout_ms` | `15000` | Bound on a node's setup calls after TLS (`setup_timeout`). |
| `max_sessions` | `100000` | Node sessions this instance holds, counting nodes still in their TLS handshake; lowered at start to the open file limit minus the descriptors other connections may hold (see [sizing](#sizing)). Beyond it new nodes are refused before TLS. |
| `proxy_protocol` | `false` | Read a PROXY protocol v2 header before the TLS of every connection on the fleet and provision listeners, to learn the node's address behind a load balancer. |
| `proxy_protocol_trusted_cidrs` | `[]` | Load balancer addresses allowed to send that header; required when `proxy_protocol` is true. Other peers are refused. |

### `[provision]`

| key | default | meaning |
|-----|---------|---------|
| `listen` | `"0.0.0.0:8443"` | The same address as `fleet.listen` shares its port; another address gets a listener of its own. |
| `server_names` | `["provision.dusk.example"]` | Names nodes provision through; distinct from `fleet.server_names`. |
| `certificate`, `key` | `/etc/nightfall/tls/provision.crt`, `/etc/nightfall/tls/provision.key` | Server certificate chain and key, PEM. |
| `fleet_tokens_file` | `/etc/nightfall/secrets/fleet-tokens.toml` | See [fleet tokens](provisioning.md#fleet-tokens). |
| `install_token_keys` | `/etc/nightfall/secrets/install-token-jwks.json` | See [install tokens](provisioning.md#install-tokens). |
| `device_id_key_file` | `/etc/nightfall/secrets/device-id.key` | See [the device id key](provisioning.md#the-device-id-key). |
| `challenge_ttl_ms` | `300000` | Lifetime of an assign challenge, and the longest a provisioning connection stays open. A provisioning connection that sends and receives nothing for 30 seconds is closed sooner, and its messages may be at most 64 KiB. |
| `renew_grace` | `"2160h"` | How long after expiry a certificate may still be renewed; Go duration syntax (`ns`, `us`, `ms`, `s`, `m`, `h`). |
| `tpm_endorsement_roots` | `""` | PEM file of the TPM manufacturer certificates an attesting node's endorsement key certificate must chain to; empty refuses every node that attests. See [TPM attestation](provisioning.md#tpm-attestation). |

### `[inner]`

| key | default | meaning |
|-----|---------|---------|
| `listen` | `"0.0.0.0:8444"` | Address clients connect to. |
| `relay_listen` | `"0.0.0.0:8445"` | Address other instances relay clients to. Expose it to nightfall instances only: it trusts the PROXY header every connection starts with. |
| `advertise` | `"nightfall-0.nightfall-inner.dusk.svc:8444"` | `host:port` clients reach this instance's inner listener at; published as `inner_address`. |
| `relay_advertise` | `"nightfall-0.nightfall-inner.dusk.svc:8445"` | `host:port` other instances reach this instance's relay listener at; published in the census. |
| `server_name_suffix` | `"fleet.dusk.example"` | Clients connect to `<namespace id>.<suffix>`. |
| `certificate`, `key` | `/etc/nightfall/tls/inner.crt`, `/etc/nightfall/tls/inner.key` | Server certificate with DNS name `*.<suffix>`, and its key. |
| `client_ca` | `/etc/nightfall/pki/internal-ca.crt` | The internal CA; client certificates must chain to it, carry exactly one `urn:dusk:principal:<name>` URI and no node identity. |

### `[step_ca]`

| key | default | meaning |
|-----|---------|---------|
| `url` | `"https://step-ca:9000"` | step-ca, the fleet-client CA. Must be https. |
| `root` | `/etc/nightfall/pki/fleet-client-root.crt` | The root nightfall trusts step-ca's TLS certificate with. |
| `provisioner` | `"nightfall"` | The JWK provisioner's name. |
| `provisioner_key_file` | `/etc/nightfall/secrets/provisioner.jwk` | Its private JWK, decrypted. |
| `certificate_lifetime` | `"168h"` | Lifetime nightfall asks for; must match the provisioner's claims. |
| `max_concurrent` | `16` | Signing requests in flight; beyond it nodes are told `overloaded`. |
| `timeout_ms` | `10000` | Bound on each request to step-ca. |

### `[ledger]`

| key | default | meaning |
|-----|---------|---------|
| `signing_key_file` | `/etc/nightfall/secrets/ledger-signing.key` | Ed25519 key that signs checkpoints. |
| `param_key_file` | `/etc/nightfall/secrets/ledger-param.key` | HMAC key of `param_hash`, at least 32 bytes as hex. |
| `checkpoint_interval_ms` | `1000` | How often a signed checkpoint closes the chain. |
| `queue_entries` | `100000` | Entries waiting for Kafka; a call is admitted only when its call and result entries fit. A node session's open and close entries are held apart, two for each of `fleet.max_sessions`, so sessions never take this room from calls. |
| `partition` | `-1` | The ledger partition this instance writes; -1 takes the number at the end of `instance` (the StatefulSet ordinal). |

### `[kafka]`

| key | default | meaning |
|-----|---------|---------|
| `brokers` | `"kafka:9092"` | Bootstrap servers. |
| `allow_plaintext` | `false` | Allow `security.protocol` other than `ssl` or `sasl_ssl`. Development only. |
| `properties` | `{}` | librdkafka properties for every client (security, SASL, TLS files). The ledger's idempotence and transaction settings cannot be overridden. |
| `topics` | see [Kafka topics](#kafka-topics) | Topic names. |
| `census_interval_seconds` | `300` | How often the full census is written. |
| `census_heartbeat_seconds` | `15` | How often a header-only census heartbeat is written; an instance silent for three of its heartbeats is dead to the others. Must be shorter than the interval. |

### `[admission]`

How the membrane holds calls against the processes twilight intends; see
[admission](membrane.md#admission).

| key | default | meaning |
|-----|---------|---------|
| `max_intended_processes` | `1000000` | The most intended processes the instance holds, across every node. A record past it is dropped and its process stays unintended. Size it above the number of processes twilight has open at once - a campaign phase holds one per node, plus the facts, reaps and sessions of every node that connects. |
| `clock_skew_ms` | `5000` | How far twilight's clock and this instance's may disagree: a process counts as open this long before its `created_at` and after its `expires_at`. At most 3600000. |
| `intent_wait_ms` | `3000` | How long a call no intended process allows waits for this instance to read `dusk.intended-processes` up to its end before it is refused. At most 60000. |

### `[schemas]`, `[permissions]`

| key | default | meaning |
|-----|---------|---------|
| `schemas.directory` | `/usr/share/nightfall/schemas` | The schema bundles; see [schema bundles](membrane.md#schema-bundles). |
| `permissions.file` | `/etc/nightfall/permissions.toml` | See [the permissions file](membrane.md#the-permissions-file). |

### `[limits]`

The call and capability limits are on [the membrane](membrane.md#limits) page.
These act before or around them:

| key | default | meaning |
|-----|---------|---------|
| `handshakes_per_second` | `2000` | TLS handshakes the instance starts per second, on every listener. Beyond it connections are closed before TLS. |
| `handshake_failures_per_ip_per_minute` | `30` | Failed handshakes from one address before it is refused for `penalty_seconds`. |
| `credential_failures_per_ip_per_hour` | `20` | Failed credentials (certificates without a valid identity, refused fleet or install tokens) from one address before it is refused. |
| `penalty_seconds` | `60` | How long a penalized address is refused. |
| `session_setups_per_identity_per_5s` | `1` | Node sessions one device and installation may start in 5 seconds. |
| `enrollments_per_second`, `enrollments_per_second_per_credential`, `enrollment_alert_per_minute` | `50`, `10`, `600` | See [provisioning limits](provisioning.md#limits). |
| `max_message_bytes` | `4194304` | Largest Cap'n Proto message read on any link. |
| `max_relayed_connections` | `10000` | Relays this instance runs at once. |
| `max_provisioning_connections` | `10000` | Provisioning connections this instance holds at once. Beyond it new ones are closed before TLS. |
| `max_provisioning_connections_per_ip` | `16` | Provisioning connections one address holds at once (one /64 for IPv6); networks in `per_ip_exempt_cidrs` are not bound. |
| `per_ip_exempt_cidrs` | `[]` | Networks never penalized and not bound per address, such as a large NAT in front of many nodes. |

Only failures count against an address; a node that connects and authenticates
never does. An IPv6 address counts as its /64. The penalty box counts the
failures of up to about a million addresses, forgetting the ones it has not seen
for the longest past that, and holds up to about a million penalties, lifting the
oldest early past that. `[[limits.cidr]]` tables tune one network:

```toml
[[limits.cidr]]
cidr = "203.0.113.0/24"
handshake_failures_per_minute = 300
enrollments_per_hour = 5000
```

The most specific matching table applies; `enrollments_per_hour` caps the
enrollments the whole network starts in an hour. An enrollment counts once, at
its assign call; its enroll call does not count again.

### `[admin]`

| key | default | meaning |
|-----|---------|---------|
| `listen` | `"0.0.0.0:9100"` | Health, readiness, metrics and the admin API. |
| `certificate`, `key`, `client_ca` | `""` | Set all three (a server certificate and the internal CA) to serve TLS and enable the admin API; set none to serve plain HTTP without it. |

## Reloading

Every 30 seconds nightfall checks the modification time of the server
certificates and keys, the two client CA bundles and the admin TLS files, and
swaps in what changed, including the trust nightfall checks renewing
certificates with, which follows `fleet.client_ca`; a change that does not load (a key that does not match its
certificate, a client CA bundle that now shares a certificate with the other
client CA bundle) keeps the previous material, is logged at `warn` and counted in
`nightfall_tls_reload_failures_total`.
`permissions.toml` is checked the same way: every live client whose policy
changed, whose principal lost its roles or whose certificate is now denied is
disconnected, and the rest keep their connections.

The fleet tokens, the install token keys, the device id key, the ledger keys,
the step-ca settings and every other key of `nightfall.toml` are read at start; change them with a
restart.

## The admin API

`admin.listen` always answers:

| request | answer |
|---------|--------|
| `GET /healthz` | `200 ok` while the process runs |
| `GET /readyz` | `200 ready`, or `503 not ready: <reason>` (see [readiness](#readiness)) |
| `GET /metrics` | Prometheus text format |

With `[admin]` TLS configured, these are also served, as JSON, to clients whose
certificate (from the internal CA) names a principal with the role `admin` in
`permissions.toml` and whose certificate is not in `deny_certificates`; anyone
else gets 403. A kill names the admin principal in its ledger event (`by`). Every endpoint is then served over
HTTPS only, so probes and scrapers use `https`; they need no client
certificate. Without admin TLS the admin endpoints answer 404.

| request | answer |
|---------|--------|
| `GET /v1/sessions?limit=<1..1000>&after=<namespace id>` | This instance's node sessions in namespace order, 100 by default, with `next` to continue from. |
| `GET /v1/sessions/<namespace id>` | One session: identity, epoch, tenant, address, certificate, times, quarantine, schema bundle and its connected clients. |
| `GET /v1/sessions/<namespace id>/provenance` | Every capability each client holds on the session, with the call that produced it and its parent, back to the bootstrap. |
| `POST /v1/sessions/<namespace id>/kill` | Closes the session (`killed`); every client capability on it stops working. 202. |
| `POST /v1/clients/<session_id>/kill` | Disconnects one client and drops its capabilities. 202. |

A role used only for the admin API needs no `allow` pattern:

```toml
[[role]]
name = "admin"

[[principal]]
name = "operator-*"
roles = ["admin"]
```

## Metrics

| metric | labels | meaning |
|--------|--------|---------|
| `nightfall_sessions` | `shard` | Node sessions each shard holds. |
| `nightfall_handshakes_total` | `listener` (`fleet`, `provision`, `inner`, `relay`), `outcome` | Connections by how far they got: `ok`, `failed`, `timed_out`, `penalized`, `rate_limited`, `full`, `no_client_hello`, `unknown_server_name`, `no_proxy_header`. |
| `nightfall_calls_total` | `direction`, `action`, `result` | Calls through the membrane. |
| `nightfall_call_duration_seconds` | | Time from a call's arrival to its result. |
| `nightfall_ledger_queue_depth` | | Entries waiting for Kafka. |
| `nightfall_ledger_commit_seconds` | | Time to commit a ledger transaction. |
| `nightfall_ledger_delivery_failures_total` | | Ledger transactions Kafka did not take. |
| `nightfall_enrollments_total` | `operation`, `outcome` | Provisioning outcomes. |
| `nightfall_enrollment_rate_alert` | | 1 while enrollments are above `enrollment_alert_per_minute`. |
| `nightfall_rate_limited_total` | `limit` | Refusals by the limit that refused. |
| `nightfall_penalty_box_ips` | | Addresses being refused. |
| `nightfall_directory_remote_nodes` | | Node sessions other instances hold. |
| `nightfall_relayed_connections` | | Relays open now. |
| `nightfall_membranes_dropped_total` | `reason` | Clients disconnected by nightfall. |
| `nightfall_inflight_bytes` | | Bytes of calls in flight. |
| `nightfall_node_state_lag_seconds` | | Age of the last node state applied; 0 when caught up. |
| `nightfall_admission_refused_total` | `rule` | Calls refused because no intended process allowed them, by [rule](membrane.md#admission). |
| `nightfall_intended_processes` | | Intended processes the instance holds. |
| `nightfall_intended_processes_lag_seconds` | | Age of the last intended process record applied; 0 when caught up. |
| `nightfall_intended_processes_dropped_total` | | Intended process records dropped because the table was full. |
| `nightfall_heartbeat_misses_total` | | Heartbeats a node did not answer. |
| `nightfall_ready` | | 1 when `/readyz` would answer 200. |
| `nightfall_sessions_refused_total` | `reason` | Node sessions refused after TLS: `revoked`, `retired`, `setup_timeout`, `invalid_certificate`. |
| `nightfall_binding_conflicts_total` | | Nodes refused for a namespace id bound to another identity. |
| `nightfall_principals_refused_total` | | Clients refused for a denied certificate or principal, or no role. |
| `nightfall_shard_handoffs_total`, `nightfall_shard_handoff_failures_total` | | Client connections passed to the shard holding their node. |
| `nightfall_relay_failures_total` | | Relays to an instance that could not be reached. |
| `nightfall_unknown_server_names_total` | `listener` | Connections refused for their server name. |
| `nightfall_accept_descriptor_exhaustion_total` | | Times the listeners paused for lack of file descriptors. |
| `nightfall_invalid_messages_total`, `nightfall_consumer_errors_total` | `topic` | Kafka messages dropped as invalid, and failed polls. |
| `nightfall_census_failures_total`, `nightfall_event_delivery_failures_total` | `topic` on the second | Census writes and events Kafka did not take. |
| `nightfall_dead_instances_total` | | Instances whose census stopped. |
| `nightfall_tls_reload_failures_total`, `nightfall_permissions_reload_failures_total` | | Changed files that did not load. |
| `device_id_collision` | | Device ids seen with many installations from many addresses (cloned images). |

## Sizing

**File descriptors.** Each node session holds one, each client connection one,
each relay two, each provisioning connection one, and Kafka, step-ca and the
listeners a few dozen. At start nightfall raises its soft `RLIMIT_NOFILE` to the
hard limit, reserves 10 000 descriptors plus two for every relay
(`max_relayed_connections`) and one for every provisioning connection
(`max_provisioning_connections`) - 40 000 with the defaults - and lowers
`max_sessions` to the soft limit minus that reserve, logging the budget; raise
the hard limit where nightfall runs (`ulimits: nofile` in Docker Compose,
`LimitNOFILE=` in systemd, the container runtime's default in Kubernetes) to at
least `max_sessions` plus the reserve.

**Connection tracking.** Every node is one long-lived TCP connection. On hosts
and load balancers that track connections, keep `net.netfilter.nf_conntrack_max`
at least twice the sessions the host holds, or exempt the fleet port from
tracking (a `NOTRACK` rule for it), so a reconnect storm does not fill the table.

**Memory.** An idle node session (connected, heartbeating, no client) cost 52
KiB of resident memory, measured with 2000 sessions on a debug build with both
ends of every link in one process
(`cargo nextest run -p nightfall --test sizing --run-ignored ignored-only
--no-capture`), so nightfall's own share is below that. Clients add their
membranes and the calls they have in flight, which `[limits]
max_inflight_bytes_per_session` and `max_inflight_bytes_instance` bound; the
ledger queue holds up to `ledger.queue_entries` entries, and two more for each
node session, of up to 256 KiB each.

**CPU.** Sessions are spread over `shards` threads, one per core by default, by
whichever shard accepts them; TLS handshakes are the expensive part, which is
what `handshakes_per_second` bounds.

## Draining

On SIGTERM (or SIGINT) nightfall:

1. turns `/readyz` to 503 and stops accepting connections on the fleet and
   provision listeners. The inner and relay listeners keep accepting clients of
   the nodes it still holds until the last of them closes, and the admin listener
   keeps answering;
2. closes its node sessions in random order spread over `drain_seconds`, each
   with reason `shutdown` on `dusk.connections` and in the ledger, so the nodes
   reconnect elsewhere a few at a time. Its census keeps running and lists the
   sessions it still holds, so other instances keep relaying their clients to it;
3. once the last session has closed, or 10 seconds after the last one was told
   to close, writes a final census with no sessions, giving up after 10 seconds
   when Kafka does not take it;
4. flushes its producers, writes a final signed checkpoint and exits with status 0.

Give the process `drain_seconds + 60` seconds before it is killed
(`terminationGracePeriodSeconds` in Kubernetes) and replace instances one at a
time. Roll out nightfall before nodes of a new version, so the schema bundle of
every node version is already installed when its nodes connect.

## Ledger tools

`nightfall verify-ledger` and `nightfall ledger-hash` are described on
[the ledger](ledger.md#verifying) page.
