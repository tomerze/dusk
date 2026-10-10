# Process on the wrong node

Kind `target_mismatch`, severity `critical`. twilight's reconcile raises it when
nightfall's ledger shows a call under a pid twilight intended for one node
reaching another node. The detail carries `message`, `pid`, `principal`,
`session_id`, `call_id`, the node the call reached (`device_id`,
`installation_id`, `namespace_id`), the node the pid was intended for
(`intended_device_id`, `intended_installation_id`), `action`, `time`, and the
entry's place in the ledger: `instance`, `partition` and `sequence`. One alert
stays open per pid. It never resolves by itself.

## What it means

Work twilight meant for one node was started on another, and nightfall forwarded
the call. A client may have sent an intended pid somewhere else on purpose - a
replayed pid - or the namespace id twilight used led to the wrong node: whoever
controls Kafka can forge presence and so make twilight send a node's work to the
namespace id of another node ([Kafka](../security.md#kafka)). Either way a node
ran something nobody asked it to.

nightfall's [admission](../security.md#admission) holds every call against the
intended processes of the node it reaches and refuses a pid intended for another
node, so this call got past it: a role exempt from admission, a nightfall
instance that does not hold calls to the intended processes, compromised or
misconfigured, or a defect.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

What twilight intended at the pid, in `psql` ([where to run SQL](index.md#sql)):

```sql
SELECT pid, device_id, installation_id, campaign_id, attempt, action_kind, subject, created_at, expires_at
FROM intended_processes
WHERE pid = <pid>;
```

Every call under the pid, and on which node it landed, in ClickHouse:

```sql
SELECT time, principal, session_id, action, device_id, installation_id, namespace_id, result_code
FROM dusk.ledger
WHERE pid = <pid> AND kind = 'call'
ORDER BY time
```

The sessions each node had around that time, to see whether the namespace id
the call used belonged to the intended node:

```sql
SELECT time, event, device_id, installation_id, namespace_id, instance, disconnect_reason
FROM dusk.connections
WHERE namespace_id = '<namespace_id>' AND time > now() - INTERVAL 1 DAY
ORDER BY time
```

Whether a role exempt from admission made the calls: every call of such a role
that admission would have refused carries an `admission_override` event in the
ledger, which names the role and the rule
([admission](../security.md#admission)). In ClickHouse:

```sql
SELECT time, principal, action, pid, event_detail
FROM dusk.ledger
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
  AND event = 'admission_override' AND time > now() - INTERVAL 1 DAY
ORDER BY time
```

No row means a nightfall instance forwarded the calls without holding them to
the intended processes. The calls admission refused never reached the node: they
are `denied` entries in the ledger and count in
`nightfall_admission_refused_total{rule}`.

## What to do

Contain first:

1. Quarantine the node the call reached, and look at the intended node too:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"lifecycle": "quarantined", "reason": "target_mismatch alert <id>"}' \
      "$TWILIGHT/api/v1/nodes/<device_id>/<installation_id>/lifecycle"
    ```

2. If the pid belongs to a campaign (`campaign_id` in `intended_processes`),
   pause it while you look:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"reason": "target_mismatch alert <id>"}' \
      "$TWILIGHT/api/v1/campaigns/<campaign_id>/pause"
    ```

3. If the calls came from a principal that is not one of your dawn pods, add
   it to `deny_principals` in the ConfigMap `nightfall-permissions`, as for
   [a process nobody intended](process_without_intent.md#what-to-do).

Then find out what ran on the node it reached: the action and the campaign tell
you which script, and `GET /api/v1/campaigns/<campaign_id>` shows its
definition. Release the node (`"lifecycle": "active"`) or revoke it (`admin`,
`"lifecycle": "revoked"`), and resume or abort the campaign.

## How to resolve

Resolve it by hand once you know how the pid reached the other node and both
nodes are dealt with: *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve`.
