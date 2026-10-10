# Pid reused

Kind `pid_reused`, severity `high`. twilight's reconcile raises it when
nightfall's ledger shows a process at one pid created on more than one client
connection, and twilight had not sent that work again since the process was
first created. The detail carries `message`, `pid`, `sessions` (the client
connections that created it), `commands` (shell commands run in it so far),
and from the intended process `device_id`, `installation_id`, `action_kind`,
`max_commands` and, for campaign work, `campaign_id`; `last_call_at`,
`result_at` and `result_status` when reconcile has them. One alert stays open
per pid. It never resolves by itself.

## What it means

dawn creates each piece of work's process once. A second creation that
twilight did not cause comes from a dawn instance that reconnected to nightfall
in the middle of the work, from two dawn instances that got the same work at
once during a dawn rollout ([`POST /v1/dispatch`](../dawn.md#post-v1dispatch)),
or from another client that learned the pid and used it.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Each creation, with its principal and connection, in ClickHouse
([where to run SQL](index.md#sql)):

```sql
SELECT time, principal, session_id, instance, device_id, installation_id, result_code
FROM dusk.ledger
WHERE pid = <pid> AND action = 'Dusk.process'
ORDER BY time
```

* The same dawn `principal` on each, and that dawn pod restarted between them
  (`kubectl -n dusk get pod <principal>` shows its restarts and age, and its log
  stops and starts again): a reconnect. Nothing else touched the node.
* Two of your dawn pods within seconds of each other while dawn was being
  rolled out: the rollout case, and the script may have run twice.
* A principal that is not one of your dawn pods: someone else used the pid.

What dawn reported for the pid:

```sql
SELECT time, action_kind, status, delivered, dawn_instance, error
FROM dusk.process_results
WHERE pid = <pid>
ORDER BY time
```

## What to do

* A reconnect or a rollout: nothing to contain. If the work is a one-shot
  script, check on the node that it did not run twice where that matters
  ([what to do: the action](../campaigns.md#what-to-do-the-action)).
* Another principal: contain as for
  [a process nobody intended](process_without_intent.md#what-to-do) - quarantine
  the node, then add that principal to `deny_principals` in the ConfigMap
  `nightfall-permissions` - and pause the campaign in `campaign_id`:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"reason": "pid_reused alert <id>"}' \
      "$TWILIGHT/api/v1/campaigns/<campaign_id>/pause"
    ```

## How to resolve

Resolve it by hand once the second creation is explained: *Resolve* on the UI's
Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
