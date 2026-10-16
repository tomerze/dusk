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

The `twilight` binary has these commands:

| Command | What it does |
|---------|--------------|
| `twilight serve [--config FILE] [--migrate]` | Runs an instance until it receives `SIGTERM` or `SIGINT`. |
| `twilight migrate [--config FILE]` | Brings the database schema up to date and exits. |
| `twilight token create --name NAME --role ROLE [--config FILE]` | Creates an API token and prints it once. `ROLE` is `viewer`, `operator` or `admin`. |
| `twilight token revoke ID [--config FILE]` | Revokes the API token with that id. |
| `twilight node-state rebuild [--config FILE] [--stall-seconds N] [--memory-limit MIB]` | Writes `dusk.node-state` again from the inventory and prints what it did ([Rebuilding the node-state topic](#rebuilding-the-node-state-topic)). |
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

Every row of `nodes`, `node_presence` and `campaign_nodes` carries the
partition of `topics.process_results` its node's results go to: the
murmur2 hash of `<device id>/<installation id>`, modulo the topic's partition
count, which is how Kafka's default partitioner, and with it dawn's producer,
places them. The migration that adds that column (`0018_node_partitions`)
needs the partition count, so while it is pending `migrate` and `serve
--migrate` read it from Kafka, waiting up to 5 minutes for the topic to exist:
run them where Kafka is reachable, after the topics are created. No other
migration needs Kafka.

That migration rewrites `nodes`, `node_presence` and `campaign_nodes` to compute
every row's partition, and holds an exclusive lock on the three tables until it
commits, so every query a running instance makes on them waits for it.
Computing a partition took about 18 µs a row on a test host under a load
average near 90: for 1,000,000 nodes and one campaign of 1,000,000 rows, about a
minute of hashing, besides rewriting the tables and building their indexes. On
a large inventory, run it with twilight scaled to zero or in a maintenance
window.

The partition count is fixed from then on. While the topic has another number
of partitions, every instance logs it at `error` on each topic check, every 30
seconds, and sets `twilight_process_results_partitions_mismatched` to 1, and
the leader writes no campaign row, advances no phase and dispatches nothing;
the API keeps answering. Kafka cannot take partitions away from a topic, so
the way back from a grown topic is to recreate it with the count the inventory
was migrated with, which loses the results still on it. twilight has no way to
change the count once migrated.

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
| `max_streams` | `1000` | Live-update streams (`GET /api/v1/stream`) one instance serves at once. One more is refused with 503 and `Retry-After: 30`. |
| `max_requests` | `8` | API requests one instance answers at once, live-update streams aside. A request waits for a free slot until its deadline, then is answered 503 `busy` with `Retry-After: 1`, so a burst of slow requests can never take the database connections the engine needs. |
| `request_timeout_seconds` | `15` | How long one API request may take, waiting for a slot included. A request still running then is cancelled, its database queries with it, and answered 503 `timeout`. |

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
| `topics.intended_processes` | `dusk.intended-processes` | |
| `results_group` | `twilight-results` | The consumer group in which the leader owns the partitions of `topics.process_results`, the inventory's slices, and applies the process results that land in them. |
| `inventory_group` | `twilight-inventory` | The consumer group the leader records enrollments in. |
| `reconcile_group` | `twilight-reconcile` | The consumer group every instance shares to reconcile the ledger. |
| `reconcile_results_group` | `twilight-reconcile-results` | The consumer group every instance shares to read process results for reconcile. |

twilight reads `dusk.connections` and `dusk.census` without a consumer group:
every instance reads every partition. It needs Read on its four consumer
groups and these topic ACLs:

| Topic | Operations |
|-------|------------|
| `dusk.connections`, `dusk.census`, `dusk.ledger`, `dusk.enrollments`, `dusk.process-results` | Read, Describe, DescribeConfigs |
| `dusk.node-state`, `dusk.intended-processes` | Write, Describe, DescribeConfigs |

twilight never creates topics. An instance is not ready while one of the seven is
missing or its `cleanup.policy` is not the one the Kafka contracts in
`services/contracts/kafka/` give it (`compact` for `dusk.census`,
`dusk.node-state` and `dusk.intended-processes`, `delete` for the others); it
checks again every 30 seconds.

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
| `reap_interval_seconds` | `300` | How long the reap sweep takes to look at every online node once, one of 30 groups at a time. |
| `reaps_per_second` | `20` | Reap requests sent to dawn per second, across the fleet. |
| `reap_retry_seconds` | `600` | How long a pid dawn was asked to reap waits before it is asked again, when dawn never reported it reaped. |
| `revocations_per_second` | `1000` | Installations a [revocation](#revocations) revokes per second, across every running revocation. |
| `node_state_stall_seconds` | `120` | How long a partition of `dusk.node-state` may read nothing, short of its end, before a [rebuild](#rebuilding-the-node-state-topic) fails. |

### `alerts`

| Key | Default | Meaning |
|-----|---------|---------|
| `enrollments.window_seconds` | `300` | The window the [enrollment alerts](#enrollment-alerts) count in. |
| `enrollments.baseline_days` | `7` | How many earlier days the baseline averages the same hour of. |
| `enrollments.baseline_factor` | `4` | How many times its baseline a count must pass, besides its floor, to raise an alert. |
| `enrollments.escalation_factor` | `2` | The first time an open alert's count reaches this many times the count it opened with, the alert is raised to critical; after that its detail takes the new count each time the count reaches this many times the count it last recorded. At least 2. |
| `enrollments.resolve_after_seconds` | `900` | How long a count must stay at or below its threshold before its alert resolves itself. |
| `enrollments.issued_floors.fleet`, `.credential`, `.network` | `100`, `30`, `20` | The least count of new installations in a window that raises `enrollment_rate`, across the fleet, for one credential and for one network. |
| `enrollments.refused_floors.fleet`, `.credential`, `.network` | `300`, `60`, `60` | The same for refused enrollments and `denied_enrollments_spike`. |
| `enrollments.new_network_floors.fleet`, `.credential` | `50`, `20` | The same for installations from networks new to the baseline period and `enrollment_from_new_network_burst`, across the fleet and for one credential. |
| `public_url` | | twilight's address as people open it, such as `https://twilight.example.org`. Every notification links its alert as `<public_url>/alerts?alert=<id>`. Required with `receivers`. |
| `runbook_url` | | Where the [runbooks](runbooks/index.md) are published, with `{kind}` where the alert's kind goes, such as `https://docs.example.org/stack/runbooks/{kind}/`. Every notification links its kind's runbook. Required with `receivers`. |
| `delivery_horizon_seconds` | `86400` | How long twilight keeps retrying a notification before it gives up on it. |
| `receivers` | none | Where notifications go: each receiver has a `name` (lowercase letters, digits and dashes, at most 63 characters) and exactly one of `pagerduty`, `slack`, `email`, `teams` or `webhook`. |
| `routes` | none | Which alerts each receiver gets. A receiver no route names stops twilight from starting. |

`receivers` and `routes` are set in the file only; `TWILIGHT__ALERTS__RECEIVERS`
and `TWILIGHT__ALERTS__ROUTES` are refused. Every secret a receiver needs - a
PagerDuty routing key, a Slack or Teams webhook URL, an SMTP password, a webhook
signing secret - comes from a file. twilight reads each one when it starts, and
does not start while one is missing or empty, then again for every attempt, so
a replaced file is used without a restart; a change to `receivers` or `routes`
themselves takes a restart. A secret never appears in a log line, a metric, the
API or a delivery's `last_error`.

```yaml title="/etc/twilight/twilight.yaml"
alerts:
  public_url: https://twilight.example.org
  runbook_url: https://docs.example.org/stack/runbooks/{kind}/
  receivers:
    - name: on-call
      pagerduty:
        routing_key_file: /run/secrets/alert-receivers/pagerduty-routing-key
    - name: ops-chat
      slack:
        webhook_url_file: /run/secrets/alert-receivers/slack-webhook-url
    - name: soc-mail
      email:
        host: smtp.example.org
        username: dusk-alerts
        password_file: /run/secrets/alert-receivers/smtp-password
        from: dusk-alerts@example.org
        to: [soc@example.org]
    - name: noc
      teams:
        webhook_url_file: /run/secrets/alert-receivers/teams-webhook-url
    - name: siem
      webhook:
        url: https://siem.example.org/dusk/alerts
        secret_file: /run/secrets/alert-receivers/webhook-secret
  routes:
    - receiver: on-call
      severities: [critical]
    - receiver: ops-chat
      severities: [high, medium, low]
    - receiver: soc-mail
      tenants: [acme]
    - receiver: noc
      kinds: [process_without_intent, default_shell_without_intent, ledger_chain_broken]
    - receiver: siem
```

`alerts.webhook_url` is gone, and twilight does not start while it is set. A
`webhook` receiver and a route take its place, with `public_url` and
`runbook_url` beside them:

```yaml
alerts:
  public_url: https://twilight.example.org
  runbook_url: https://docs.example.org/stack/runbooks/{kind}/
  receivers:
    - name: webhook
      webhook:
        url: https://hooks.example.org/dusk
        secret_file: /run/secrets/alert-receivers/webhook-secret
  routes:
    - receiver: webhook
      severities: [critical, high]
```

The receiver then gets [`dusk.alert-notification/v1`](#the-webhook), signed,
for every opening, escalation, acknowledgement and resolution of a critical or
high alert, where `webhook_url` got the bare alert when it opened.

#### Receivers

| Receiver | Key | Default | Meaning |
|----------|-----|---------|---------|
| any | `rate_per_minute` | `120` for `pagerduty`, `60` for `slack`, `teams` and `email`, `600` for `webhook` | The most notifications twilight sends the receiver in a minute, counted across all instances. |
| `pagerduty` | `routing_key_file` | | A file holding the integration key of a PagerDuty service's Events API v2 integration. |
| | `url` | `https://events.pagerduty.com/v2/enqueue` | The Events API v2 endpoint. |
| `slack` | `webhook_url_file` | | A file holding a Slack incoming webhook URL. |
| `teams` | `webhook_url_file` | | A file holding the URL of a Microsoft Teams Workflows webhook, such as one made from the *Send webhook alerts to a channel* template. |
| `email` | `host` | | The SMTP server. |
| | `port` | `587` with `starttls`, `465` with `tls`, `25` with `none` | |
| | `security` | `starttls` | `starttls` (refuses a server that does not offer STARTTLS), `tls` (TLS from the first byte) or `none` (plaintext, without credentials; for a relay on the same host or a test sink only). The server's certificate is verified against the system's certificate authorities; set the environment variable `SSL_CERT_FILE` to a PEM bundle to trust a private one. |
| | `username`, `password_file` | | Sign in with PLAIN, or LOGIN when the server offers only that. Set together, never with `none`. |
| | `from` | | The sender, one bare address. |
| | `to` | | The recipients, bare addresses. |
| `webhook` | `url` | | An `http` or `https` URL. |
| | `secret_file` | | A file holding the secret the body is signed with ([below](#the-webhook)). |

#### Routes

Each route names a `receiver` and may narrow what it gets with lists:

| Key | Matches |
|-----|---------|
| `severities` | `critical`, `high`, `medium`, `low` |
| `kinds` | alert kinds, such as `process_without_intent` (the [runbooks](runbooks/index.md) list every kind) |
| `tenants` | the alert's tenant: the tenant of the node it names (`device_id` and `installation_id` in its detail) or of the campaign it names, or for an [enrollment alert](#enrollment-alerts) the tenant of its credential. An alert with no tenant never matches a route that lists tenants. |

An absent or empty list matches everything. An alert goes to the receiver of
every route it matches, once per receiver even when several routes name the
same one. An acknowledgement or a resolution goes to every receiver that got
the alert's opening or escalation, whatever the routes say by then.

#### What is sent, and when

Every change to an alert is a transition, and every transition is a
notification to its receivers:

| Transition | When |
|------------|------|
| `opened` | An alert is raised whose fingerprint has no open alert. |
| `re_escalated` | An open alert is raised again at a higher severity. It takes the new severity and the new detail, and loses its acknowledgement, so it waits for someone to acknowledge it again. A repeat at the same or a lower severity only counts an occurrence and sends nothing. |
| `acknowledged` | Someone acknowledges the alert. |
| `resolved` | Someone resolves the alert, or twilight does (`campaign_paused_by_gate` resolves when the campaign is resumed, aborted, completed or fails). |

twilight records each transition in Postgres (`alert_transitions`) in the same
transaction that changes the alert, with the alert as it is at that moment, so a
transition is never lost and never recorded twice, whichever code path made it,
and each of its notifications shows the alert as it was then, however often it
is raised again before the notification goes out. Within a second it becomes one row
per receiver in `alert_deliveries`, which every twilight instance delivers
from; an instance takes a row for two minutes at a time, so a row is sent by one
instance at a time, and the row of an instance that died is taken by another
once that time is up. Each receiver gets an alert's notifications in order: a
resolution waits until the opening was delivered or given up on.

A failed attempt is retried with full-jitter exponential backoff, from up to 5
seconds after the first failure to up to 10 minutes, or after the receiver's
`Retry-After` (up to an hour) when it answers 429. A network failure, HTTP 408,
425, 429 or 5xx, or an SMTP 4xx reply is retried; any other HTTP 4xx answer or
an SMTP 5xx reply gives up at once; a notification still not delivered
`delivery_horizon_seconds` after its transition gives up then. A notification
twilight gave up on is not sent again; [Alert delivery
failing](runbooks/alert_delivery_failing.md) is the Grafana rule that pages for
it.

Each receiver has a queue of its own, which each instance works with at most
four sends at a time, so a receiver that hangs or refuses holds up only its own
notifications: PagerDuty keeps paging while the mail server times out. A queue
sends the highest severity first, and within a severity the kinds take turns,
the kind with the fewest notifications waiting first, so a backlog of one kind
never holds back an alert of another. A receiver gets at most its
`rate_per_minute` notifications a minute, counted across all instances. When it
answers 429, twilight pauses the whole receiver, not only that notification,
for its `Retry-After` or the backoff above, whichever is longer. Once the pause
is recorded, no instance starts a send to that receiver, not even for a
notification it had already taken; only a send that checked the pause before it
was recorded still reaches it.

Slack, Teams and email get at most one notification per kind and transition a
minute. Whatever of one kind waits for one of them goes out together - one
alert as itself, several as one digest - and the alerts of that kind that come
in the minute after are held and sent together when it ends. A digest says how
many alerts it carries and between which times, names a few of them, and links
the kind on twilight's Alerts page, `<public_url>/alerts?kind=<kind>`. An alert
in a digest keeps its order: its resolution is never sent before its opening.
PagerDuty and webhooks get every alert on its own, since each PagerDuty incident
resolves with its alert and a webhook keeps a record of each.

A storm of 10,000 `revocation_not_enforced` alerts, all raised in one minute,
reaches each receiver like this:

| Receiver | A storm |
|----------|---------|
| PagerDuty | 10,000 incidents, two a second at the default `rate_per_minute`, so the last opens about 83 minutes after the first. An alert of another kind raised meanwhile goes out next, ahead of the storm's backlog, and a higher severity before it. PagerDuty's alert grouping can fold the incidents by `class`, which is the kind. |
| Slack, Teams | A message with the first alerts the queue finds waiting, then one digest a minute for as long as the storm lasts. The same again when they are resolved. |
| Email | The same as Slack, as mails; a digest is a thread of its own. |
| Webhook | 10,000 notifications, ten a second at the default `rate_per_minute`. |

Restarts, crashes and leader changes neither lose nor repeat a notification,
with one exception: when twilight stops after a receiver accepted a
notification and before it recorded that, the notification is sent again with
the same idempotency key. PagerDuty folds the repeat into the incident it
already has, a webhook receiver can drop it by its `Idempotency-Key`, and a mail
client by its `Message-ID`; Slack and Teams show it twice.

Every notification says what happened and links the alert in twilight and its
kind's runbook. An opening or an escalation also carries the alert's severity,
kind and tenant, its node (host name, device id and installation id), campaign
(name and id), principal and credential when it names them, when it was raised
and how many times it occurred, and its evidence: the rest of the alert's
detail.

| Receiver | What it gets |
|----------|--------------|
| PagerDuty | `trigger` for `opened` and `re_escalated`, `acknowledge`, `resolve`. `dedup_key` is the alert's fingerprint (`sha256:` and its SHA-256 when the fingerprint is longer than PagerDuty's 255 characters), so a resolution in twilight resolves the incident. `payload.severity` is `critical`, `error`, `warning` or `info` for critical, high, medium and low; `class` is the kind, `group` the tenant, `custom_details` the facts and evidence, and `links` the alert and its runbook. PagerDuty sends nothing more for an incident acknowledged there, even on a new trigger, and an acknowledgement or resolution made in PagerDuty does not reach twilight. |
| Slack | A Block Kit message: a header, what happened, the facts and the evidence, and the links. An acknowledgement or a resolution is a follow-up message saying who did it and when. |
| Teams | An Adaptive Card 1.4 with the same content and two buttons for the links. |
| Email | `multipart/alternative`, a plain-text and an HTML part, `Auto-Submitted: auto-generated`. An acknowledgement or a resolution replies to the opening (`In-Reply-To`, `References`), so mail clients show an alert as one thread. |
| Webhook | The JSON below. |

#### The webhook

twilight POSTs one JSON object per notification:

| Field | Value |
|-------|-------|
| `schema` | `dusk.alert-notification/v1` |
| `id` | A UUID per notification, the same on every attempt; also the `Idempotency-Key` header. |
| `receiver` | The receiver's name. |
| `transition`, `transition_at`, `actor` | The transition, when it happened (RFC 3339, UTC) and who made it (`null` when twilight did). |
| `severity` | The alert's severity at the transition. |
| `title`, `summary` | One line saying what happened, and the alert's message. |
| `hostname`, `campaign_name` | The node's host name and the campaign's name, when the alert names them; otherwise `null`. |
| `alert` | The alert as it was at the transition, in the shape `GET /api/v1/alerts/{id}` answers, without `deliveries`: `id`, `time`, `last_seen_at`, `occurrences`, `severity`, `kind`, `fingerprint`, `tenant`, `detail`, `acknowledged_by`, `acknowledged_at`, `resolved_by`, `resolved_at`. |
| `links` | `alert` and `runbook`: the two links. |

| Header | Value |
|--------|-------|
| `Content-Type` | `application/json` |
| `Idempotency-Key` | The notification's `id`. |
| `Dusk-Timestamp` | When this attempt was made, in Unix seconds. |
| `Dusk-Signature` | The lowercase hex HMAC-SHA256, keyed with the secret in `secret_file`, of the timestamp, a colon and the body: `<Dusk-Timestamp>:<body>`. |

The body is the same bytes on every attempt; the timestamp and the signature
are new on each. A receiver checks the signature over the raw body, refuses a
timestamp more than 300 seconds from its own clock, and drops an `id` it has
seen, so a captured request cannot be replayed:

```python title="verify.py (Python 3.10 or newer)"
import hashlib
import hmac
import time

WINDOW_SECONDS = 300


def verify(secret: bytes, headers: dict[str, str], body: bytes) -> bool:
    timestamp = headers.get("Dusk-Timestamp", "")
    if not (timestamp.isascii() and timestamp.isdigit()):
        return False
    if abs(time.time() - int(timestamp)) > WINDOW_SECONDS:
        return False
    signed = timestamp.encode() + b":" + body
    expected = hmac.new(secret, signed, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, headers.get("Dusk-Signature", ""))
```

Grafana's webhook contact point signs its own notifications the same way with
the same two headers when it is provisioned by `twilight grafana-alerting`
(below), so one check serves both; Grafana's body is Grafana's JSON, not
`dusk.alert-notification/v1`.

#### Grafana

Grafana's alert rules notify through the same receivers. `twilight
grafana-alerting [--config FILE] [--output FILE]` reads the same configuration
and writes a Grafana alerting provisioning file: a contact point per receiver,
with the receiver's name and the uid `dusk-<name>`; the notification policy,
one child policy per route matching the rule labels `severity`, `kind` and
`tenant` and letting the alert go on to the next (`continue`), then a last
child policy that matches every alert and is muted at all times by the mute
timing `dusk-unrouted`, so a Grafana alert no route matches is shown in Grafana
and sent nowhere, as twilight drops an alert no route matches (the root policy
names the first route's receiver because Grafana needs one); and the template group `dusk`,
whose titles and messages link each rule's runbook from `runbook_url` and its
`kind` label. Provisioning the policy replaces Grafana's whole notification
policy tree.

The file names each secret as `$__file{<path>}`, the path twilight reads it
from, so Grafana must see every receiver's secret file at the same path.
Grafana sends its email through its own SMTP settings: set
`GF_SMTP_ENABLED=true`, `GF_SMTP_HOST` (`host:port`), `GF_SMTP_USER`,
`GF_SMTP_PASSWORD__FILE`, `GF_SMTP_FROM_ADDRESS` and `GF_SMTP_STARTTLS_POLICY`
to the email receiver's server. The local stack and the Kubernetes overlays run
`grafana-alerting` before Grafana starts ([Deploy](deploy.md)); after a change
to the receivers or routes, run it again and restart Grafana.

### Enrollment alerts

The leader reads `dusk.enrollments` and counts new enrollments only: an
`enroll` that was `issued` is a new installation, and an `assign` or `enroll`
that was `denied` or `rate_limited` is a refusal. Renewals and errors are never
counted. Each is counted for the whole fleet, for its credential - a fleet token
or provision token entry by `credential_ref`, an install token key by
`credential_issuer` - and for
its network, the /24 of an IPv4 address or the /48 of an IPv6 one. An
installation from a network no installation came from in the `baseline_days`
before also counts as one from a new network, while that network is no older
than one window.

For every one of those, over the last `window_seconds`, twilight compares the
count with its threshold: the larger of its floor and `baseline_factor` times
its baseline, the average count of a window in the same UTC hour of each of the
last `baseline_days` days. twilight writes the counts by hour to the
`enrollment_counts` table for that, and deletes them after `baseline_days` and
one more day. A count above its threshold raises a high alert, which reaches
the receivers its routes name like every other alert:

| Kind | Counts | Fingerprint |
|------|--------|-------------|
| `enrollment_rate` | New installations | `enrollment_rate:fleet`, `enrollment_rate:credential:<kind>:<name>`, `enrollment_rate:network:<network>`, `enrollment_rate:network` |
| `denied_enrollments_spike` | Refusals | `denied_enrollments_spike:fleet`, `...:credential:<kind>:<name>`, `...:network:<network>`, `...:network` |
| `enrollment_from_new_network_burst` | New installations from new networks | `enrollment_from_new_network_burst:fleet`, `...:credential:<kind>:<name>` |
| `credential_quota_reached` | Refusals for `credential_quota_reached`; any one raises it | `credential_quota_reached:credential:<kind>:<name>` |

The detail holds the `scope`, the `credential_kind` and `credential` (and the
`tenant`) or the `network`, the `count`, the `window_seconds`, the `baseline`,
the `threshold`, and the networks and credentials the count came from with
their counts. An alert for a credential takes the credential's tenant, so
`tenants` in a route matches it. The first time an open alert's count reaches
`escalation_factor` times the count it opened with, twilight raises the alert to
critical: receivers get it as `re_escalated`, so PagerDuty pages again, and its
acknowledgement is cleared. Each later time the count reaches
`escalation_factor` times the count it last recorded, the detail takes the new
count and counts an occurrence at critical, which sends nothing. Once the count
stays at or below its threshold for `resolve_after_seconds`, twilight resolves
the alert as `twilight`, and the next rise opens a new one. A new
leader goes on with the open alerts. The counts are those of the leader since it
was elected, so a leader that took over in the middle of a burst counts the
burst from then on.

At most 10 networks of one kind have an alert of their own at a time, so
refusals from many networks cannot flood the alerts. A network that goes above
its threshold while 10 are open is named in the kind's `<kind>:network` alert
instead: its `count` is the number of such networks, `networks` lists the ones
with the most enrollments in the window, and `network_alerts` says how many
networks have an alert of their own. Like any other, it is raised to critical
the first time that number reaches `escalation_factor` times the number it
opened with, its detail takes the new number at each later step, and it
resolves once no network is left in it for `resolve_after_seconds`. A flood
from any number of networks therefore opens at most 11 network alerts of a
kind, and sends receivers no more than their openings, one escalation each and
their resolutions. twilight watches at most
100 000 networks at once and counts no new network while that many are
watched; the fleet and every credential are always counted.

An alert resolved by hand or through the API is noticed within 15 seconds, or
at its next escalation: twilight forgets the counts behind it, and the next
count above the threshold opens a new alert. Resolving the `<kind>:network`
alert forgets the counts of the networks it named.

What twilight counts while a count's alert is open, or while its network is in
the `<kind>:network` alert, is not written to `enrollment_counts`, so a burst
does not raise the baseline later windows of the same hour are compared with.

`twilight_enrollment_observations_total{signal}` counts what was counted:
`issued`, `refused` and `quota`.

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

## The API

This section is for the people who write programs against twilight: the web
UI, release automation, dashboards, chat bots.

The API is JSON over HTTP under `/api/v1` on [`listen`](#top-level).
`GET /api/openapi.json` serves its OpenAPI 3.1.1 document, without
authentication: every route with its parameters, request body, each status it
answers with and the body of each, the security schemes, and
`x-required-role`, the role each operation needs. Request and response bodies
are closed schemas (`additionalProperties: false`), so clients can be
generated from the document, and twilight's tests hold every route's answers
to it.

### Calling it

A program sends an [API token](#api-tokens):

```sh
curl -fsS -H "Authorization: Bearer $TWILIGHT_TOKEN" \
  'https://twilight.example.org/api/v1/campaigns?status=running'
```

A browser page [logs in](#logging-in) and is then authenticated by the
session cookie. Every request a cookie session makes other than a `GET` must
carry the value of the `twilight_csrf` cookie, which the page's script can
read, in an `X-CSRF-Token` header; without it, or with another session's
value, the request is refused with 403 `csrf_failed`. API tokens and client
certificates need no CSRF token.

A request without credentials, or with ones that are unknown, revoked or
expired, is answered 401 `unauthenticated` with
`WWW-Authenticate: Bearer realm="twilight"`; when logins are possible the
error's `details.login` is `/api/v1/auth/login`, where a page sends the
browser. A caller below the route's role gets 403 `forbidden` naming the role
it needs.

| Route | Role | What it does |
|-------|------|--------------|
| `GET /api/v1/auth/login?return_to=` | none | Starts a login and redirects to the identity provider; [development mode](#development-mode) starts the session at once. |
| `GET /api/v1/auth/callback` | none | Where the identity provider returns the browser. |
| `POST /api/v1/auth/logout` | viewer | Ends the caller's session and clears its cookies; 204. |
| `GET /api/v1/me` | viewer | The caller: `subject`, `name`, `role`, `authentication` (`oidc`, `token`, `mtls` or `dev`) and, for a session, `expires_at`, when it ends unless it is used again. |

### Requests and answers

* A request body is `application/json`, at most 1 MiB, exactly one JSON value.
  A body that does not match its schema - an unknown field, a misspelled key
  inside a campaign's policy, a wrong type - is refused with 400
  `invalid_argument`, each problem listed in `details.violations`, for example
  `{"at": "/policy/gates", "message": "additional properties 'max_failure_rat' not allowed"}`.
  Ranges and rules across fields are checked after that and name the field in
  `details.field`.
* A change answers the resource as it is afterwards: a campaign transition or
  edit answers the campaign with its counters, a lifecycle change the node.
* Every answer carries `X-Request-Id` and `Cache-Control: no-store`.
* A device id and an installation id are 32 lowercase hexadecimal digits, a
  campaign id a UUID, an alert id a positive integer. Times are RFC 3339.

### Errors

Every error is one shape:

```json
{"error": {"code": "gate_override_required", "message": "...", "details": {}}}
```

`code` is stable and meant for programs; `message` is for people; `details`
holds the facts that belong to the code.

| Status | `code` | When | `details` |
|--------|--------|------|-----------|
| 400 | `invalid_argument` | A parameter, a path segment or a body is wrong. | `field` or `violations` |
| 400 | `invalid_selector` | A selector does not parse. | `position`, `end` |
| 401 | `unauthenticated` | No usable credentials. | `login` |
| 401 | `login_failed` | A login did not complete. | `login` |
| 403 | `forbidden` | The caller lacks the role, or its certificate was refused. | |
| 403 | `csrf_failed` | A cookie session's change without its CSRF token. | |
| 403 | `no_role` | A login whose claims map to no role. | |
| 404 | `not_found` | No such route, node, campaign or alert. | |
| 404 | `login_unavailable` | No identity provider is configured. | |
| 405 | `method_not_allowed` | The route exists with other methods, listed in `Allow`. | |
| 409 | `version_conflict` | A campaign edit made to an older version. | |
| 409 | `not_draft` | An edit to a campaign that has started. | |
| 409 | `invalid_transition` | A transition the campaign's status does not allow. | |
| 409 | `gate_override_required` | Resuming a campaign a health gate paused, without `override_gate` and a reason. | |
| 409 | `overlap` | Starting a campaign that overlaps a running one. | `count`, `campaigns` |
| 409 | `not_archivable` | Archiving a campaign too soon after its last dispatch. | |
| 409 | `node_offline` | An action that needs the node online. | |
| 413 | `payload_too_large` | A body over 1 MiB. | |
| 415 | `unsupported_media_type` | A body that is not `application/json`. | |
| 500 | `internal` | twilight failed. The log line `api request failed` with the same `request_id` holds the cause. | `request_id` |
| 502 | `dawn_rejected` | dawn refused the request. | `dawn_status`, `dawn_message` |
| 503 | `busy`, `timeout`, `dawn_busy`, `dawn_unavailable`, `issuer_unavailable`, `too_many_streams` | Try again after `Retry-After` seconds. | `retry_after_seconds` |
| 504 | `dawn_unanswered` | dawn did not answer; the request may or may not have reached the node. | |

### Lists

Every list answers a page:

```json
{"items": [], "next_cursor": "..."}
```

Pass `next_cursor` back as `cursor` for the next page; it is `null` on the
last one. `limit` is 1 to 500, 100 by default. A cursor is opaque and belongs
to the query that made it: a node cursor made for one `sort` is refused with
400 for another. Two lists differ:

* `GET /api/v1/nodes` with `online=true` or `online=false` filters after
  reading the inventory, so a page can hold fewer than `limit` nodes while
  `next_cursor` is not `null`; keep following it.
* A campaign's events are oldest first and paged by `after`, an event id: pass
  `next_cursor` as `after`. A client polling a campaign's history passes the id
  of the last event it holds.

### Routes

| Route | Role | What it does |
|-------|------|--------------|
| `GET /api/v1/overview` | viewer | Nodes online and in total, nodes by lifecycle, the running and paused campaigns with their counters, open alerts by severity (`alerts`) and the ones nobody has acknowledged (`alerts_unacknowledged`), whether the online view is degraded and whether this instance leads. |
| `GET /api/v1/nodes?selector=&online=&sort=&limit=&cursor=` | viewer | Nodes matching a [selector](campaigns.md#which-nodes-the-selector), each with whether it is online and when it was last seen. `sort` is `device_id` (the default) or an indexed column - `hostname`, `lifecycle`, `country`, `os_name`, `os_version`, `os_build`, `dusk_version`, `hardware_class`, `tenant`, `locale`, `impl`, `target_arch`, `reported_version` - with a leading `-` for descending order; versions sort as semantic versions. |
| `GET /api/v1/nodes/{device}/{installation}` | viewer | A node with its live sessions (namespace, epoch, nightfall instance), its campaign rows, and `device`: the last lifecycle change made to its device as a whole, or `null`. |
| `POST /api/v1/nodes/{device}/{installation}/lifecycle` | operator | `{"lifecycle", "reason"}` sets the node `active` or `quarantined` (operator) or `retired` or `revoked` (admin), through `dusk.node-state`. Only an admin changes the lifecycle of a node that is `retired` or `revoked`. |
| `POST /api/v1/nodes/{device}/{installation}/sessions` | operator | `{"reason", "ttl_seconds"}` (60 to 28800) opens an interactive session: twilight records a random pid as the node's intended interactive process for `ttl_seconds` and answers 201 `{"pid", "node"}`, the pid to connect a shell at through dawn's `/v1/connect` and the node's live session. The session's process is reaped once it expires. |
| `POST /api/v1/nodes/{device}/{installation}/logs` | operator | `{"level", "duration_seconds"}` streams the node's logs to the collector through dawn: 202 `{"stream_id"}`. |
| `POST /api/v1/nodes/{device}/{installation}/files` | operator | `{"path"}` collects a file from the node into object storage through dawn: 202 `{"upload_id"}`. |
| `GET /api/v1/revocations?limit=&cursor=` | viewer | [Revocations](#revocations), newest first. |
| `POST /api/v1/revocations` | admin | `{"credential_kind", "credential", "enrolled_after", "enrolled_before", "reason", "dry_run"}` revokes every installation one credential enrolled: 202 `{"matched", "revocation"}`, or 200 with `revocation` `null` for a `dry_run`, which only counts. |
| `GET /api/v1/revocations/{id}` | viewer | A revocation and how far it got. |
| `POST /api/v1/devices/{device}/lifecycle` | admin | `{"lifecycle", "reason"}` with `retired` or `revoked` blocks every installation of the device, those enrolled now and any it enrolls later, through a device-scope `dusk.node-state` record; `active` lifts the block, and each installation keeps its own lifecycle. Answers `{"device_id", "lifecycle", "reason", "changed_at", "actor"}`. |
| `POST /api/v1/selectors/validate` | viewer | `{"selector"}` answers `ok`, the `error` with its position, how many nodes it `matched`, and a `sample` of them. |
| `GET /api/v1/campaigns?status=&limit=&cursor=` | viewer | Campaigns, newest first, each with its counters; `status` takes one status or several, comma-separated. |
| `POST /api/v1/campaigns` | operator | Creates a draft from a [definition](campaigns.md): 201 with `Location`. |
| `GET /api/v1/campaigns/{id}` | viewer | A campaign with its counters by phase and state. |
| `PUT /api/v1/campaigns/{id}` | operator | Replaces a draft's definition; the body is the definition plus the `version` it was made to. |
| `GET /api/v1/campaigns/{id}/overlap` | viewer | `{"count", "campaigns"}`: how many nodes an `ensure_*` campaign shares with running campaigns of its kind (and `version_key`), and which, so a draft can be checked before it is started. |
| `POST /api/v1/campaigns/{id}/start`, `/pause`, `/resume`, `/abort`, `/complete`, `/archive` | operator | Moves the campaign; `pause`, `abort` and `complete` take an optional `{"reason"}`, `resume` takes `{"override_gate", "reason"}`. |
| `GET /api/v1/campaigns/{id}/nodes?state=&phase=&limit=&cursor=` | viewer | The campaign's node rows; `state` takes one state or several, comma-separated. |
| `POST /api/v1/campaigns/{id}/nodes/retry` | operator | `{"states", "nodes", "reason"}` gives `failed` or `unknown` rows a new attempt with a new pid; answers `{"count"}`. |
| `POST /api/v1/campaigns/{id}/nodes/resolve` | operator | `{"nodes", "outcome", "reason"}` closes `unknown` rows by hand as `succeeded` or `failed`; answers `{"count"}`. |
| `GET /api/v1/campaigns/{id}/events?after=&limit=` | viewer | The campaign's history. |
| `GET /api/v1/campaigns/{id}/gates` | viewer | The health gate as it stands: the tallies overall and per breakdown group, the verdict and its reason, the thresholds, the sample the gate waits for, the phase and its bake time. |
| `GET /api/v1/alerts?state=&limit=&cursor=` | viewer | Alerts, newest first; `state` is `open` (the default) or `all`. Each alert carries its `tenant` and its `deliveries`: one per transition and receiver, with its `state` (`pending`, `delivered` or `failed`), `attempts`, `last_error`, `next_attempt_at` and `delivered_at`. |
| `GET /api/v1/alerts/{id}` | viewer | One alert, with its deliveries. |
| `POST /api/v1/alerts/{id}/acknowledge`, `/resolve` | operator | Acknowledges or resolves an alert. |
| `GET /api/v1/stream` | viewer | [Live updates](#live-updates). |

What each answer holds field by field, and the request bodies' full schemas,
are in `/api/openapi.json`.

### Live updates

`GET /api/v1/stream` is a `text/event-stream`. Each event is named by its kind
and its data is one JSON object, `{"kind", "time", "data"}`:

| Event | `data` |
|-------|--------|
| `presence` | `{"online", "degraded"}`: the nodes online in the answering instance's view, and whether that view is degraded. |
| `counters` | Each running or paused campaign's `campaign_id`, `status`, `phase` and `counters`. |
| `alerts` | `{"open", "unacknowledged", "latest"}`: open alerts by severity, the open ones nobody has acknowledged by severity, and the newest open alert. |

A new stream first gets the latest event of each kind, then each kind again
whenever it changes, checked every 5 seconds. A comment line is sent every 15
seconds so idle connections survive proxies. A stream ends after five
minutes, and when the instance shuts down; the `retry: 5000` it starts with
tells a browser's `EventSource` to reconnect five seconds later, which also
authenticates the caller again. A browser's `EventSource` cannot send an
`Authorization` header, so a page uses its session cookie. A client that does
not take an event within 10 seconds is disconnected, and so is one that falls
more than 64 events behind, so that it reconnects to a fresh snapshot. An
instance serves at most [`max_streams`](#top-level) at once.

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
| `twilight_slices_owned` | Partitions of `topics.process_results` this instance owns under a lease. |
| `twilight_slice_lease_losses_total{how}` | Slices this instance stopped owning without giving them up: `lost` when the consumer group dropped it, `fenced` when another instance took the lease. |
| `twilight_slice_partition_mismatches_total` | Process results that arrived in a partition other than the one their node's key hashes to, which means their producer does not partition like Kafka's default partitioner. Each is logged at `error`. |
| `twilight_online_nodes` | Nodes the online view holds as online. |
| `twilight_process_results_partitions_mismatched` | 1 while `topics.process_results` has another partition count than the inventory was migrated with; the leader writes no campaign row, advances no phase and dispatches nothing then. |
| `twilight_view_degraded` | 1 while the online view is degraded (below). |
| `twilight_node_state_rebuilds_total` | Times this instance wrote `dusk.node-state` again from the inventory. |
| `twilight_connections_skips_total` | Times the connection stream skipped to its latest offsets. |
| `twilight_evaluation_queue`, `twilight_evaluation_dropped_total` | Nodes waiting for evaluation, and connect hints dropped because the queue was full. |
| `twilight_row_transitions_total{state}` | Campaign node state changes, by the state entered. |
| `twilight_dispatch_processes_total{kind,outcome}` | Campaign processes handed to dawn, by dawn's answer. |
| `twilight_reap_requests_total{outcome}`, `twilight_reaped_pids_total` | Reap requests sent to dawn, by dawn's answer, and pids dawn reported reaped. |
| `twilight_process_results_total{action_kind,status}` | Process results applied. A file collected or a log stream run for a campaign's process is counted under its own action kind and never moves the campaign's row. |
| `twilight_gate_verdicts_total{verdict}` | Health gate verdicts. |
| `twilight_facts_refresh_total{outcome}` | Facts reads asked of dawn. |
| `twilight_presence_writes_total` | `node_presence` rows written. |
| `twilight_dawn_requests_total{route,outcome}`, `twilight_dawn_request_seconds{route}`, `twilight_dawn_endpoints` | Calls to dawn and the endpoints routed to. |
| `twilight_kafka_invalid_messages_total{topic}` | Kafka records dropped because they failed their contract. |
| `twilight_kafka_refused_records_total{consumer}` | Kafka records skipped because the database refused their data; each is logged at `error` with its topic, partition and offset. U+0000 in a string from a node is replaced with U+FFFD before it is stored, so this stays at zero unless something else is wrong. |
| `twilight_alerts_open{severity,kind}` | Open alerts, refreshed every 30 seconds. A kind with no open alert left drops out of the gauge. |
| `twilight_alert_deliveries_total{receiver,transition,outcome}`, `twilight_alert_deliveries_pending{receiver}` | Notification attempts by outcome - `delivered`, `retry` or `failed` (given up), a digest once for each alert it carries - and the notifications not yet delivered, refreshed every 30 seconds. |
| `twilight_feed_subscribers_dropped_total` | Live-update streams ended because they fell behind. |
| `twilight_reconcile_entries_total{kind}`, `twilight_reconcile_duplicate_entries_total` | Ledger entries reconciled, and exact duplicates skipped. |
| `twilight_reconcile_findings_total{kind}` | Reconcile findings, repeats of an open alert included. |
| `twilight_unattributed_processes_total{principal}` | Processes a client created without naming their pid, by principal. Client-side argument builders start helper processes this way, so they are counted, not alerted. |
| `twilight_reconcile_lag_seconds` | How far behind the present the least advanced ledger partition is reconciled; `+Inf` while a partition has never been reconciled. Refreshed every 30 seconds. |
| `twilight_reconcile_intended_process_cache_entries` | Intended processes held in reconcile's cache. |
| `twilight_api_requests_total{route,code}`, `twilight_api_request_seconds{route}` | API requests answered, by route pattern (`GET /api/v1/campaigns/{id}`) and status code, and how long they took; a live-update stream counts until it ends. Requests under `/api/` that match no route count under the route `unmatched`; the UI's pages are not counted. |
| `twilight_api_authentication_failures_total{reason}` | API requests refused because the caller could not be authenticated or lacked the role: `missing`, `malformed_authorization`, `token`, `session`, `certificate`, `principal`, `role`, `csrf`, `oidc` or `no_role`. |
| `twilight_api_streams` | Live-update streams open on this instance. |
| `twilight_api_requests_refused_total` | API requests answered 503 `busy` because no request slot came free before their deadline. |

twilight logs JSON lines to standard output, one per event, with the ids it
concerns as fields: `campaign_id`, `device_id`, `installation_id`,
`namespace_id`, `epoch`, `pid`, `term`, `partition`, `lease_epoch`, `instance`.

## What twilight guarantees

### One leader at a time

One instance leads: it holds a Postgres session advisory lock on its own
connection to `leader_url` and increments `leadership.term` when it takes it.
It checks every 2 seconds, with a 2-second time limit, that it still holds the
lock, and stops leading after two failed checks. The connection sets TCP
keepalives (idle 10 s, interval 5 s, 3 probes) and a 15-second TCP user
timeout, so a lost primary is noticed within seconds rather than minutes.

Every change to a campaign's node rows is a compare-and-set that also requires
the term to be the current one and the lease of the node's slice to be current
(below), so a leader that lost the lock without noticing cannot change anything
after the next leader took over. A new leader starts every campaign's rate budget empty - a failover
never releases a burst on top of what the previous leader spent - and rebuilds
the campaigns' counters from their rows.

### Slices

The partitions of `topics.process_results` are the inventory's slices: every
node row carries the partition its node's results land in (see Upgrading the
schema). The leader joins the consumer group `kafka.results_group` on that topic,
with a 10-second session timeout and a heartbeat every second, and owns the
partitions the group assigns it. For each one it adds one to the partition's
epoch in `node_partition_leases` and records itself as the owner, works out from
the slice's rows which of its nodes have a process in flight, and only then
reads the slice's results.

* Every insert and update of a campaign node row names the slice's partition
  and the epoch its writer holds, and Postgres refuses it unless that is still
  the partition's epoch. Every write holds a shared lock on its slice until it
  commits, and taking the lease takes that lock exclusively, so a write either
  commits before its successor takes the lease or is refused: an instance that
  lost a slice without noticing writes nothing once its successor took the
  lease.
* When the group takes a slice away, its owner stops writing the slice's rows,
  waits for the writes already running, and commits the results it applied
  before the rebalance goes on. When the group drops the instance, because its
  session expired, it stops at once.
* An owner whose write is refused because another instance took the lease
  stops reading that slice until the group assigns it again.
* A slice whose epoch could not be raised is not read until it is. The owner
  tries again after a random wait that doubles with every failure, up to a
  minute, and logs at `error` once a slice has gone without its lease for a
  minute. Taking a slice away waits only for the attempt on that slice; losing
  it cancels the attempt.
* Results arrive at least once: a rebalance, a restart or a new leader can
  hand a result over again. Applying one is safe twice and out of order: a
  report older than the node's last one does not replace it, and a row whose
  outcome is `unknown` takes no `started`, `running`, `duplicate`, `ended` or
  `timed_out` from before it became `unknown`. Older and newer are told apart
  by time: the time dawn stamped on the result, against the time of the node's
  last report (stamped by dawn, or taken by twilight when it read the facts
  itself) and the time twilight marked the row `unknown`. That holds while
  dawn's and twilight's clocks agree to within the time between two results
  for one node, so keep their hosts' clocks synchronized.
* Operator retries and resolutions through the API write under the slice's
  current lease, whichever instance answers the request; their compare-and-set
  on the row's revision keeps them from overwriting a change the owner made in
  between.

Only the leader joins the group. While a deposed leader is still a member, the
new leader may own only part of the slices; the deposed leader's writes fail on
the term, it commits no result past one it could not apply and keeps trying
that one until it stops leading, and once it leaves the group its slices move
to the new leader, which reads from there.

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
* nightfall admits only the calls an intended process allows
  ([Admission](membrane.md#admission)), so after committing the row twilight
  writes it to `dusk.intended-processes` - keyed
  `<device id>/<installation id>/<pid>`, with the campaign, who asked, the
  subject, `created_at`, `expires_at` and both command budgets - and calls dawn
  only once Kafka has acknowledged the record. While it writes, it holds the
  row locked and writes what the row says then, so the topic always ends with
  the row's latest state. When the write fails, dawn is not called: campaign
  work goes back to `pending` with the status `intent_unpublished` and is tried
  again after a backoff of up to five minutes, and other work fails its
  request. A row that was committed and never written - twilight stopped
  between the two, or Kafka refused the write - is written by the leader within
  five seconds; so is a tombstone for every row that has expired or been
  reaped, and a resend of an expired row writes it again. Each write is
  counted in `twilight_intended_process_publications_total{kind, outcome}`.
* The reap sweep looks at every online node once per `reap_interval_seconds`,
  and at every node that connects. A pid is reaped once no row of a campaign
  can send it again - its row has finished, or moved on to a later attempt -
  and its campaign's retry horizon (`retry.max_backoff_seconds` plus the
  node time-out) has passed since its intended process expired; the pid of
  other work is reaped once it expires. twilight asks dawn to kill and reap up
  to 256 pids of a node per request, oldest first, at most `reaps_per_second`
  requests across the fleet; dawn reports each pid `reaped` in
  `dusk.process-results`, and twilight records it in `reaped_at`.
  A pid dawn did not report reaped is asked again after `reap_retry_seconds`.
  Until then each process stays in the node's process table, which is what
  makes a resend a `duplicate` instead of a second run. Each reap request is
  recorded in `intended_processes` too, as a `reap` at a pid of its own that
  is never reaped, so the commands it runs in the default shell are held to it.

### Reconcile

Every instance takes a share of the `dusk.ledger` partitions and checks each
call nightfall forwarded against the processes twilight intended - a call
nightfall refused, `denied` or `rate_limited`, never reached a node and is not
judged - verifies each
partition's hash chain and checkpoint signatures, and raises the alerts
[Campaigns](campaigns.md#reconcile-alerts) describes. The offsets, the chain
heads and the alerts of one batch are written in one transaction, so a restart
neither skips nor repeats an entry. Watch `twilight_reconcile_lag_seconds`:
`result_without_ledger` is only judged once every partition has been reconciled
ten minutes past a result.

### Rebuilding the node-state topic

`dusk.node-state` is how nightfall learns every revocation, retirement and
quarantine, and twilight's inventory is its system of record. Once the topic
carries every lifecycle of the inventory, twilight writes a completion record
on each of its partitions; nightfall serves no node from a topic that lacks
one ([Readiness](nightfall.md#readiness)).

The leader looks at the topic's id and partition count when it is elected and
every minute after, and reads the topic only when they differ from those of
the last check that found it complete, which the table `node_state_checks`
records. When a partition has no completion record - the first start of a new
stack, a topic Kafka lost, partitions added - it rebuilds the topic from the
inventory, logs at `warn` how many lifecycles it published, and counts it in
`twilight_node_state_rebuilds_total`; a check or rebuild that fails is tried
again after a backoff of at most five minutes. `twilight node-state rebuild`
does the same at once, whatever the topic holds, and prints what it did; run it
after restoring the inventory's database, so that lifecycle changes the topic
holds that are later than the restore point go back into the inventory. Both
read every partition to its end offset first. A partition that reads nothing
for `engine.node_state_stall_seconds` (`--stall-seconds` for the command)
before it gets there fails the rebuild before anything is written, and the
command exits with 1; a read that keeps moving is never cut short, however
large the topic. After partitions are added, both write each key whose latest
state sits on the partition it hashed to before again on the partition it
hashes to now.

The command runs beside the serving process, in its container, so it lowers its
own soft memory limit to 400 MiB (`--memory-limit`, in MiB): with the serving
process's 3600 MiB (`GOMEMLIMIT`) that stays within the container's 4 GiB. The
read holds every key of the topic in memory; for a topic of millions of keys,
raise the Deployment's memory limit and `--memory-limit` together before
running it. How the two are brought in line, and the drills, are in
[Backups and disaster recovery](disaster-recovery.md#the-rebuild).

### Revocations

A revocation revokes every installation one credential enrolled, as twilight's
inventory recorded it: for a fleet token entry the nodes whose
`credential_kind` is `fleet_token`, and for a provision token entry those whose
`credential_kind` is `provision_token`, whose `credential_ref` is the entry's
name; for an install token key those whose `credential_issuer` is the key id,
optionally only those whose `enrolled_at` is at or after `enrolled_after` and
before `enrolled_before`, and never one that is already `revoked`. `matched` is
how many matched when it started. The leader revokes them in the order of their
device and installation ids, in batches, at `engine.revocations_per_second`
across all running revocations: for each batch, in one transaction, it locks
the batch's installations that are not yet revoked, produces one
`dusk.node-state` record per installation, with the revocation's reason and
actor, waits until Kafka has them all, then marks the installations `revoked`
with the time the records carry and moves the revocation's place forward - the
order every lifecycle change follows, so a change to one of those nodes, or a
rebuild of the node-state topic, waits for the batch. A batch Kafka did
not take, or that a former leader was writing, is sent again from that place, so
a revocation carries on across restarts and leader changes and no installation
is skipped. It finishes when no matching installation is left; one enrolled
after it passed its place in the order is not included, which is why the
credential is retired first ([Provisioning](provisioning.md#revoking-what-a-credential-enrolled)).
Each revocation is logged when it starts and when it finishes.
