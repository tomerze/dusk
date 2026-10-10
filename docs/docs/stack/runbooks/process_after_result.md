# Calls after the result

Kind `process_after_result`, severity `high`. twilight's reconcile raises it
when nightfall's ledger shows calls under a pid more than a minute after dawn
reported that process's final result, and twilight had not sent the work again
since. Killing and reaping it (`Dusk.kill`, `Dusk.waitpid`) is not counted. The
detail carries `message`, `pid`, `result_at`, `result_status`, `last_call_at`,
`sessions`, `commands`, and from the intended process `device_id`,
`installation_id`, `action_kind`, `max_commands` and, for campaign work,
`campaign_id`. One alert stays open per pid. It never resolves by itself.

## What it means

When dawn's work at a pid is done it stops the shell there, and the process
stays in the node's process table only to be reaped
([`POST /v1/dispatch`](../dawn.md#post-v1dispatch)). Calls under the pid after
the result come from a dawn instance that kept using a finished process, or from
another client that knows the pid. Either way something used the node after its
work was over.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

The calls after the result, in ClickHouse ([where to run SQL](index.md#sql)):

```sql
SELECT time, principal, session_id, action, result_code
FROM dusk.ledger
WHERE pid = <pid> AND kind = 'call'
  AND time > parseDateTime64BestEffort('<result_at>', 9, 'UTC')
ORDER BY time
```

What dawn reported, and which dawn instance:

```sql
SELECT time, action_kind, status, delivered, dawn_instance, error
FROM dusk.process_results
WHERE pid = <pid>
ORDER BY time
```

If the late calls come from the same dawn instance, its log for the pid, in
hexadecimal, shows what it was doing:

```sh
kubectl -n dusk logs <dawn_instance> --since=24h | grep "$(python3 -c 'print(format(<pid>, "x"))')"
```

## What to do

* Late calls from a principal that is not one of your dawn pods: contain as for
  [a process nobody intended](process_without_intent.md#what-to-do) - quarantine
  the node, then add the principal to `deny_principals` in the ConfigMap
  `nightfall-permissions`.
* Late calls from the dawn instance that reported the result, with nothing in
  its log to explain them: treat that dawn instance as compromised and deny its
  principal the same way. It then reaches no node until you remove the entry.

## How to resolve

Resolve it by hand once the late calls are explained: *Resolve* on the UI's
Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
