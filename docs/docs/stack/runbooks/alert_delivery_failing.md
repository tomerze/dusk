# Alert delivery failing

Kind `alert_delivery_failing`, severity `high`. Grafana's rule
`dusk-alert-delivery-failing` reads twilight's `alert_deliveries` table in the
inventory database and fires when a notification gave up (`state` `failed`) in
the last hour, or one has been retrying for more than 10 minutes. It resolves by
itself once neither is true.

## What it means

twilight delivers every alert transition - `opened`, `re_escalated`,
`acknowledged`, `resolved` - to each receiver its `alerts.routes` pick, as one
row of `alert_deliveries` per transition and receiver
([Alerts](../twilight.md#alerts)). Any twilight instance delivers rows that are
due; the rows live in Postgres, so they survive restarts and leader changes.

* **Retrying**: a network error, HTTP 408, 425, 429 or 5xx, or an SMTP 4xx. The
  receiver is down, rate limiting, or out of reach. twilight tries again with a
  growing, randomised wait.
* **Gave up**: any other HTTP 4xx or an SMTP 5xx gives up at once - the receiver
  refused the notification, usually because of its configuration (a revoked
  webhook, a wrong routing key, a refused address). A row still not delivered
  `alerts.delivery_horizon_seconds` (86400 by default) after it was created gives
  up too.

Until it is fixed, the people behind that receiver do not hear about the alerts
it carries. A notification that gave up is not sent again.

## How to confirm

The notifications that gave up in the last hour or have been retrying for more
than 10 minutes, in `psql` ([where to run SQL](index.md#sql)):

```sql
SELECT d.alert_id, a.kind, a.severity, t.transition, d.receiver, d.state, d.attempts,
       d.last_error, d.last_attempt_at, d.next_attempt_at, d.created_at
FROM alert_deliveries d
JOIN alert_transitions t ON t.id = d.transition_id
JOIN alerts a ON a.id = d.alert_id
WHERE (d.state = 'failed' AND coalesce(d.last_attempt_at, d.created_at) > now() - interval '1 hour')
   OR (d.state = 'pending' AND d.created_at < now() - interval '10 minutes')
ORDER BY d.created_at;
```

One alert's deliveries, from the API or on the UI's Alerts page:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

twilight's log has one line per attempt - `alert notification failed; retrying`
(warn), `alert notification gave up` (error), `alert notification delivered`
(info) - with `alert_id`, `transition`, `receiver`, `attempts` and `error`:

```sh
kubectl -n dusk logs -l app.kubernetes.io/name=twilight -c twilight --prefix --since=1h | grep 'alert notification'
```

and counts them in `twilight_alert_deliveries_total{receiver,transition,outcome}`
(`delivered`, `retry`, `failed`) and `twilight_alert_deliveries_pending{receiver}`
on port 9102.

## What to do

1. Read the alerts the failing receiver should have carried (the first query)
   and handle them now, from the UI's Alerts page: their notifications did not
   reach anyone through it.
2. Fix the receiver from `last_error`:
    * A refusal (HTTP 4xx, SMTP 5xx): correct the secret the receiver reads -
      the PagerDuty routing key, the Slack or Teams webhook URL, the mail
      password - or its entry in `alerts.receivers`, such as the mail server's
      address or recipients. twilight reads a secret file again for every
      attempt, so a replaced secret is used from the next one; a change to
      `alerts.receivers` itself takes a restart:
      `kubectl -n dusk rollout restart deployment twilight`.
    * A network error or timeout: check that the receiver answers and that
      twilight can reach it. In Kubernetes twilight reaches outside the cluster
      only where its NetworkPolicies allow: the shipped `twilight-oidc` policy
      allows public addresses on port 443 and `twilight-alert-receivers` SMTP
      servers on 25, 465 and 587; a webhook on a private address needs a policy
      of its own ([Kubernetes](../deploy.md#kubernetes)).
    * HTTP 429 or 5xx: the receiver itself is struggling; twilight keeps trying
      until the delivery horizon.
3. Watch `twilight_alert_deliveries_pending` for that receiver fall as the
   retries go through.

## How to resolve

Nothing to do in Grafana: the rule resolves once no notification has given up
in the last hour and none has been retrying for more than 10 minutes.
