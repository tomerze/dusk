# Unexpected process shape

Kind `process_shape`, severity `high`. twilight's reconcile raises it in two
cases, both from shell commands (`ShPortal.sh`) in nightfall's ledger:

* **A process's own shell** ran more commands than twilight intended for it -
  for a campaign's process its script, one `cp` per collected file and one
  `logs stream`; for an interactive session `reconcile.commands_per_session`. The
  detail carries `message`, `pid`, `commands`, `max_commands`, `sessions`,
  `device_id`, `installation_id`, `action_kind`, `campaign_id` for campaign
  work, and `last_call_at`, `result_at`, `result_status` when reconcile has
  them. One alert stays open per pid.
* **A node's default shell** ran more commands in one stretch than the
  processes twilight intended for the node at the time allow together
  (`reconcile.default_shell`). The detail carries `message`, `pid` (the default
  shell's), `device_id`, `installation_id`, `window_start`, `last_call_at`,
  `commands` and `max_commands`. One alert stays open per node and stretch.

Each time twilight sends a campaign's process again, both allowances grow by
one more run of it. It never resolves by itself.

## What it means

dawn runs a fixed number of commands for each piece of work
([`reconcile`](../twilight.md#reconcile)). More than that means something ran
more than the work: a person running more than `reconcile.commands_per_session`
commands in one interactive session, or a client running its own commands inside
a process twilight intended.

nightfall's [admission](../security.md#admission) refuses shell commands beyond
a process's budget and beyond the default shell's, so these commands got past
it: a role exempt from admission, a node that reconnected to another nightfall
instance, which counts its commands from zero ([the
membrane](../membrane.md#admission)), a nightfall instance that does not hold
calls to the intended processes, or a defect.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

The commands in the process's own shell, in ClickHouse
([where to run SQL](index.md#sql)):

```sql
SELECT time, principal, session_id, result_code
FROM dusk.ledger
WHERE pid = <pid> AND action = 'ShPortal.sh' AND kind = 'call'
ORDER BY time
```

The commands in the default shell during the stretch:

```sql
SELECT time, principal, session_id, result_code
FROM dusk.ledger
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
  AND pid = <pid> AND action = 'ShPortal.sh' AND kind = 'call'
  AND time BETWEEN parseDateTime64BestEffort('<window_start>', 9, 'UTC') - INTERVAL 5 SECOND
               AND parseDateTime64BestEffort('<last_call_at>', 9, 'UTC')
ORDER BY time
```

The ledger keeps a keyed hash of each command, not the command, so it shows how
many ran, when and from which client, not what they were
([what stays undetectable](../security.md#what-stays-undetectable)).

* `action_kind` `interactive`: someone ran more than
  `reconcile.commands_per_session` commands in one session. Who opened it is
  `subject` in `intended_processes` for the pid.
* Extra commands from a `principal` that is not one of your dawn pods: a client
  ran its own commands.
* Extra commands from a dawn pod: compare them with that pod's log for the pid
  (`kubectl -n dusk logs <principal> | grep <pid in hexadecimal>`). Commands its
  work does not explain mean the dawn instance is compromised.

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
the intended processes, or that the node's commands were counted by more than
one instance. The calls admission refused never reached the node: they are
`denied` entries in the ledger and count in
`nightfall_admission_refused_total{rule}`.

## What to do

* A long interactive session that someone owns: nothing to contain. If such
  sessions are normal for you, raise `reconcile.commands_per_session`.
* Otherwise contain as for
  [a process nobody intended](process_without_intent.md#what-to-do): quarantine
  the node, add the principal to `deny_principals` in the ConfigMap
  `nightfall-permissions`, and pause the campaign in `campaign_id`:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"reason": "process_shape alert <id>"}' \
      "$TWILIGHT/api/v1/campaigns/<campaign_id>/pause"
    ```

## How to resolve

Resolve it by hand once the extra commands are explained: *Resolve* on the UI's
Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
