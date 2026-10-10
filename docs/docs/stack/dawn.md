# dawn

dawn is the client layer of the Dusk stack: it runs work on dusk nodes. twilight decides what should happen on which node; dawn connects to that node through nightfall, runs the work as a process at the pid twilight chose, and reports what happened to Kafka. It is also where an operator opens an interactive shell on a node, over REST or MCP.

This page is for two readers: the **operator** who deploys and configures dawn, and the **integrator** who writes a client of its API - twilight, or anything else that dispatches work. Everything here describes dawn 0.1.0 (`services/dawn/pyproject.toml`), on Python 3.14.

## What dawn talks to

| Peer | Direction | Protocol | What for |
|------|-----------|----------|----------|
| twilight, operators | in | HTTPS on `listen` (8443) | the API below |
| Prometheus, the OTel collector's scraper, the kubelet | in | plain HTTP on `metrics_listen` (9101) | `/metrics`, `/healthz`, `/readyz` |
| nightfall's inner listener | out | TLS 1.3 with a client certificate, Cap'n Proto RPC | a few connections per piece of work (below) |
| Kafka | out | Kafka protocol | `dusk.process-results`, `dusk.process-output`, `dusk.files` |
| S3 (Ceph RGW) | out | HTTP(S), path-style | collected files |
| the OTel collector | out | OTLP gRPC | node logs dawn was asked to stream |
| an OTLP endpoint | out | OTLP HTTP/protobuf | dawn's own traces and logs |
| an OIDC issuer | out | HTTPS | OIDC keys |

## Running dawn

```bash
dawn --config /etc/dawn/dawn.toml
```

dawn reads its configuration from the file `--config` names, else from the file in `$DAWN_CONFIG`, else from `/etc/dawn/dawn.toml` when that exists. Every key can also be set in the environment as `DAWN__<SECTION>__<KEY>` (`DAWN__<KEY>` for top-level keys), and the environment wins over the file. A value that is a list or a table is written as JSON:

```bash
export DAWN__INSTANCE=dawn-0
export DAWN__LIMITS__MAX_NODE_SESSIONS=20000
export DAWN__NIGHTFALL__ALLOWED_INNER_ADDRESSES='["nightfall-*.nightfall-inner.dusk.svc:8444"]'
```

An invalid configuration stops dawn before it does anything, with exit status 2 and every problem listed on standard error:

```
dawn: invalid configuration: ...
```

At startup dawn raises its open-file soft limit to the hard limit. It refuses to start, with exit status 1, when the hard limit is below 2 × `limits.max_node_sessions` + 1024, since a session holds up to two connections; raise the hard limit (the container's `nofile` ulimit, or `LimitNOFILE=` under systemd) or lower `limits.max_node_sessions`.

dawn logs one JSON object per line to standard output, with `time`, `level`, `logger`, `message` and the ids involved (`pid`, `campaign_id`, `attempt`, `device_id`, `installation_id`, `namespace_id`, `principal`, ...) as fields. A pid in a log line is bare lowercase hex, the way the node's own logs write it.

### Stopping

On SIGTERM or SIGINT, dawn:

1. reports not ready and answers every new call that would reach a node - dispatches, reaps, facts, log streams, file collections, interactive sessions - with 503,
2. waits up to `drain_seconds` for the dispatches and file collections already running, then stops what is still running: a script that was delivered and has not ended goes on running on the node and is reported `running` - or `ended`, when the node's logs show its end; work whose script ended keeps its result; work not yet delivered, and every file collection, is reported `error`,
3. stops every log stream and closes every interactive session,
4. stops both listeners, giving open requests up to 30 seconds, and flushes Kafka and OTLP.

Give the process at least `drain_seconds` + 60 seconds before it is killed (Kubernetes: `terminationGracePeriodSeconds`).

## Configuration

A complete `/etc/dawn/dawn.toml` for a Kubernetes StatefulSet, with every value that is not a default:

```toml
instance = "dawn-0"
output_key_file = "/etc/dawn/secrets/output.key"

[tls]
certificate = "/etc/dawn/tls/server.crt"
key = "/etc/dawn/tls/server.key"
client_ca = "/etc/dawn/pki/internal-ca.crt"

[auth]
tokens_file = "/etc/dawn/secrets/tokens.toml"

[nightfall]
default_inner_address = "nightfall-inner:8444"
allowed_inner_addresses = ["nightfall-*.nightfall-inner.dusk.svc:8444"]
server_name_suffix = "fleet.dusk.example"

[kafka]
brokers = "kafka:9092"
properties = { security_protocol = "SSL", ssl_cafile = "/etc/dawn/pki/kafka-ca.crt", ssl_certfile = "/etc/dawn/tls/kafka.crt", ssl_keyfile = "/etc/dawn/tls/kafka.key" }

[s3]
endpoint = "http://ceph:8080"
access_key_file = "/etc/dawn/secrets/s3-access-key"
secret_key_file = "/etc/dawn/secrets/s3-secret-key"

[otlp]
endpoint = "http://otel-collector:4318"
```

### Top level

| Key | Default | Meaning |
|-----|---------|---------|
| `listen` | `0.0.0.0:8443` | Address of the HTTPS API. |
| `metrics_listen` | `0.0.0.0:9101` | Address of `/metrics`, `/healthz` and `/readyz`, in plain HTTP. Keep it inside the cluster. |
| `instance` | the host name | This dawn's name: `dawn_instance` in every message it produces, `service.instance.id` in its telemetry. No slash or whitespace. |
| `drain_seconds` | `300` | How long a stopping dawn waits for running work. |
| `log_level` | `info` | `debug`, `info`, `warning` or `error`. |
| `output_key_file` | `/etc/dawn/secrets/output.key` | The key of the output digests (below). The file's bytes, without surrounding whitespace, are the key; at least 32 of them. `openssl rand -hex 32` makes one. |
| `principals` | `{"twilight-*" = "dispatcher"}` | Which certificate principals get which role (below). |

### `[tls]`

| Key | Default | Meaning |
|-----|---------|---------|
| `certificate`, `key` | none, required | The API's server certificate chain and key, PEM. |
| `client_ca` | none | The CA whose client certificates authenticate callers - the internal CA. Without it, only bearer tokens authenticate. |

The three files are checked every 30 seconds and a changed set is loaded for new connections, so a renewed certificate needs no restart. A CA removed from `client_ca` stays trusted until dawn restarts.

### `[auth]`

| Key | Default | Meaning |
|-----|---------|---------|
| `tokens_file` | none | Static bearer tokens (below). |
| `oidc_issuer` | none | An `https://` OIDC issuer whose access tokens authenticate callers. Set with `oidc_audience`. |
| `oidc_audience` | none | The `aud` those tokens must carry. |
| `role_claim` | `groups` | The token claim holding the caller's groups: a string or a list of strings. |
| `role_map` | `{}` | Which claim value gets which role, as `{ "dusk-operators" = "operator" }`. |

At least one way to authenticate must be configured: `tokens_file`, `oidc_issuer`, or `tls.client_ca` with `principals`. The issuer's keys are fetched again every 300 seconds, and at most every 30 seconds when a token names a key id dawn has not seen.

### `[nightfall]`

| Key | Default | Meaning |
|-----|---------|---------|
| `default_inner_address` | `nightfall-inner:8444` | Where dawn reaches a node whose reference names no nightfall. |
| `allowed_inner_addresses` | `[]` | The other `host:port` a node reference may name. `*` stands for one DNS label: `nightfall-*.nightfall-inner.dusk.svc:8444` allows `nightfall-3.nightfall-inner.dusk.svc:8444` and nothing outside that domain. |
| `server_name_suffix` | `fleet.dusk.example` | dawn asks nightfall for a node by the TLS server name `<namespace id>.<suffix>` and verifies nightfall's certificate for that name. |
| `ca` | `/etc/dawn/pki/internal-ca.crt` | The CA nightfall's inner certificate chains to. |
| `certificate`, `key` | `/etc/dawn/tls/client.crt`, `/etc/dawn/tls/client.key` | dawn's client certificate, carrying `urn:dusk:principal:dawn-<pod>`. Re-read on every connection. |
| `connect_timeout_seconds` | `10` | How long dawn waits for one of its 256 threads that open connections to be free, and then, once one is, how long it waits for the connection to a node to come up - connected through nightfall and holding its shell server on the node. No free thread in time is a 503, or an `error` result with `delivered: false` for dispatched work; no connection in time is `unreachable`. A connection that comes up later is closed as soon as it does. |

### `[kafka]`

| Key | Default | Meaning |
|-----|---------|---------|
| `brokers` | `kafka:9092` | Bootstrap servers, comma-separated. |
| `allow_plaintext` | `false` | dawn refuses a `security_protocol` other than `SSL` or `SASL_SSL` unless this is `true`. For development only. |
| `properties` | `{}` | Options passed to aiokafka 0.14.0's `AIOKafkaProducer`, such as `security_protocol`, `sasl_mechanism`, `sasl_plain_username`. `ssl_cafile`, `ssl_certfile` and `ssl_keyfile` name PEM files dawn builds the TLS context from, checked every 30 seconds and loaded again for new broker connections when they change; `sasl_plain_password_file` names a file holding the SASL password, read once at startup. |
| `topics.process_results`, `topics.process_output`, `topics.files` | `dusk.process-results`, `dusk.process-output`, `dusk.files` | Topic names. |

dawn always produces with `acks=all`, idempotence on, zstd compression and a 5 ms linger. It creates no topics.

### `[s3]`, `[otlp]`, `[collector]`

| Key | Default | Meaning |
|-----|---------|---------|
| `s3.endpoint` | AWS | The S3 endpoint, `http://` or `https://`. Requests are path-style. |
| `s3.region` | `us-east-1` | |
| `s3.bucket` | `dusk-files` | Where collected files go. Give it a lifecycle rule that aborts incomplete multipart uploads: dawn aborts the ones it gives up on, but a dawn that is killed cannot. |
| `s3.access_key_file`, `s3.secret_key_file` | none | Files holding the credentials; without them, the AWS default chain. |
| `s3.ca` | system roots | The CA the endpoint's certificate chains to. |
| `otlp.endpoint` | none | Base URL of an OTLP HTTP endpoint (`http://otel-collector:4318`) for dawn's own traces and logs. dawn adds `/v1/traces` and `/v1/logs`. Without it, dawn exports no telemetry. |
| `collector.endpoint` | `otel-collector:4317` | The OTLP gRPC `host:port` node logs are streamed to. |
| `collector.allowed_endpoints` | `[]` | Other `host:port` a log stream request may name. |

### `[limits]`

| Key | Default | Meaning |
|-----|---------|---------|
| `max_node_sessions` | `10000` | Node sessions dawn holds at once: one per node with dispatches or reaps, one per facts read, log stream, file collection and interactive session. A session holds up to two connections to its node at a time (below). Beyond it, requests get 503. |
| `max_log_streams` | `256` | Log streams at once. Beyond it, 429. |
| `max_concurrent_uploads` | `64` | Files collected at once, together with `max_staged_bytes`: dawn collects at most this many, and at most `max_staged_bytes` / `max_file_bytes`. A file collection asked for through `/v1/files` beyond it gets 429; files dispatched work collects wait for a slot. |
| `max_staged_bytes` | `201326592` | Room in dawn's temporary directory for the files it collects (below). At least `max_file_bytes`. The default fits a 256 MiB temporary volume. |
| `max_work_per_node` | `32` | Pieces of work one dispatch may hold (a larger one is 400), and the work, or the reaps, that may wait for one node (beyond it, 429). |
| `max_queued_script_bytes` | `268435456` | Bytes of scripts, across every node, that may wait to run or be running at once. A dispatch that would pass it gets 429. At least 1048576, the largest script. |
| `process_timeout_default` | `900` | The time limit of a facts read and of a file collection asked for through `/v1/files`. |
| `process_timeout_max` | `86400` | The most a piece of work's `timeout_seconds` is honoured up to. |
| `max_log_stream_seconds` | `3600` | The longest a log stream may last. |
| `max_file_bytes` | `67108864` | The largest file dawn collects. |
| `max_output_bytes` | `1048576` | The output of one process that dawn produces to `dusk.process-output`. |
| `max_interactive_sessions` | `256` | Interactive sessions at once, counted in `max_node_sessions` as well; keep it below `max_node_sessions` so dispatched work always has sessions left. Beyond it, 503. |
| `interactive_session_seconds` | `28800` | How long an interactive session stays open before dawn closes it. |

## Authentication and roles

Every request needs a caller, except `/healthz` and `/readyz` on either port and everything on the metrics port. A caller is one of:

- **a client certificate** from `tls.client_ca`, presented in the TLS handshake. It must carry exactly one `urn:dusk:principal:<name>` URI SAN; the name is the caller, and `principals` maps it to roles by pattern (`*` matches anything). A certificate carrying a `urn:dusk:device:` or `urn:dusk:installation:` SAN - a node's certificate - is refused.
- **a static bearer token** from `auth.tokens_file`: `Authorization: Bearer <token>`.
- **an OIDC access token** from `auth.oidc_issuer`, verified against the keys the issuer's `/.well-known/openid-configuration` names, with its `iss`, `aud`, `exp`, `iat` and `sub` checked. The caller is `sub`; its roles come from `role_claim` through `role_map`. A token with no mapped value is refused.

When a request carries an `Authorization` header, the header decides and a client certificate is not looked at. dawn takes a caller's identity only from the TLS connection and the `Authorization` header: `X-Forwarded-For`, `X-Forwarded-Client-Cert` and every other header a proxy might add are ignored, so terminate TLS at dawn itself.

The tokens file, `/etc/dawn/secrets/tokens.toml`, holds SHA-256 digests, never tokens, and is re-read when it changes:

```toml
[[token]]
sha256 = "9f2c...64 lowercase hex digits..."
role = "operator"
subject = "alice@example.org"
```

Make a token and its digest with:

```bash
token=$(openssl rand -hex 32)
printf '%s' "$token" | sha256sum
```

There are three roles:

| Role | May call |
|------|----------|
| `dispatcher` | `POST /v1/dispatch`, `POST /v1/reap`, `POST /v1/facts`, `POST /v1/logs`, `DELETE /v1/logs/{id}` for any stream, `POST /v1/files`, and everything a viewer may. twilight is the dispatcher. |
| `operator` | `/v1/connect`, `/v1/disconnect`, `/v1/sh`, `/v1/sh/stream`, `/mcp`, `POST /v1/logs`, `DELETE /v1/logs/{id}` for a stream they started, `POST /v1/files`, and everything a viewer may. |
| `viewer` | `GET /v1/logs`, `GET /v1/help`, `GET /v1/help/{program}`, `GET /v1/openapi.json`, `GET /v1/docs`, `GET /v1/static/...`. |

A path this table does not name needs the `operator` role.

A refused caller gets 401 (no or unknown credentials) or 403 (no role for the call), with `{"error": "..."}`.

## API

The API lives under `/v1`, takes and returns JSON, and describes itself: `GET /v1/openapi.json` is its OpenAPI 3.1 document, and `GET /v1/docs` renders it with Swagger UI, served from dawn itself. Errors are `{"error": "..."}`; a body that does not match its schema is 400.

### Node references and pids

Every call that reaches a node names it with a **node reference**:

```json
{
  "device_id": "0123456789abcdef0123456789abcdef",
  "installation_id": "fedcba9876543210fedcba9876543210",
  "namespace_id": "00000000000000aa",
  "nightfall": null
}
```

`device_id` and `installation_id` are 32 lowercase hex digits, `namespace_id` 16, as `dusk.connections` carries them. `nightfall` is the `host:port` of the nightfall instance holding the node, or null for `nightfall.default_inner_address`; anything else must be in `nightfall.allowed_inner_addresses` (else 400). dawn asks nightfall for the node by the TLS server name `<namespace_id>.<nightfall.server_name_suffix>`.

Every piece of work is a **process** on the node, named by its **pid**: a u64 written as a decimal string, `"11400714819323198485"`. twilight chooses it before it calls dawn. A pid is never below 65536, which twilight keeps reserved, nor the node's default shell server's pid, `17505437192229758416` (`sh.capnp`'s `defaultPid`); either is 400. dawn reaches the process at a pid with the `dusk` extension's `Dusk(..., sh_server_pid=pid)`: a shell server at that pid on the node, which runs the commands dawn sends it.

Besides the shell at the pid, dawn uses the node's **default shell server** (`Dusk(...)` without `sh_server_pid`): it runs `ps`, every read (`kvs get`, `logs dump`), every kill and every reap there. The default shell server runs the commands of several connections at once, so dawn's work on one node shares it with every other client of that node. dawn kills and reaps the shells at the pids of facts reads, log streams, file collections and interactive sessions from it once they end, and never stops it. A dispatched piece of work holds up to two connections to nightfall at once.

### `POST /v1/dispatch`

Runs work on one node. Role `dispatcher`.

```json
{
  "node": { "...": "a node reference" },
  "work": [
    {
      "pid": "11400714819323198485",
      "campaign_id": "0b6b3d2a-1c4e-4f5a-9b8c-7d6e5f4a3b2c",
      "attempt": 1,
      "kind": "ensure_version",
      "script": "...dusk shell source...",
      "timeout_seconds": 900,
      "version_key": "dusk.version",
      "desired_version": "0.2.0",
      "config_hash": null,
      "collect_facts": true,
      "collect_files": [],
      "stream_logs": null
    }
  ]
}
```

| Field | Meaning |
|-------|---------|
| `pid` | The process the work runs as. A pid appears once in a dispatch. |
| `campaign_id`, `attempt` | The campaign and its attempt (from 1) the pid was derived for, or null; dawn copies them into every message about the work. |
| `kind` | `run_script`, `ensure_version`, `ensure_config` or `quarantine`. |
| `script` | The dusk shell source the work runs. Required for every kind. |
| `timeout_seconds` | The time the whole piece of work may take, from when dawn starts it to the end of its files and logs; never more than `limits.process_timeout_max`. |
| `version_key`, `desired_version` | Required by `ensure_version`: the exact kvs key the version is read from, and the version wanted. Never `dusk.device.id`. |
| `config_hash` | Required by `ensure_config`: the `dusk.config.hash` wanted. |
| `collect_facts` | Read the node's facts after the script, into the result's `reported`. |
| `collect_files` | Up to 16 distinct paths to collect after the script. A path may hold `'` or `"`, not both. |
| `stream_logs` | `{"level": "info", "duration_seconds": 300}`: stream the node's logs to `collector.endpoint` after the script, at that level and above. |

The answer is 202 `{"accepted": [pids]}`; the work happens after. Other answers: 400 (malformed, a pid named twice, or more work than `limits.max_work_per_node`), 429 (the node already has `limits.max_work_per_node` pieces of work waiting, or the scripts waiting across dawn would pass `limits.max_queued_script_bytes`), 503 (dawn is at `limits.max_node_sessions`, or stopping).

**One dispatch at a time per node.** dawn works on a node's dispatches and reaps one after another. Those that arrive while one runs wait, and run together when it ends: first every waiting reap, then the work. A pid already waiting or running is accepted again without running twice. Within the work the order is quarantine, then `run_script`, then `ensure_config`, then `ensure_version`; within a kind, the order the work arrived in, since a piece of work carries no campaign start time.

**A pid runs once.** Each piece of work:

1. dawn runs `ps` in the node's default shell server; for `ensure_version` and `ensure_config` it also reads `version_key` and `dusk.config.hash` there, each with `kvs get <key>`, into `reported`.
2. When `ps` lists the pid - running, or exited and not yet reaped - the work was dispatched before and does not run again: the result is `duplicate`, with `delivered: false`.
3. `ensure_version` whose `version_key` already holds `desired_version`, and `ensure_config` whose `dusk.config.hash` already is `config_hash`, report `already_satisfied`, with `delivered: false`, and nothing runs.
4. Otherwise dawn connects at the pid: the node starts a shell server there, the work is delivered, and dawn produces a `started` result with `delivered: true`. When that connection fails, nothing was delivered, but the node may already have started the shell server: dawn waits for a connection it stopped waiting for to come up or fail, then kills and reaps whatever is at the pid from the default shell server, so the pid can be sent again. When the pid stays in the process table, the result is `error` with `delivered: true`, so that the work is sent again under another pid.
5. The script runs in that shell server. Every value it produces goes to `dusk.process-output`; when it ends, the work `succeeded`, or `failed` when the script failed on the node.
6. In the default shell server, `ensure_version` reads `version_key` again and `ensure_config` reads `dusk.config.hash`, into `reported`; with `collect_facts`, the facts (below) replace `reported`.
7. In the shell at the pid, each of `collect_files` is collected (below), with a `collect_file` result each that carries the work's pid, `campaign_id` and `attempt`; then, with `stream_logs`, the node's logs stream for `duration_seconds`, or until the work's time is up.
8. dawn stops the shell at the pid with `kill <pid>` from the default shell server, within 10 seconds of its own that the work's time does not count, so a log stream that lasts until the work's time is up does not keep the shell running. It stays in the node's process table, exited, until it is reaped: that is what makes a resent pid a `duplicate`. dawn stops it because a dusk 0.1.0 node runs at most 16 processes started with `Dusk.run` at once, and a shell server left running would hold one of them.
9. One final result, the last message about the pid: the `collect_file` results of its files come before it.

Steps 6 and 7 run only when the script succeeded or failed, step 8 also when it `ended`. A step that fails after the script's result leaves the result as it was. **When the connection breaks while the script runs**, dawn reads the node's logs in the default shell server with `logs dump --replay-only -l info` and looks for the end of the `sh_exec` span of the shell at the pid. Without one, the result is `running`: the script may still be running, and the shell at the pid keeps running until it is reaped. With one, the result is `ended`: the script finished, but the logs do not say how. A script that timed out keeps its shell server running as well. A node that restarted has an empty process table, so a pid sent to it again runs again; a script that must not run twice checks the state it would change first. One dawn works on a node's dispatches one at a time, but two dawn instances given the same pid at the same moment - twilight sending again a dispatch whose answer it did not get, to another instance during a rollout - can both find the pid missing from `ps`: the second then reaches the first one's shell server at the pid and runs the script in it again.

### `POST /v1/reap`

Kills and reaps processes on one node, once twilight will never send their pids again. Role `dispatcher`.

```json
{ "node": { "...": "a node reference" }, "pids": ["11400714819323198485"] }
```

Up to 256 pids. The answer is 202 `{"accepted": [pids]}` (400 when `pids` is empty or malformed, 429 when the node already has `limits.max_work_per_node` reaps waiting, 503). The reap waits its turn with the node's dispatches. In the node's default shell server, dawn stops each process that still runs with `kill <pid>` and reaps it once it has exited with `kill --signal 8 <pid>`, looking at `ps` in between, up to four times, waiting a little longer each time while none of them has exited. Each pid gets one result with `action_kind: reap`: `reaped` when it is out of the process table - a pid with nothing at it needs nothing - else `error` ("the process did not exit"), or `unreachable` when dawn could not reach the node. Once a pid is reaped, sending it again runs the work again.

### `POST /v1/facts`

Reads a node's facts and answers with them. Role `dispatcher`.

```json
{ "node": { "...": "a node reference" }, "pid": "11400714819323198485", "version_keys": ["myapp.version"] }
```

`version_keys` (optional, up to 16) are exact kvs keys to read besides the `dusk.*` ones; the first is `reported.version_key`, else `dusk.version`. The answer, 200:

```json
{
  "facts": { "dusk.version": "0.1.0", "dusk.os.locale": "en_US.UTF-8", "...": "..." },
  "reported": {
    "version_key": "dusk.version",
    "version": "0.1.0",
    "config_hash": "...",
    "services": ["nightfall", "sh[server]"],
    "facts": { "...": "the same facts" }
  }
}
```

dawn reads them in a shell at `pid`, which it kills and reaps afterwards: first `kvs get dusk.; ps`, then `kvs get <key>` for `dusk.config.hash` and each of the `version_keys` the first line did not name. A `kvs get` of a key the node does not have fails before it runs, so each of those is a line of its own, and a key the node does not have is left out. The facts are every kvs key whose name starts with `dusk.` and that the `dusk` extension dawn runs knows by name - the keys the programs built into it register - and `dusk.config.hash` and the `version_keys`, found by their exact names; a key a `kvs get` of an exact name answers besides it is dropped as it is read. `reported.services` names the processes the node runs (state `R` or `RR` in `ps`), other than the `ps` that listed them. A node answering more than 4 MiB for one line is an `error`. **`dusk.device.id` is never among the facts**: it is dropped as it is read, before anything is answered or produced. The same result goes to `dusk.process-results` with `action_kind: collect_facts`. Other answers: 403 (nightfall refused), 502 (nightfall could not reach the node), 503, 504 (the node did not answer within `limits.process_timeout_default`).

### `POST /v1/logs`, `GET /v1/logs`, `DELETE /v1/logs/{stream_id}`

```json
{ "node": { "...": "a node reference" }, "pid": "11400714819323198485", "level": "info", "duration_seconds": 600, "endpoint": null }
```

Streams the node's logs at `level` (`error`, `warn`, `info`, `debug`, `trace`) and above to an OpenTelemetry collector over OTLP gRPC for `duration_seconds`: to `collector.endpoint`, or to `endpoint` when it is in `collector.allowed_endpoints` (else 400). The stream runs as `logs stream otlp://<endpoint> -l <level>` in a shell at `pid`, which dawn kills and reaps when the stream ends. Answers 202 `{"stream_id": "..."}` - the same id again when the same request comes while its stream runs; 400 when `duration_seconds` exceeds `limits.max_log_stream_seconds`; 429 at `limits.max_log_streams`; 503 at `limits.max_node_sessions`.

`GET /v1/logs` lists the running streams with their node, level, endpoint, start, end and the caller that started each. `DELETE /v1/logs/{stream_id}` stops one and answers 204 once the node has stopped sending, or 404 when there is no such stream or an operator asks to stop a stream somebody else started. When a stream ends, a `stream_logs` result goes to `dusk.process-results`.

### `POST /v1/files`

```json
{ "node": { "...": "a node reference" }, "pid": "11400714819323198485", "path": "/var/log/syslog", "campaign_id": null }
```

Collects one file, in a shell at `pid` that dawn kills and reaps afterwards, and answers 202 `{"upload_id": "..."}` at once - the same id again when the same node, pid and path come while that file is being collected. A path may hold `'` or `"`, not both (else 400).

A file collection - this one, and each of dispatched work's `collect_files` - copies the file from the node with the shell's `cp :<node path> <file on dawn>`, into a file of its own in dawn's temporary directory (`$TMPDIR`, else `/tmp`), then uploads it to S3: one `PutObject` for a file of 8 MiB or less, else a multipart upload of 8 MiB parts, holding one part in memory at a time. It checks the SHA-256 `cp` reports against the one it computes before it stores anything, and deletes the copy on dawn whatever happens. dawn copies at most `limits.max_staged_bytes` / `limits.max_file_bytes` files at once, so the temporary directory needs room for `limits.max_staged_bytes`, and for what one copy writes in the quarter of a second before dawn sees it pass `limits.max_file_bytes`. dawn watches the copy grow, and stops one that passes `limits.max_file_bytes` by closing its connection to the node: within dispatched work, that ends the work's later file collections and log stream with `error`. The object key is

```
files/<device_id>/<installation_id>/<pid>/<index>-<file name>
```

where `<index>` is the path's position in `collect_files` (0 for `/v1/files`). When the file is stored, one `dusk.files` message names it. A collection that fails - the node refuses, the file is larger than `limits.max_file_bytes`, the digests disagree, S3 fails, time runs out, dawn stops - aborts its multipart upload and stores nothing. Time running out or dawn stopping while the file is copied closes the connection, as above. Either way one `collect_file` result follows.

### Interactive sessions: `/v1/connect`, `/v1/sh`, `/v1/sh/stream`, `/v1/disconnect`, `/mcp`

These are dusk.gw's API (see [API gateway](../features/gateway.md)), with one difference: `POST /v1/connect` takes a node reference and a pid instead of a host and port, and the MCP `connect` tool takes the node reference's fields and the pid as arguments.

```json
{ "node": { "...": "a node reference" }, "pid": "11400714819323198485" }
```

It answers `{"descriptor": "a3f91c07"}` (or 403 when nightfall refused; 409 when the pid already has an open session in this dawn; 502; 503, also when dawn holds `limits.max_interactive_sessions`). Commands sent with the descriptor run in the shell at the pid, so functions defined by one command are there for the next. A descriptor belongs to the caller that opened it, and an MCP session to the caller that started it, by the way they authenticated as well as by their name. The session closes on `/v1/disconnect`, when the MCP session ends, or `limits.interactive_session_seconds` after it opened; then one `interactive` result goes to `dusk.process-results`, and dawn kills and reaps the shell at the pid.

### Health

`GET /healthz` answers 200 `{"status": "alive"}` while dawn runs. `GET /readyz` answers 200 when Kafka takes dawn's messages and it is not stopping, else 503; its body names each check:

```json
{"ready": false, "checks": {"kafka": false, "accepting": true}}
```

Both answer on the API port and the metrics port.

## What dawn produces

### Kafka

Every message follows its schema in `services/contracts/kafka/` and carries `schema`, a UUIDv7 `id` and a `time` in RFC 3339 UTC with nanoseconds. The key of every message is `<device_id>/<installation_id>`.

**`dusk.process-results/v1`.** For dispatched work that runs, a `started` message when the shell server at its pid is up, a `collect_file` message for each file it collects, and one final message, whose `action_kind` is the work's `kind` and which is always the last message about the pid. A `collect_file` message carries the work's pid, `campaign_id` and `attempt` but says only how that file went: the work's own result is the message whose `action_kind` is the work's `kind`. For any other work, reap, facts read, file collection, log stream and interactive session, one final message. `pid` is the decimal string the call named. `status`:

| Status | When |
|--------|------|
| `started` | The shell server at the pid is up and takes the script. `finished_at` is null. |
| `succeeded`, `failed` | The script ran and succeeded or failed on the node. |
| `duplicate` | The pid was in the node's process table already. Nothing ran. |
| `running` | The connection broke, or dawn stopped, while the script ran, and the node's logs show no end of it. |
| `ended` | The connection broke, or dawn stopped, while the script ran, and the node's logs show it ended, but not how. |
| `already_satisfied` | `ensure_version` or `ensure_config` found the value already in place. Nothing ran. |
| `unreachable` | dawn could not get a connection to the node through nightfall: it was refused, or not up within `nightfall.connect_timeout_seconds`. |
| `timed_out` | A time limit passed: the work's `timeout_seconds` for dispatched work and the files it collects, `limits.process_timeout_default` for a facts read or a file collection asked for through `/v1/files`. |
| `denied` | nightfall refused a call: its permissions do not allow it, or no process twilight intends on the node allows it (`denied: not intended`). |
| `reaped` | `action_kind` is `reap` and the process is out of the node's process table. |
| `error` | Anything else, with the reason in `error`. |

`delivered` says whether this call started the shell server at the pid: from `started` on for dispatched work, once the shell at the pid is up for a facts read, file collection, log stream or interactive session. It is false for `duplicate`, `already_satisfied` and every reap. `timed_out` and `error` results carry it too.

`output_count` counts every value the script produced; `output_truncated` says whether some never reached `dusk.process-output`: the output passed `limits.max_output_bytes`, one value was larger than a Kafka message, or Kafka did not take a message.

`output_digest` is hex HMAC-SHA256, keyed with `output_key_file`, over each value the script produced in order, every one encoded as JSON with sorted keys, no whitespace (`separators=(",", ":")`), non-ASCII characters as they are, and followed by a newline. It covers the values that were cut off too, so whoever holds the key can check output collected elsewhere against it.

**`dusk.process-output/v1`.** One message per value a script produced, `index` from 0, `value` as JSON the way dusk.gw renders it. When the output passes `limits.max_output_bytes`, one last message with `truncated: true` and `value: null` follows the last value kept, and nothing after it.

**`dusk.files/v1`.** One message per stored file: the pid, node path, bucket, object key, size, SHA-256 and `application/octet-stream`.

When Kafka does not take a message within 30 seconds, dawn logs it at `error` with its topic, key, id and pid, counts it in `dawn_kafka_produce_failures_total`, and reports not ready until Kafka takes a message again. A `dusk.process-results` or `dusk.files` message is sent again, with the same `id`, after a random wait of up to 1, 2, 4... seconds, at most 60, until Kafka takes it or dawn stops; the work it reports waits for it, holding its node. A `dusk.process-output` message is not sent again: the result still counts and digests it, and its `output_truncated` is true. A process result larger than one Kafka message (1 000 000 bytes) is shortened before it is sent: its `error` to 16384 characters, then `reported.facts` to null, then `reported` to null, and dawn logs it at `warn`.

### Metrics

On `metrics_listen`, `GET /metrics`:

| Metric | Type | Labels |
|--------|------|--------|
| `dawn_requests_total` | counter | `route`, `method`, `status` |
| `dawn_request_duration_seconds` | histogram | `route` |
| `dawn_process_results_total` | counter | `action_kind`, `status` (final results only) |
| `dawn_node_sessions` | gauge | |
| `dawn_log_streams` | gauge | |
| `dawn_upload_bytes_total` | counter | |
| `dawn_kafka_produce_failures_total` | counter | `topic` |

### Traces and logs

With `otlp.endpoint` set, dawn exports a span per request and per dispatched piece of work, and every log line, over OTLP HTTP/protobuf, as service `dawn` with `service.instance.id` = `instance`.

## Measuring sessions per process

A dawn process holds every node connection it opens in one Python process, and makes them on a pool of 256 threads. The load test in `services/dawn/tests/test_load.py` opens 10 000 idle connections the way dawn does and reports the threads and resident memory they cost, so a deployment can size `limits.max_node_sessions` and the pod's memory from a measurement. Run it from `services/dawn` against a node; `uv run` builds the `dusk` extension from the repository first, which needs the Rust toolchain the repository pins:

```bash
DAWN_LOAD_NODE=127.0.0.1:9090 \
DAWN_LOAD_REPORT=load.json \
uv run pytest -m load -s
```

or against nightfall's inner listener, adding `DAWN_LOAD_SERVER_NAME=<namespace id>.<server name suffix>`, `DAWN_LOAD_CA`, `DAWN_LOAD_CERTIFICATE` and `DAWN_LOAD_KEY`; the `dusk` extension must take those as keyword arguments. Every connection attaches to the node's default shell server. `DAWN_LOAD_SESSIONS` (default 10000) and `DAWN_LOAD_HOLD_SECONDS` (default 30) change its size. The hard open-file limit must be at least twice the session count + 1024. The report holds `threads_before`, `threads_held`, `rss_kib_before`, `rss_kib_held` and `rss_kib_per_session`; the test fails when the connections cost more threads than one per core, the pool's 256 and 32 more.
