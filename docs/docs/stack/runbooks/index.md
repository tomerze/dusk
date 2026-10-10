# Runbooks

This section is for whoever is on call for the Dusk stack. Each page covers one
alert: what it means, how to confirm it, what to do first to contain it and
then to recover, and when it is resolved. Each page is named after its alert's
`kind`: the kind of a twilight alert, or the `kind` label of a Grafana alert
rule, whose notifications link the page.

## The alerts

| Kind | Severity | Raised by | Page |
|------|----------|-----------|------|
| `process_without_intent` | critical | twilight reconcile | [Process nobody intended](process_without_intent.md) |
| `default_shell_without_intent` | critical | twilight reconcile | [Default shell used without intent](default_shell_without_intent.md) |
| `target_mismatch` | critical | twilight reconcile | [Process on the wrong node](target_mismatch.md) |
| `result_without_ledger` | critical | twilight reconcile | [Result without a ledger entry](result_without_ledger.md) |
| `ledger_chain_broken` | critical | twilight reconcile | [Ledger chain broken](ledger_chain_broken.md) |
| `revocation_not_enforced` | critical | twilight leader | [Revocation not enforced](revocation_not_enforced.md) |
| `process_after_deadline` | high | twilight reconcile | [Calls after the deadline](process_after_deadline.md) |
| `pid_reused` | high | twilight reconcile | [Pid reused](pid_reused.md) |
| `process_after_result` | high | twilight reconcile | [Calls after the result](process_after_result.md) |
| `process_shape` | high | twilight reconcile | [Unexpected process shape](process_shape.md) |
| `quarantine_override` | high | twilight reconcile | [Quarantine override](quarantine_override.md) |
| `enrollment_rate` | high | twilight leader | [Enrollment spike](enrollment_rate.md) |
| `campaign_paused_by_gate` | high | twilight leader | [Campaign paused by its gate](campaign_paused_by_gate.md) |
| `campaign_failed_by_policy` | high | twilight leader | [Campaign failed by its policy](campaign_failed_by_policy.md) |
| `campaign_conflict` | medium | twilight leader | [Campaign conflict](campaign_conflict.md) |
| `critical_alert_unacknowledged` | critical | Grafana, over the inventory database | [Critical alert unacknowledged](critical_alert_unacknowledged.md) |
| `clickhouse_ledger_chain_broken` | critical | Grafana, over ClickHouse | [Ledger chain broken in ClickHouse](clickhouse_ledger_chain_broken.md) |
| `ledger_evidence_stalled` | critical | Grafana, over ClickHouse | [Ledger evidence copy stalled](ledger_evidence_stalled.md) |
| `alert_delivery_failing` | high | Grafana, over the inventory database | [Alert delivery failing](alert_delivery_failing.md) |
| `vector_messages_dropped` | high | Grafana, over ClickHouse | [Messages dropped by Vector](vector_messages_dropped.md) |

## How alerts reach people

twilight keeps every alert in the `alerts` table of the inventory database and
records each change to it - opened, raised again at a higher severity,
acknowledged, resolved - as a transition, which it delivers to the receivers its
`alerts.routes` pick (PagerDuty, Slack, email, Teams or a webhook) and retries
until each one takes it; see [Alerts](../twilight.md#alerts). Grafana's rules
are a second path that does not depend on twilight's delivery:
[`critical_alert_unacknowledged`](critical_alert_unacknowledged.md) fires when a
critical alert raised more than 15 minutes ago is still unacknowledged, and
[`alert_delivery_failing`](alert_delivery_failing.md) when a notification gave
up or keeps failing.

## Before you start

The commands on these pages are for the Kubernetes overlays, which put the Dusk
stack in the namespace `dusk`. In the local stack, run
`docker compose exec <service> ...` from `infra/compose/` where a page runs
`kubectl -n dusk exec`.

### The twilight API

Make yourself a token once. `operator` may acknowledge and resolve alerts, move
campaigns and quarantine nodes; revoking a node needs `admin`.

```sh
export TWILIGHT=https://twilight.example.org
export TOKEN=$(kubectl -n dusk exec deploy/twilight -c twilight -- twilight token create --name on-call --role operator)
```

The token goes to standard output and its id to standard error; revoke it
afterwards with `twilight token revoke <id>`. In the local stack the address is
`http://127.0.0.1:8080`. Every call below sends the token:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts?state=open"
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

The first lists the open alerts, newest first; the second answers one alert,
with its `detail`, its `tenant` and the state of its notification to each
receiver (`deliveries`). `$TWILIGHT/alerts?alert=<id>` opens the same alert in
the UI. Every route is in [Routes](../twilight.md#routes).

### SQL

ClickHouse holds the queryable copy of the ledger, connections, enrollments,
process results and telemetry in the database `dusk`. Run the ClickHouse
queries on these pages in Grafana: **Explore**, data source **ClickHouse**.
Grafana is at `http://127.0.0.1:3000` in the local stack; in Kubernetes:

```sh
kubectl -n dusk port-forward service/grafana 3000:3000
```

Postgres holds twilight's inventory in the database `inventory`. Grafana's
**Inventory** data source reads only `nodes`, `node_presence`, `campaigns`,
`campaign_counters`, `campaign_events`, `campaign_nodes`, `alerts`,
`alert_transitions`, `alert_deliveries` and `ledger_chain_heads`. For any other
table, such as `intended_processes`, use `psql`:

| Stack | Command |
|-------|---------|
| local stack | `docker compose exec postgres psql -U postgres -d inventory` |
| dev overlay | `kubectl -n dusk exec -it postgres-0 -- psql -U postgres -d inventory` |
| prod overlay | `psql -d inventory` in the primary pod of the CloudNativePG cluster `postgres` |

### Pids

Alerts, the ledger and twilight write a pid in decimal. dawn's logs and the
node's own logs write it in lowercase hexadecimal:

```sh
python3 -c 'print(format(11259529557207498630, "x"))'
```

### Logs and metrics

```sh
kubectl -n dusk logs -l app.kubernetes.io/name=twilight -c twilight --prefix --since=1h
kubectl -n dusk logs dawn-0 --since=1h
kubectl -n dusk logs nightfall-0 --since=1h
kubectl -n dusk logs deploy/vector --since=1h
```

twilight serves its metrics on port 9102, nightfall on 9100, dawn on 9101:

```sh
kubectl -n dusk port-forward deploy/twilight 9102:9102
curl -s http://127.0.0.1:9102/metrics | grep twilight_reconcile_lag_seconds
```

The collector also scrapes them into ClickHouse, in `dusk.otel_metrics` with
`service_name` `twilight`, `nightfall`, `dawn` or `vector`.

### Acknowledging and resolving

Acknowledge an alert as soon as you own it: the UI's Alerts page has
*Acknowledge*, the API `POST /api/v1/alerts/{id}/acknowledge`. It takes the
alert off the UI's critical banner, and receivers that got the alert are told.
Resolve it when it is dealt with: *Resolve*, or
`POST /api/v1/alerts/{id}/resolve`.

```sh
curl -fsS -X POST -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>/acknowledge"
curl -fsS -X POST -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>/resolve"
```

One alert stays open per fingerprint: the same finding again counts in its
`occurrences` and moves its `last_seen_at`. After a resolve, the next occurrence
opens a new alert.
