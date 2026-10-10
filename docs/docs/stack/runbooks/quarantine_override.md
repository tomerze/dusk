# Quarantine override

Kind `quarantine_override`, severity `high`. twilight's reconcile raises it for
each `quarantine_override` event in nightfall's ledger: a principal whose role
has `quarantine_override = true` made a call on a quarantined node. The detail
carries `message`, `principal`, `session_id`, `call_id`, `pid`, `action`,
`device_id`, `installation_id`, `namespace_id`, `time`, and the event's place
in the ledger: `instance`, `partition` and `sequence`. One alert stays open per
client connection and pid. It never resolves by itself.

## What it means

Quarantine limits every client on a node to what the role `quarantine` allows,
except roles with `quarantine_override = true`, which keep their own grants
([revocation and quarantine](../security.md#revocation-and-quarantine)). No
role in the shipped permissions file has the override, so this alert means
someone gave a role the override and a principal with it reached a node that
was quarantined: incident response working on the node, or someone else who
holds that role.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Which roles have the override, and which principals get them:

```sh
kubectl -n dusk get configmap nightfall-permissions -o jsonpath='{.data.permissions\.toml}'
```

Every override on the node, in ClickHouse ([where to run SQL](index.md#sql)):

```sql
SELECT time, principal, session_id, action, pid, instance
FROM dusk.ledger
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
  AND event = 'quarantine_override'
ORDER BY time DESC
```

Ask the people working the incident on that node whether the principal is
theirs.

## What to do

* Claimed by incident response: nothing to contain.
* Nobody claims it: add the principal to `deny_principals` in the ConfigMap
  `nightfall-permissions` (`kubectl -n dusk edit configmap nightfall-permissions`,
  and the same change in `infra/k8s/base/nightfall/permissions.toml`), and set
  `quarantine_override = false` on the role unless someone needs it. Then
  handle the node as for
  [a process nobody intended](process_without_intent.md#what-to-do).

## How to resolve

Resolve it by hand once the override is confirmed or the principal is cut off:
*Resolve* on the UI's Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
