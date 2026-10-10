# Default shell used without intent

Kind `default_shell_without_intent`, severity `critical`. twilight's reconcile
raises it when nightfall's ledger shows a shell command (`ShPortal.sh`) run in a
node's default shell while no process twilight intended for that node was open:
none created before the command and not yet expired, allowing 5 seconds either
side for clock skew. The detail carries `message`, `pid` (the default shell's),
`principal`, `session_id`, `call_id`, `device_id`, `installation_id`,
`namespace_id`, `action`, `time`, and the entry's place in the ledger:
`instance`, `partition` and `sequence`. One alert stays open per node and client
connection. It never resolves by itself.

## What it means

dawn runs its process table reads, state reads, kills and reaps in a node's
default shell, and only around work twilight intended for that node
([`reconcile`](../twilight.md#reconcile)). A command there with nothing intended
means a client used the node outside any work twilight asked for. A command in
the default shell can run anything the node can.

nightfall's [admission](../security.md#admission) refuses the node's default
shell while none of the node's intended processes is open, so this command got
past it: a role exempt from admission, a nightfall instance that does not hold
calls to the intended processes, compromised or misconfigured, or a defect.
Treat it as a compromise until shown otherwise.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Rule out clock skew first: list the processes twilight intended for the node
within a minute of the command, in `psql` ([where to run SQL](index.md#sql)).
A process open a few seconds off the command's `time` points at the clocks of
the nightfall instance in `instance` and of twilight, not at an intruder.

```sql
SELECT pid, action_kind, subject, created_at, expires_at
FROM intended_processes
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
  AND created_at <= timestamptz '<time>' + interval '1 minute'
  AND expires_at >= timestamptz '<time>' - interval '1 minute'
ORDER BY created_at;
```

Everything the client connection did, in ClickHouse:

```sql
SELECT time, action, pid, device_id, installation_id, result_code
FROM dusk.ledger
WHERE session_id = toUUID('<session_id>') AND kind = 'call' AND time > now() - INTERVAL 1 DAY
ORDER BY time
```

Calls from a `principal` that is not one of your dawn pods confirm it.

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

Contain first, as for [a process nobody intended](process_without_intent.md#what-to-do):

1. Quarantine the node:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"lifecycle": "quarantined", "reason": "default_shell_without_intent alert <id>"}' \
      "$TWILIGHT/api/v1/nodes/<device_id>/<installation_id>/lifecycle"
    ```

2. Add the principal to `deny_principals` in the ConfigMap
   `nightfall-permissions` (`kubectl -n dusk edit configmap nightfall-permissions`)
   and in `infra/k8s/base/nightfall/permissions.toml`.

3. Revoke the node (`admin`, `"lifecycle": "revoked"`) if what ran on it cannot
   be known.

When the intended processes show a clock gap instead, fix the time on the hosts
of that nightfall instance and of twilight; nothing on the node needs undoing.

## How to resolve

Resolve it by hand once the principal is cut off or the clocks are fixed:
*Resolve* on the UI's Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
