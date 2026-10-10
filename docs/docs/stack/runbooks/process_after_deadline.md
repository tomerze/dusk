# Calls after the deadline

Kind `process_after_deadline`, severity `high`. twilight's reconcile raises it
when nightfall's ledger shows a call under a pid after the time until which
twilight intended that process. Killing and reaping it (`Dusk.kill`,
`Dusk.waitpid`) is not counted. The detail carries `message`, `pid`,
`expires_at` (the end of the intended process), `time` (when the call reached
nightfall), `principal`, `session_id`, `call_id`, `device_id`,
`installation_id`, `namespace_id`, `action`, and the entry's place in the
ledger: `instance`, `partition` and `sequence`. One alert stays open per pid.
It never resolves by itself.

## What it means

A client kept working in a process after twilight's intent for it ended: a
person still typing in an interactive session after the time they picked when
they opened it ([acting on a node](../ui.md#acting-on-a-node)), or a client
using a pid it kept from earlier work. The call's `time` comes from the
nightfall instance in `instance` and `expires_at` from twilight, so clocks that
disagree look the same.

nightfall's [admission](../security.md#admission) refuses calls under a pid
whose intended process expired, killing and reaping it aside, so this call got
past it: a role exempt from admission, a nightfall instance that does not hold
calls to the intended processes, or a defect.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

What the pid was intended for, and who asked, in `psql`
([where to run SQL](index.md#sql)):

```sql
SELECT pid, action_kind, subject, campaign_id, attempt, created_at, expires_at, last_dispatched_at
FROM intended_processes
WHERE pid = <pid>;
```

The calls after the deadline, in ClickHouse:

```sql
SELECT time, principal, session_id, action, result_code
FROM dusk.ledger
WHERE pid = <pid> AND kind = 'call'
  AND time > parseDateTime64BestEffort('<expires_at>', 9, 'UTC')
ORDER BY time
```

* `action_kind` `interactive`: `subject` opened the session (`token:<id>` for an
  API token). Calls minutes or hours past `expires_at` from the same dawn
  instance are that session still in use.
* Calls a few seconds past `expires_at`: compare the clocks of the nightfall
  instance and twilight.
* Calls from another `principal` than the one the earlier calls came from, or
  long after work that is not interactive: treat it as
  [a process nobody intended](process_without_intent.md).

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

* An interactive session in use: ask its `subject` to end it with dawn's
  `/v1/disconnect`, and to open a new session from twilight for more time.
* Clocks: fix the time on the hosts of that nightfall instance and of twilight.
* Anything else: contain as for
  [a process nobody intended](process_without_intent.md#what-to-do) - quarantine
  the node, then add the principal to `deny_principals` in the ConfigMap
  `nightfall-permissions`.

## How to resolve

Resolve it by hand once the calls are accounted for: *Resolve* on the UI's
Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
