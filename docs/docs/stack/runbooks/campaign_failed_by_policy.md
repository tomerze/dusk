# Campaign failed by its policy

Kind `campaign_failed_by_policy`, severity `high`. The twilight leader raises it
in the same transaction that moves a campaign to `failed` because its health
gate failed under the policy `abort.on_gate_failure: abort`; more failures than
`abort.max_total_failures` is one of the ways a gate fails. The detail carries
`message`, `campaign_id`, `campaign_name`, `phase` (the phase's index) and
`phase_name`, `reason` (the gate's reason, for example
`failure rate 0.62 in os_build=22631.4317, 41 of 66`), `group` (the breakdown
group that failed, or empty), `overall` (`succeeded`, `failed`, `eligible`,
`silent`) and `thresholds` (`max_failure_rate`, `max_silent_rate`,
`min_sample`). One alert stays open per campaign. It never resolves by itself.

## What it means

The campaign's open phases did worse than its policy allows, and its policy
says to stop for good ([phases and health gates](../campaigns.md#phases-and-health-gates)).
`failed` is final: the campaign cannot be resumed or retried. Nodes that were
waiting became `cancelled`; processes already on their way finish and their
results are recorded. The `reason` names the limit: a failure rate, a silent
rate - nodes that succeeded, disconnected and did not come back within
`gates.silent_window_seconds` - or `max_total_failures`. Nothing more is sent;
the containment is done.

## How to confirm

The campaign, with `status` `failed` and its `abort_reason`, and its gate's
tallies overall and per group:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/campaigns/<campaign_id>"
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/campaigns/<campaign_id>/gates"
```

The UI's campaign page says why it stopped, with the failing group, and its
**Gates** tab shows the breakdown ([watching a campaign](../ui.md#watching-a-campaign)).

The failed nodes and why, with Grafana's **Inventory** data source
([where to run SQL](index.md#sql)); for a group such as `os_build=22631.4317`,
filter on its dimension and value:

```sql
SELECT device_id, installation_id, state, last_status, last_error, failures, finished_at
FROM campaign_nodes
WHERE campaign_id = '<campaign_id>'
  AND state IN ('failed', 'unknown')
  AND breakdown ->> 'os_build' = '22631.4317'
ORDER BY finished_at DESC
LIMIT 100;
```

For a silent rate, the nodes that went silent:

```sql
SELECT device_id, installation_id, event_at, breakdown
FROM campaign_nodes
WHERE campaign_id = '<campaign_id>' AND silent
LIMIT 100;
```

## What to do

1. Find the cause in `last_error` and on the failed or silent nodes. For nodes
   left `unknown` - the script reached them and no result came back - find out
   what happened on them before anything runs there again
   ([each node's state](../campaigns.md#each-nodes-state)).
2. Fix what the script or the change got wrong.
3. If the work still has to be done, start a new campaign: duplicate this one
   as a new draft (UI: **More → Duplicate as a new draft**) or `POST` its
   definition to `/api/v1/campaigns`. A new campaign runs on every node its
   selector matches, those that already ran this one included, so narrow the
   selector for a one-shot `run_script`. An `ensure_version` node that already
   reports the version succeeds without running anything
   ([what to do: the action](../campaigns.md#what-to-do-the-action)).

Archive the failed campaign when its rows are no longer needed, an hour past its
node timeout after its last dispatch at the earliest:
`POST /api/v1/campaigns/<campaign_id>/archive`.

## How to resolve

Resolve it by hand once the failures are understood and a new campaign, if any,
is created: *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve`.
