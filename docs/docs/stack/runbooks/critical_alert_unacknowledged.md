# Critical alert unacknowledged

Kind `critical_alert_unacknowledged`, severity `critical`. Grafana's rule
`dusk-open-critical-alert` reads twilight's `alerts` table in the inventory
database every minute and fires when a critical alert raised more than 15
minutes ago is neither acknowledged nor resolved. It is a second path to people
that does not depend on twilight's own delivery, and it resolves by itself once
no such alert is left.

## What it means

A critical twilight alert has waited a quarter of an hour without anyone
taking it. Either its notification never reached anyone, or it reached someone
who did not acknowledge it in twilight. Two cases that look like the second:

* It was acknowledged in PagerDuty only. twilight tells PagerDuty about
  acknowledgements, not the other way round, so the alert stays unacknowledged
  in twilight.
* It was raised again at a higher severity, which clears its acknowledgement.
  An incident already acknowledged in PagerDuty gets no new page from
  PagerDuty, so this rule is what tells you.

## How to confirm

The critical alerts nobody has acknowledged, from the API:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts?state=open&limit=500" \
  | jq '.items[] | select(.severity == "critical" and .acknowledged_at == null) | {id, kind, time, occurrences}'
```

or with Grafana's **Inventory** data source ([where to run SQL](index.md#sql)):

```sql
SELECT id, time, last_seen_at, kind, fingerprint, occurrences
FROM alerts
WHERE severity = 'critical' AND resolved_at IS NULL AND acknowledged_at IS NULL
ORDER BY time;
```

For each, whether its notifications went out: `deliveries` in
`GET /api/v1/alerts/<id>` lists each receiver with its `state` (`pending`,
`delivered`, `failed`), `attempts` and `last_error`.

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

If twilight does not answer, the rows above are still the truth: the alerts
are in Postgres whatever twilight's state.

## What to do

1. Take each alert: open its kind's page from [the runbooks](index.md#the-alerts)
   and follow it.
2. Acknowledge it in twilight - *Acknowledge* on the UI's Alerts page, or:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>/acknowledge"
    ```

3. If a receiver shows `failed` or keeps retrying, fix it as in
   [alert delivery failing](alert_delivery_failing.md). If no receiver is listed
   at all, no route in `alerts.routes` matches the alert's severity, kind or
   tenant ([Alerts](../twilight.md#alerts)).

## How to resolve

Nothing to do in Grafana: the rule resolves at its next evaluation once every
critical alert older than 15 minutes is acknowledged or resolved.
