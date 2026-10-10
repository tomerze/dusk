# Revocation not enforced

Kind `revocation_not_enforced`, severity `critical`. The twilight leader checks
every minute for nodes it has revoked or retired, or whose device it has
revoked or retired, that its online view still shows connected two census intervals
(`engine.census_interval_seconds`, 300 seconds by default) after the change, and
raises this alert for each. The detail carries `device_id`, `installation_id`,
`lifecycle` (`revoked` or `retired`), `changed_at`, and the session it still
sees: `instance` (the nightfall instance) and `namespace_id`. One alert stays
open per node; each later check that still finds it counts in `occurrences`. It
never resolves by itself.

## What it means

When nightfall reads a node's `revoked` or `retired` record on
`dusk.node-state`, it closes the node's sessions within a second and refuses it
from then on ([revocation and quarantine](../security.md#revocation-and-quarantine)).
A node still connected long after means one of: the nightfall instance is not
applying `dusk.node-state`; the record never reached it; a forged record on
`dusk.node-state` lifted the revocation, which needs control of Kafka
([Kafka](../security.md#kafka)); or twilight's online view of that instance is
out of date. Until the session closes, clients can still reach the node
through it.

## How to confirm

Read the alert, then the node as twilight sees it - its lifecycle, its live
sessions with their nightfall instance, and its device's lifecycle:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/nodes/<device_id>/<installation_id>"
```

Whether twilight's online view is degraded (`degraded` in the overview). After
it skipped connection events it stays degraded until every nightfall instance
has sent a full census, and until then it can miss disconnections
([the online view](../twilight.md#the-online-view)).

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/overview"
```

The node's session events in ClickHouse ([where to run SQL](index.md#sql)). A
`disconnected` with `revoked` or `lifecycle_changed` after `changed_at` and no
`connected` after it means nightfall did close it:

```sql
SELECT time, event, instance, namespace_id, epoch, disconnect_reason, remote_address
FROM dusk.connections
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
ORDER BY time DESC
LIMIT 20
```

Whether the nightfall instance keeps up with `dusk.node-state`:
`nightfall_node_state_lag_seconds` is the age of the last node state it applied,
0 when it is caught up.

```sh
kubectl -n dusk port-forward pod/<instance> 9100:9100
curl -s http://127.0.0.1:9100/metrics | grep -E 'nightfall_node_state_lag_seconds|nightfall_consumer_errors_total|nightfall_sessions_refused_total'
```

## What to do

Contain first: restart the nightfall instance that holds the session.

```sh
kubectl -n dusk delete pod <instance>
```

It closes all its sessions in random order over `drain_seconds` (300 by
default), and every node it held reconnects to another instance. An instance
that has read the record refuses a revoked node after the handshake, and a new
pod is not ready until it has read
`dusk.node-state` to the end, so a restart never lets a revoked node back in
([draining](../nightfall.md#draining)). If nightfall's
[admin API](../nightfall.md#the-admin-api) is enabled (`[admin]` TLS; the
shipped overlays leave it off), `POST /v1/sessions/<namespace_id>/kill` on that
instance closes the one session at once.

Then find out why:

* `nightfall_node_state_lag_seconds` well above 0, or
  `nightfall_consumer_errors_total{topic="dusk.node-state"}` rising: nightfall
  cannot read the topic. Look at its log and at Kafka.
* The node keeps connecting after the restart: some instance does not see the
  record. Compare the node's lifecycle in twilight with the records on
  `dusk.node-state`; a record you did not make is a Kafka compromise, escalate.
* The view was degraded and ClickHouse shows the session closed: twilight's
  view was behind, and the node was not connected.

## How to resolve

Resolve it by hand once `GET /api/v1/nodes/<device_id>/<installation_id>` shows
no live session and the cause is known: *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve`. If the node is still connected at the next
check, a new alert opens.
