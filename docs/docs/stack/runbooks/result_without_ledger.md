# Result without a ledger entry

Kind `result_without_ledger`, severity `critical`. twilight's reconcile raises
it when dawn reported on `dusk.process-results` that it started a process at a
pid, and nightfall's ledger shows no `Dusk.process` for that pid within ten
minutes. It is judged only once every ledger partition has been reconciled ten
minutes past the result. The detail carries `message`, `pid`, `delivered_at`,
what dawn reported - `dawn_instance`, `device_id`, `installation_id`,
`namespace_id`, `action_kind`, `campaign_id`, `status` - and what reconcile saw
at the pid: `sessions`, `commands`, and `result_at`, `result_status` and
`last_call_at` when it has them. One alert stays open per pid. It never
resolves by itself.

## What it means

Either the ledger is missing calls or the result is false. A compromised
nightfall can make calls on its nodes without writing them to the ledger; a
compromised dawn, or whoever controls Kafka, can write results for processes
that never ran
([compromised components](../security.md#compromised-components)). Results
count in campaigns' health gates, so a false one can move a campaign on.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Check that reconcile is not behind: `twilight_reconcile_lag_seconds` should be
seconds, not minutes ([logs and metrics](index.md#logs-and-metrics)).

Look for the pid in ClickHouse's copy of the ledger
([where to run SQL](index.md#sql)):

```sql
SELECT time, instance, partition, sequence, principal, session_id, action, device_id, installation_id
FROM dusk.ledger
WHERE pid = <pid>
ORDER BY time
```

A `Dusk.process` within ten minutes of `delivered_at` means the entry exists
and reconcile missed it: look for reconcile warnings in the twilight instances'
logs before you resolve the alert. No entry at all for the pid confirms the
alert.

What dawn reported, and which nightfall instance held the node then:

```sql
SELECT time, action_kind, status, delivered, dawn_instance, campaign_id, attempt, error
FROM dusk.process_results
WHERE pid = <pid>
ORDER BY time
```

```sql
SELECT time, event, instance, namespace_id, disconnect_reason
FROM dusk.connections
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
  AND time BETWEEN parseDateTime64BestEffort('<delivered_at>', 9, 'UTC') - INTERVAL 1 DAY
               AND parseDateTime64BestEffort('<delivered_at>', 9, 'UTC') + INTERVAL 1 HOUR
ORDER BY time
```

The dawn pod named in `dawn_instance` logs the pid in hexadecimal:

```sh
kubectl -n dusk logs <dawn_instance> --since=24h | grep "$(python3 -c 'print(format(<pid>, "x"))')"
```

The node's own log is the only other record: `logs dump --replay-only` on the
node lists every process created, with its pid in hexadecimal
([nightfall](../security.md#nightfall)). Run it in an interactive session:
*Open session* on the node's page in the UI, then dawn's shell endpoints.

## What to do

Contain first:

1. Pause the campaign in `campaign_id`, so a false result moves nothing on:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"reason": "result_without_ledger alert <id>"}' \
      "$TWILIGHT/api/v1/campaigns/<campaign_id>/pause"
    ```

2. If the node's log shows the process, the nightfall instance that held the
   node did not write its call to the ledger. Every nightfall instance mounts
   the same secrets ([nightfall](../security.md#nightfall)), so treat all of
   them as exposed and escalate.

3. If the node's log does not show it, the result is false. Add the dawn
   instance's principal (its pod name) to `deny_principals` in the ConfigMap
   `nightfall-permissions`, as for
   [a process nobody intended](process_without_intent.md#what-to-do), and find
   out how its pod was changed.

Then resolve the campaign's rows by what really happened: retry them, or close
`unknown` ones by hand ([each node's state](../campaigns.md#each-nodes-state)),
and resume or abort the campaign.

## How to resolve

Resolve it by hand once you know which record is wrong: *Resolve* on the UI's
Alerts page, or `POST /api/v1/alerts/<id>/resolve`.
