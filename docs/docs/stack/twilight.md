# Running twilight

This page is for the people who run the Dusk stack: how to start twilight, how to
configure it, how people and programs sign in to it, what it needs from
Postgres and Kafka, and what it guarantees while it runs. Writing campaigns is
covered in [Campaigns](campaigns.md).

twilight is the orchestration layer of the Dusk stack. It keeps the inventory of
every node in Postgres, follows which nodes are online through the connection
and census streams nightfall writes to Kafka, runs campaigns by asking dawn to
start processes on nodes, and compares nightfall's ledger with the processes it
intended. You run it as several
identical instances; one of them leads and does the campaign work, every one of
them follows the online view and the ledger.

## Commands

The `twilight` binary has five commands:

| Command | What it does |
|---------|--------------|
| `twilight serve [--config FILE] [--migrate]` | Runs an instance until it receives `SIGTERM` or `SIGINT`. |
| `twilight migrate [--config FILE]` | Brings the database schema up to date and exits. |
| `twilight token create --name NAME --role ROLE [--config FILE]` | Creates an API token and prints it once. `ROLE` is `viewer`, `operator` or `admin`. |
| `twilight token revoke ID [--config FILE]` | Revokes the API token with that id. |
| `twilight version` | Prints the version and the git revision it was built from. |

A command exits with 0 when it succeeds, 2 when it was called wrongly (an
unknown command or flag, a missing argument) and 1 on any other failure, with
the reason on standard error.

`token create` prints the token itself, `twilight_` followed by 43 characters,
on standard output and its id on standard error. Only its SHA-256 is stored, so
it cannot be shown again; revoke it and create another if it is lost. The
token's creator is recorded as `cli:` followed by the name of the account that
ran the command.

### Upgrading the schema

`serve` refuses to start while the database lacks migrations the binary needs,
and says to run `twilight migrate`. Run `twilight migrate` once per upgrade,
before the new instances start - in Kubernetes, as a Job or an init container
running the same image. `serve --migrate` applies them itself before serving,
which is convenient for a single instance or a development stack.

Migrations run one at a time, each in its own transaction, under a Postgres
advisory lock, so two instances migrating at once cannot interleave. A
migration already applied never changes: if the checksum recorded for an
applied migration differs from the one the binary carries, `migrate` and
`serve` stop and name it.

### Shutting down

On `SIGTERM` an instance reports itself not ready, ends every live-update
stream, stops accepting connections on `listen`, and stops its loops - a leader
gives up its leadership, consumers commit their offsets - while the API
requests already in flight finish. It waits at most `drain_seconds` for all of
that; requests still running then are cut off, and the instance exits with 1.
Kubernetes counts the `preStop` wait below inside the grace period, so set
`terminationGracePeriodSeconds` above `drain_seconds` plus that wait: with the
defaults, above 35 seconds.

The instance stops accepting connections as soon as it receives `SIGTERM`, and
a Kubernetes Service keeps sending it new ones until the endpoint removal has
reached every node. Give the pod a `preStop` hook that waits a few seconds
first; the twilight image has no shell, so use the `sleep` action, which
Kubernetes 1.30 and later provide:

```yaml title="twilight Deployment, container spec"
lifecycle:
  preStop:
    sleep:
      seconds: 5
```

## Configuration

twilight reads `/etc/twilight/twilight.yaml`, or the file `--config` names, and
then the environment: `TWILIGHT__<SECTION>__<KEY>` sets a key, overriding the
file. A list in the environment is comma-separated. The default file may be
absent; a file named with `--config` must exist. An unknown key in the file or
an unknown `TWILIGHT__` variable stops twilight from starting, so a misspelled
key never silently leaves a default in place. The configuration's problems are
reported together, not one per attempt.

```yaml title="/etc/twilight/twilight.yaml"
instance: twilight-0
database:
  url: postgres://twilight@postgres:5432/inventory?sslmode=verify-full
kafka:
  brokers: [kafka-0.kafka:9092, kafka-1.kafka:9092, kafka-2.kafka:9092]
  tls:
    ca: /etc/twilight/pki/kafka-ca.crt
    certificate: /etc/twilight/tls/kafka.crt
    key: /etc/twilight/tls/kafka.key
dawn:
  service: dawn
reconcile:
  ledger_keys: /etc/twilight/pki/ledger-keys.json
alerts:
  webhook_url: https://alerts.example.org/dusk
```

The same `instance` from the environment is `TWILIGHT__INSTANCE=twilight-0`,
and the brokers are
`TWILIGHT__KAFKA__BROKERS=kafka-0.kafka:9092,kafka-1.kafka:9092,kafka-2.kafka:9092`.
In Kubernetes, set `TWILIGHT__INSTANCE` to the pod name from the downward API.

### Top level

| Key | Default | Meaning |
|-----|---------|---------|
| `instance` | the host name | This instance's name, in logs, Kafka client ids and the leadership table. It must not contain `/`. |
| `listen` | `0.0.0.0:8080` | Where the API and the web UI listen; plain HTTP unless [`tls`](#tls) is set. |
| `health_listen` | `0.0.0.0:9102` | Where `/healthz`, `/readyz` and `/metrics` are served, over plain HTTP. |
| `drain_seconds` | `30` | How long shutdown may take. |
| `log_level` | `info` | `debug`, `info`, `warn` or `error`. |

### `database`

| Key | Default | Meaning |
|-----|---------|---------|
| `url` | `postgres://twilight@postgres:5432/inventory?sslmode=verify-full` | The inventory database, as a libpq URL. |
| `leader_url` | `url` | The connection that holds the leadership lock. It must reach the primary directly, never through a connection pooler or a replica; twilight refuses a replica. |
| `max_connections` | `20` | The size of the connection pool, at least 4. |

twilight's role creates and drops partitions while it runs - one per campaign
in `campaign_nodes` and `campaign_events`, and one per day in
`intended_processes` - so it must own the schema that `twilight migrate`
creates.

### `kafka`

| Key | Default | Meaning |
|-----|---------|---------|
| `brokers` | `[kafka:9092]` | Bootstrap brokers. |
| `allow_plaintext` | `false` | Connect without TLS. Only for a development stack. |
| `tls.ca` | | The CA that signs the brokers' certificates. Required unless `allow_plaintext`. |
| `tls.certificate`, `tls.key` | | A client certificate for mutual TLS. They are read again on every connection, so a renewed certificate is used without a restart. |
| `tls.server_name` | | The name to verify the brokers' certificates against, when it differs from the broker address. |
| `sasl.mechanism` | | `SCRAM-SHA-256` or `SCRAM-SHA-512`. |
| `sasl.username`, `sasl.password_file` | | The SCRAM credentials; the password is read from the file. |
| `client_id` | `twilight` | The Kafka client id prefix; the instance name is appended. |
| `topics.connections` | `dusk.connections` | |
| `topics.census` | `dusk.census` | |
| `topics.ledger` | `dusk.ledger` | |
| `topics.enrollments` | `dusk.enrollments` | |
| `topics.node_state` | `dusk.node-state` | |
| `topics.process_results` | `dusk.process-results` | |
| `results_group` | `twilight-results` | The consumer group the leader applies process results in. |
| `inventory_group` | `twilight-inventory` | The consumer group the leader records enrollments in. |
| `reconcile_group` | `twilight-reconcile` | The consumer group every instance shares to reconcile the ledger. |
| `reconcile_results_group` | `twilight-reconcile-results` | The consumer group every instance shares to read process results for reconcile. |

twilight reads `dusk.connections` and `dusk.census` without a consumer group:
every instance reads every partition. It needs Read on its four consumer
groups and these topic ACLs:

| Topic | Operations |
|-------|------------|
| `dusk.connections`, `dusk.census`, `dusk.ledger`, `dusk.enrollments`, `dusk.process-results` | Read, Describe, DescribeConfigs |
| `dusk.node-state` | Write, Describe, DescribeConfigs |

twilight never creates topics. An instance is not ready while one of the six is
missing or its `cleanup.policy` is not the one the Kafka contracts in
`services/contracts/kafka/` give it (`compact` for `dusk.census` and
`dusk.node-state`, `delete` for the others); it checks again every 30 seconds.

### `dawn`

| Key | Default | Meaning |
|-----|---------|---------|
| `service` | `dawn` | The name of dawn's headless Service. Its addresses are resolved again every `resolve_interval_seconds`. |
| `endpoints` | | A fixed list of `host:port` dawn endpoints instead of `service`. |
| `port` | `8443` | dawn's port at each resolved address. |
| `server_name` | `dawn` | The name dawn's certificate is verified against. |
| `ca` | `/etc/twilight/pki/internal-ca.crt` | The internal CA. |
| `certificate`, `key` | `/etc/twilight/tls/client.crt`, `/etc/twilight/tls/client.key` | twilight's client certificate, read again on every handshake. |
| `allow_plaintext` | `false` | Call dawn over plain HTTP. Only for a development stack. |
| `request_timeout_seconds` | `30` | The time limit of one call to dawn. |
| `resolve_interval_seconds` | `30` | How often `service` is resolved. |

Each node is always sent to the same dawn instance while the set of dawn
endpoints stays the same: the one whose SHA-256 score of the node's
`<device>/<installation>` and the endpoint is highest. When an endpoint
disappears only the nodes it held move. An endpoint that refused a connection
is skipped for 30 seconds.

### `engine`

| Key | Default | Meaning |
|-----|---------|---------|
| `sweep_interval_seconds` | `300` | How often the leader sweeps every running campaign for matching online nodes. |
| `gate_interval_seconds` | `15` | How often the leader judges each running campaign's health gate. |
| `evaluation_queue` | `100000` | How many nodes may wait for evaluation; a connect hint beyond it is dropped and counted. |
| `dispatch_workers` | `64` | Concurrent calls to dawn. |
| `dispatch_attempts` | `3` | Attempts at a dispatch whose answer was lost, with the same pids. |
| `facts_per_second` | `50` | Facts reads asked of dawn per second, across the fleet. |
| `facts_workers` | `16` | Concurrent facts reads. |
| `facts_max_age_seconds` | `86400` | Facts older than this are read again. |
| `census_interval_seconds` | `300` | nightfall's full census interval; a revoked node still online two intervals after it was revoked raises an alert. |
| `skip_lag_records` | `200000` | Connection events behind the end before the stream skips to the latest. |
| `skip_age_seconds` | `60` | Age of the newest connection event read, while behind, before the stream skips to the latest. |
| `counters_flush_seconds` | `5` | How often the leader writes campaign counters to `campaign_counters`. |
| `process_lifetime_seconds` | `900` | How long the process of a facts read or a file collection is intended for, from when twilight asks dawn for it. |
| `intended_process_retention_days` | `31` | How long `intended_processes` and reconcile's per-pid state are kept: the ledger's 30 days plus one. |
| `presence_flush_millis` | `1000` | How often the leader writes presence changes to `node_presence`. |
| `last_seen_bucket_seconds` | `10` | The leader writes `last_seen_at` for one of 30 groups of online nodes this often, so each node's is at most 30 of these old. |

### `alerts`

| Key | Default | Meaning |
|-----|---------|---------|
| `webhook_url` | | An `http` or `https` URL every new critical or high alert is posted to. |
| `enrollment_rate_per_minute` | `600` | More enrollments than this in one minute raise a high `enrollment_rate` alert. |

The webhook receives the alert as JSON - `id`, `time`, `severity`, `kind`,
`fingerprint`, `detail` and the rest of the alert's columns - with up to five
attempts per alert. A delivery that gives up is counted in
`twilight_alert_webhook_failures_total`; the alert itself is in the `alerts`
table either way.

### `reconcile`

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `true` | Run reconcile on this instance. |
| `ledger_keys` | `/etc/twilight/pki/ledger-keys.json` | nightfall's ledger checkpoint public keys, as a JWKS. Required while reconcile is enabled. |
| `checkpoint_interval_ms` | `1000` | nightfall's checkpoint interval, which the unsigned-tail check measures against. |
| `commands_per_session` | `10000` | Shell commands (`ShPortal.sh` calls) an operator's interactive session may run before reconcile raises `process_shape`. |
| `intended_process_cache_entries` | `1000000` | Intended processes reconcile keeps in memory. |
| `default_shell.ps` | `1` | Process table reads (`ps`) dawn makes in the node's default shell for a campaign's process: one before its script, and one more after it for `ensure_*`. |
| `default_shell.state_reads` | `1` | `kvs get` reads of one reported key, made before and after the script: `ensure_config` reads `dusk.config.hash`, `ensure_version` its version key and `dusk.config.hash`. |
| `default_shell.logs_dump` | `1` | The `logs dump` dawn reads when the stream to a campaign's script breaks, to learn whether the script ended. |
| `default_shell.kill` | `1` | The `kill` dawn runs when a process's work is done, or for the pids of a reap. |
| `default_shell.reap_rounds` | `3` | The further reads dawn makes while it waits for killed processes to exit: after a facts read, a log stream, a file collection or an interactive session, and for a reap. |
| `default_shell.reaped_pid` | `0` | Further commands per pid a reap names; dawn kills and reaps every pid of a request in each read. |

dawn runs its reads, kills and reaps in the node's default shell, so reconcile
holds each shell command there (`ShPortal.sh`) to the processes twilight
intended for the node that were open at the time - created, and not yet expired,
5 s either side for clock skew. Each intended process allows the counts above:
3 shell commands for `run_script` and `quarantine`, 6 for `ensure_config`, 8 for
`ensure_version`, 4 for anything else and for a reap. A command when no process
is open raises `default_shell_without_intent`; more commands in one stretch of
open processes than they allow together raises `process_shape`.

The ledger key file holds Ed25519 public keys, for example:

```json title="/etc/twilight/pki/ledger-keys.json"
{"keys": [{"kty": "OKP", "crv": "Ed25519", "x": "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"}]}
```

A `kid`, when present, must be the key's RFC 7638 thumbprint, which is the key
id nightfall writes on every checkpoint. Keep a retired nightfall key in the
file for as long as the ledger holds checkpoints it signed (30 days).

### `tls`

TLS normally ends at the ingress in front of twilight, and these stay empty.
Set them when twilight terminates TLS itself: to serve without an ingress, or
to accept client certificates from [service callers](#client-certificates).

| Key | Default | Meaning |
|-----|---------|---------|
| `certificate`, `key` | | The server certificate (with its chain) and key, in PEM. With them `listen` serves TLS 1.2 and 1.3, HTTP/2 and HTTP/1.1. |
| `client_ca` | | The internal CA, in PEM. With it `listen` asks every caller for a client certificate and verifies one that is presented; a caller without one is still served. Needs `certificate` and `key`. |

The certificate and key are checked at most every 30 seconds, during
handshakes, and loaded again when either file's modification time changed, so
a certificate cert-manager renews in place is served without a restart. A pair
that does not load - half written, or a key that does not match the
certificate yet - leaves the previous certificate in service, logged at
`warn`, and is tried again at the next check. At startup a pair that does not
load stops `serve`. `client_ca` is read once, at startup.

### `principals`

Maps principal name patterns to roles, for callers that authenticate with a
client certificate. A pattern's `*` matches any run of characters other than
`/`; when several patterns match a name, the highest role wins. It needs
`tls.client_ca`.

```yaml title="/etc/twilight/twilight.yaml"
tls:
  certificate: /etc/twilight/tls/server.crt
  key: /etc/twilight/tls/server.key
  client_ca: /etc/twilight/pki/internal-ca.crt
principals:
  "release-automation-*": operator
  "dashboards-*": viewer
```

From the environment, a map is a comma-separated list of `pattern=role`:
`TWILIGHT__PRINCIPALS=release-automation-*=operator,dashboards-*=viewer`.

### `oidc`

Setting `issuer` turns on logins through an OpenID Connect identity provider
(see [Logging in](#logging-in)).

| Key | Default | Meaning |
|-----|---------|---------|
| `issuer` | | The issuer URL, exactly as the provider's discovery document states it. `https`, or `http` on a loopback address for a provider on the same machine. |
| `client_id` | | twilight's client id at the provider. Required with `issuer`. |
| `client_secret_file` | | A file holding the client secret, for a confidential client; read once at startup, surrounding white space removed. A public client leaves it empty. The login uses PKCE either way. |
| `redirect_url` | | twilight's callback as the browser reaches it, whose path is `/api/v1/auth/callback`, for example `https://twilight.example.org/api/v1/auth/callback`. Register exactly this URL with the provider. `https`, or `http` on a loopback address. |
| `scopes` | `[openid, profile, email]` | The scopes asked for. `openid` is always asked for. |
| `role_claim` | `groups` | The ID token claim the role is read from: a claim with exactly this name, or else a dotted path into nested claims, such as `realm_access.roles`. Its value is a string or a list of strings. |
| `role_map` | | Maps values of `role_claim` to `viewer`, `operator` or `admin`. Required with `issuer`, and it must map at least one value: there is no default role. |

```yaml title="/etc/twilight/twilight.yaml"
oidc:
  issuer: https://login.example.org/realms/fleet
  client_id: twilight
  client_secret_file: /etc/twilight/secrets/oidc-client-secret
  redirect_url: https://twilight.example.org/api/v1/auth/callback
  role_claim: groups
  role_map:
    fleet-admins: admin
    fleet-operators: operator
    fleet-readers: viewer
```

The same `role_map` from the environment is
`TWILIGHT__OIDC__ROLE_MAP=fleet-admins=admin,fleet-operators=operator,fleet-readers=viewer`.

### `sessions`

| Key | Default | Meaning |
|-----|---------|---------|
| `lifetime_seconds` | `43200` | How long a login session lasts at most, from the login. |
| `idle_seconds` | `3600` | A login session not used for this long ends. At most `lifetime_seconds`. |

Both are at least 60. A session's last use is recorded at most once a minute,
so an unused session ends `idle_seconds` after its last request at the
latest, and up to a minute sooner.

## Signing in

Every API route except the OpenAPI document and the login routes needs a caller
with a role:

| Role | May |
|------|-----|
| `viewer` | Read everything: the overview, nodes, campaigns with their nodes, events and gates, alerts, and the live-update stream; check selectors. |
| `operator` | What a viewer may, and: create, edit, start, pause, resume, abort, complete and archive campaigns; retry and resolve a campaign's nodes; set a node `active` or `quarantined` unless it is `retired` or `revoked`; open interactive sessions, stream a node's logs and collect files from it; acknowledge and resolve alerts. |
| `admin` | What an operator may, and set a node `retired` or `revoked`, change the lifecycle of a node that is `retired` or `revoked`, and retire, revoke or restore a whole device. |

A caller is identified in this order, and the first credential present
decides: an `Authorization` header, then the session cookie, then a client
certificate. A request whose `Authorization` header is wrong is refused even
when it also carries a good session cookie.

### Logging in

People log in through the OpenID Connect provider named by [`oidc`](#oidc).
The web UI sends the browser to `GET /api/v1/auth/login?return_to=<path>`;
twilight sends it on to the provider with a PKCE challenge, and when the
provider sends it back to `/api/v1/auth/callback` twilight checks that the
login started in that same browser, exchanges the code, verifies the ID token
(the provider's signature, the issuer, the audience, the expiry and the
login's nonce), reads the role, starts a session and returns the browser to
`return_to`, which must be a path on twilight's own site.

The role comes from the ID token's `role_claim`: each of its values is looked
up in `role_map`, and the highest role found wins. A user none of whose values
maps to a role gets 403 `no_role`, and the refusal is logged with the values
seen; there is no default role. A role change at the provider applies at the
user's next login.

A session is a pair of cookies: `twilight_session` (HttpOnly) and
`twilight_csrf`, both `Secure` and `SameSite=Lax`. twilight stores only their
SHA-256 digests, in the `api_sessions` table, so every instance serves every
session and a database dump holds nothing that logs anyone in. A login in
progress lives only in the browser, in the `twilight_login` cookie scoped to
the callback: its state, nonce, PKCE verifier and `return_to`, sealed with
AES-256-GCM under a key every instance shares from the `api_login_key` table,
for ten minutes. A callback whose cookie twilight did not seal, whose state
differs or whose ten minutes are over is refused, and no number of unfinished
logins keeps anyone else from logging in.
The provider is discovered on the first login, not at startup: twilight starts
while the provider is down, and a login then answers 503 `issuer_unavailable`
until it is up.

### API tokens

Programs use API tokens, made with [`twilight token create`](#commands) and
sent as `Authorization: Bearer twilight_...`. A token's role is fixed when it
is created. A token is looked up on every request, so `twilight token revoke`
takes effect on the next one; a live-update stream already open with it ends
within five minutes, when the stream ends and its client reconnects. The
caller's subject - what campaign events, alerts and intended processes name as
the actor - is `token:` followed by the token's id.

### Client certificates

Services authenticate with a client certificate from the internal CA, when
twilight terminates TLS itself with [`tls.client_ca`](#tls) set. The
certificate must carry exactly one `urn:dusk:principal:<name>` URI SAN and no
`urn:dusk:device:` or `urn:dusk:installation:` SAN; the name is the caller's
subject, and [`principals`](#principals) gives its role. A certificate that
breaks those rules, or whose name no pattern matches, is refused with 403.
twilight never reads identity from proxy headers, so a client certificate only
counts on a connection that reaches twilight's own TLS listener: route service
callers past any ingress that terminates TLS.

### Development mode

With `TWILIGHT_DEV=1` in the environment of `twilight serve`,
`GET /api/v1/auth/login` starts an `admin` session for the subject `dev` at
once, without a provider, and the session cookies lose `Secure` so they work
over plain HTTP. Since that hands the fleet to anyone who reaches the port,
`serve` refuses to start in development mode unless `listen` is a loopback
address (`127.0.0.1`, any `127.0.0.0/8` address, `[::1]` or `localhost`), and
the login also refuses callers that are not on a loopback address. Any value
other than `1` leaves development mode off. `TWILIGHT_DEV` is not a
configuration key and cannot be set in the configuration file.

## Health, readiness and metrics

On `health_listen`:

* `GET /healthz` answers 200 while the process runs.
* `GET /readyz` answers 200 only while the online view has finished loading,
  every Kafka topic is as required, the database answers and the instance is
  not shutting down; otherwise 503 with the reason.
* `GET /metrics` serves Prometheus metrics.

| Metric | What it counts |
|--------|----------------|
| `twilight_leader`, `twilight_leader_term` | 1 and the term while this instance leads. |
| `twilight_online_nodes` | Nodes the online view holds as online. |
| `twilight_view_degraded` | 1 while the online view is degraded (below). |
| `twilight_connections_skips_total` | Times the connection stream skipped to its latest offsets. |
| `twilight_evaluation_queue`, `twilight_evaluation_dropped_total` | Nodes waiting for evaluation, and connect hints dropped because the queue was full. |
| `twilight_row_transitions_total{state}` | Campaign node state changes, by the state entered. |
| `twilight_dispatch_processes_total{kind,outcome}` | Campaign processes handed to dawn, by dawn's answer. |
| `twilight_process_results_total{action_kind,status}` | Process results applied. A file collected or a log stream run for a campaign's process is counted under its own action kind and never moves the campaign's row. |
| `twilight_gate_verdicts_total{verdict}` | Health gate verdicts. |
| `twilight_facts_refresh_total{outcome}` | Facts reads asked of dawn. |
| `twilight_presence_writes_total` | `node_presence` rows written. |
| `twilight_dawn_requests_total{route,outcome}`, `twilight_dawn_request_seconds{route}`, `twilight_dawn_endpoints` | Calls to dawn and the endpoints routed to. |
| `twilight_kafka_invalid_messages_total{topic}` | Kafka records dropped because they failed their contract. |
| `twilight_kafka_refused_records_total{consumer}` | Kafka records skipped because the database refused their data; each is logged at `error` with its topic, partition and offset. U+0000 in a string from a node is replaced with U+FFFD before it is stored, so this stays at zero unless something else is wrong. |
| `twilight_alerts_open{severity,kind}`, `twilight_alert_webhook_failures_total` | Open alerts, refreshed every 30 seconds, and webhook deliveries that gave up. A kind with no open alert left drops out of the gauge. |
| `twilight_reconcile_entries_total{kind}`, `twilight_reconcile_duplicate_entries_total` | Ledger entries reconciled, and exact duplicates skipped. |
| `twilight_reconcile_findings_total{kind}` | Reconcile findings, repeats of an open alert included. |
| `twilight_unattributed_processes_total{principal}` | Processes a client created without naming their pid, by principal. Client-side argument builders start helper processes this way, so they are counted, not alerted. |
| `twilight_reconcile_lag_seconds` | How far behind the present the least advanced ledger partition is reconciled; `+Inf` while a partition has never been reconciled. Refreshed every 30 seconds. |
| `twilight_reconcile_intended_process_cache_entries` | Intended processes held in reconcile's cache. |

twilight logs JSON lines to standard output, one per event, with the ids it
concerns as fields: `campaign_id`, `device_id`, `installation_id`,
`namespace_id`, `epoch`, `pid`, `term`, `instance`.

## What twilight guarantees

### One leader at a time

One instance leads: it holds a Postgres session advisory lock on its own
connection to `leader_url` and increments `leadership.term` when it takes it.
It checks every 2 seconds, with a 2-second time limit, that it still holds the
lock, and stops leading after two failed checks. The connection sets TCP
keepalives (idle 10 s, interval 5 s, 3 probes) and a 15-second TCP user
timeout, so a lost primary is noticed within seconds rather than minutes.

Every change the leader makes to a campaign's node rows is a compare-and-set
that also requires the term to be the current one, so a leader that lost the
lock without noticing cannot change anything after the next leader took over.
A new leader starts every campaign's rate budget empty - a failover never
releases a burst on top of what the previous leader spent - and rebuilds the
campaigns' counters from their rows.

### The online view

Every instance keeps in memory which nodes are online, from nightfall's census
and connection events, and the leader writes what changes to `node_presence`.
Heartbeats are never stored; `last_seen_at` comes from the census.

* Epochs are compared per namespace: an event older than what twilight already
  knows about that namespace is ignored.
* A complete full census from a nightfall instance is the truth about that
  instance: sessions it does not list go offline, unless twilight learned them
  from a connection event newer than the census.
* A nightfall instance that has not sent a census header for three of its
  heartbeat intervals is in census timeout: the sessions it last reported go
  offline.
* When the connection stream falls more than `skip_lag_records` behind, or
  behind with its newest record older than `skip_age_seconds`, it skips to the
  latest events; the census catches the view up.

The view is degraded while any nightfall instance is in census timeout, and
after a skip until every instance has sent a full census. While it is degraded
no campaign phase advances and silent-rate gates are not judged; after a skip
the leader sweeps every campaign once the view recovers.

### Dispatch

* A connected event is a hint that a node may be due; every
  `sweep_interval_seconds`, and whenever a campaign starts, resumes or opens a
  phase, the leader also sweeps for matching online nodes, so a node that stays
  connected for weeks is reached too.
* A campaign's rate is one token bucket for the whole fleet, held by the
  leader.
* Each node has at most one dispatch with dawn at a time; the work of several
  campaigns due on one node goes in one dispatch.
* Work is a process on the node. A campaign's process runs at a pid derived
  from the campaign, the node and the attempt: the first 8 bytes, big-endian,
  of SHA-256 of `dusk-pid-v1` followed by `<campaign id>/<device id>/<installation id>/<attempt>`;
  a pid below 65536 or equal to the shell's default pid is replaced by hashing
  again with `/1`, `/2` and so on appended. Every resend of an attempt, and
  every leader, sends the same pid. dawn looks for the pid in the node's
  process table first: a process already there - running, or ended and not yet
  reaped - means the work was delivered before, and dawn reports `duplicate`
  and runs nothing. A node that restarted in between has an empty process table
  and runs the work again; that is why dawn reads an `ensure_version` node's
  reported state before it runs anything.
* Facts reads, file collections, log streams and interactive sessions run at a
  random pid. A facts read asks dawn for the version key of every
  `ensure_version` campaign with an open row on the node, so a node waiting in
  `verifying` is judged on a key outside `dusk.` too.
* Before every call to dawn that reaches a node, twilight records the pid in
  `intended_processes`: the node, the campaign and attempt, what kind of work
  it is, who asked for it, until when it is intended and how many shell
  commands may run in its own shell - for a campaign's process its script, one
  `cp` per collected file and one `logs stream`, for an interactive session
  `reconcile.commands_per_session`, for a facts read one more than the keys it
  reads (`dusk.config.hash` and those version keys), for anything else one -
  and how many shell commands it allows in the node's default shell, where
  dawn's reads, kills and reaps run (`reconcile.default_shell`). For campaign
  work that row and the node's move to `dispatching` are one transaction; a
  resend extends the row's expiry.

### Reconcile

Every instance takes a share of the `dusk.ledger` partitions and checks each
call nightfall forwarded against the processes twilight intended, verifies each
partition's hash chain and checkpoint signatures, and raises the alerts
[Campaigns](campaigns.md#reconcile-alerts) describes. The offsets, the chain
heads and the alerts of one batch are written in one transaction, so a restart
neither skips nor repeats an entry. Watch `twilight_reconcile_lag_seconds`:
`result_without_ledger` is only judged once every partition has been reconciled
ten minutes past a result.
