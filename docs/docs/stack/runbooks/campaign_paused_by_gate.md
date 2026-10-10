# Campaign paused by its gate

Kind `campaign_paused_by_gate`, severity `high`. The twilight leader raises it
in the same transaction that pauses a running campaign because its health gate
failed, under the policy `abort.on_gate_failure: pause` (the default). The
detail carries `message`, `campaign_id`, `campaign_name`, `phase` (the phase's
index) and `phase_name`, `reason` (the gate's reason, for example
`failure rate 0.62 in os_build=22631.4317, 41 of 66`), `group` (the breakdown
group that failed, or empty), `overall` (`succeeded`, `failed`, `eligible`,
`silent`) and `thresholds` (`max_failure_rate`, `max_silent_rate`,
`min_sample`). One alert stays open per campaign. It resolves by itself, with
whoever did it as the resolver, when the campaign is resumed, aborted or
completed.

## What it means

The campaign's open phases did worse than its policy allows, so twilight
stopped sending its work ([phases and health gates](../campaigns.md#phases-and-health-gates)).
The `reason` says which limit:

* `failure rate ...`: too many nodes reported their script failed, overall or
  in one group of a breakdown dimension (`os_build`, `hardware_class`,
  `dusk_version`, `country`).
* `silent rate ...`: too many nodes that succeeded disconnected and did not come
  back within `gates.silent_window_seconds`: nodes that stopped connecting after
  the change. Look at these first.
* `... failures, more than max_total_failures ...`: more nodes failed in total
  than `abort.max_total_failures`.

While it is paused nothing new is sent - no first dispatch, no resend, no
retry; processes already on their way finish and their results are recorded,
and bake time does not accrue ([life of a campaign](../campaigns.md#life-of-a-campaign)).
The containment is already done.

## How to confirm

The campaign, with `status` `paused`, `pause_kind` `gate` and `pause_reason`,
and its gate as it stands now - the tallies overall and per group, the verdict,
the reason and the thresholds:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/campaigns/<campaign_id>"
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/campaigns/<campaign_id>/gates"
```

The campaign's page in the UI shows the same on its **Gates** tab, opened on
the group that tripped the gate ([watching a campaign](../ui.md#watching-a-campaign)).

The failed nodes and why, with Grafana's **Inventory** data source
([where to run SQL](index.md#sql)); for a group such as
`os_build=22631.4317`, filter on its dimension and value:

```sql
SELECT device_id, installation_id, state, last_status, last_error, failures, finished_at
FROM campaign_nodes
WHERE campaign_id = '<campaign_id>'
  AND state IN ('failed', 'backoff', 'unknown')
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

and, for one of them, its sessions in ClickHouse:

```sql
SELECT time, event, instance, disconnect_reason
FROM dusk.connections
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
ORDER BY time DESC
LIMIT 20
```

## What to do

Find the cause in `last_error` and on the failed or silent nodes before you
send anything more. Then one of:

* **Abort** when the change is wrong. Nodes still waiting become `cancelled`;
  processes already on their way still report.

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"reason": "<why>"}' "$TWILIGHT/api/v1/campaigns/<campaign_id>/abort"
    ```

    To go on without the failing group, duplicate the campaign as a new draft
    (UI: **More → Duplicate as a new draft**) with a selector that leaves it
    out, for example `... and os_build != "22631.4317"`. A new campaign runs
    on every node its selector matches, those that already ran this one
    included.

* **Resume with a gate override** when the failures are understood and
  acceptable. The reason is recorded; until the phase passes, the gate counts
  only the nodes dispatched after the resume, so the same group fails it again
  if the problem is still there.

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"override_gate": true, "reason": "<why>"}' "$TWILIGHT/api/v1/campaigns/<campaign_id>/resume"
    ```

    Without `override_gate` and a reason the resume is refused with 409
    `gate_override_required`.

* **Retry** the failed nodes once their cause is fixed, then resume with the
  override as above: retry gives `failed` and `unknown` nodes a new attempt with
  a new pid, at most 10 000 at a time.

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"states": ["failed"], "reason": "<why>"}' "$TWILIGHT/api/v1/campaigns/<campaign_id>/nodes/retry"
    ```

* **Complete** an `ensure_*` campaign whose fleet has converged enough:
  `POST /api/v1/campaigns/<campaign_id>/complete`.

## How to resolve

No need to resolve it by hand: it resolves when the campaign is resumed,
aborted or completed. If the gate fails again after a resume, the campaign pauses again
and a new alert opens.
