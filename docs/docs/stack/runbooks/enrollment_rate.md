# Enrollment spike

Kind `enrollment_rate`, severity `high`. The twilight leader counts the records
nightfall writes to `dusk.enrollments` per minute - every assign, enroll and
renew, whatever its outcome - and raises this alert when one minute holds more
than `alerts.enrollment_rate_per_minute` (600 by default). The detail carries
`message`, `minute` (the first minute over the threshold) and `threshold`. One
alert stays open at a time: a later minute over the threshold counts in
`occurrences` and moves `last_seen_at`, and leaves `minute` as it was. It never
resolves by itself.

## What it means

Many enrollment calls at once. A fleet token extracted from one node enrolls as
many nodes as its holder likes, from as many addresses as they like
([the fleet token](../security.md#the-fleet-token)); a large rollout, a burst of
renewals or a flood of refused attempts looks the same in the count.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

What the calls were, minute by minute, in ClickHouse
([where to run SQL](index.md#sql)); `credential_ref` names a fleet token by its
entry's `name` and an install token by its subject:

```sql
SELECT toStartOfMinute(time) AS minute, operation, outcome, credential_kind, credential_ref, count() AS calls
FROM dusk.enrollments
WHERE time > now() - INTERVAL 2 HOUR
GROUP BY minute, operation, outcome, credential_kind, credential_ref
ORDER BY minute, calls DESC
```

Where they came from, for one credential:

```sql
SELECT remote_address, count() AS calls, uniqExact(hardware_fingerprint_hash) AS fingerprints, min(time) AS first_call, max(time) AS last_call
FROM dusk.enrollments
WHERE credential_ref = '<credential_ref>' AND operation = 'enroll' AND outcome = 'issued'
  AND time > now() - INTERVAL 1 DAY
GROUP BY remote_address
ORDER BY calls DESC
LIMIT 50
```

* Mostly `renew`: nodes renewing their certificates together. Nothing new
  joined.
* Mostly `denied` or `rate_limited`: refused attempts; `reason` says why
  ([what is recorded](../provisioning.md#what-is-recorded)).
* `enroll` with `issued` for one fleet token, from addresses and fingerprints
  that match no rollout you know of: a leaked token.

## What to do

For a leaked fleet token, contain first:

1. Retire the token: remove its entry from `fleet-tokens.toml` in the Secret
   `fleet-token` and restart nightfall one instance at a time
   ([fleet tokens](../provisioning.md#fleet-tokens)):

    ```sh
    kubectl -n dusk rollout restart statefulset nightfall
    ```

    This stops new enrollments with it, also those of the genuine nodes built
    with it; each instance drains for up to `drain_seconds` (300) before it
    stops.

2. Revoke the installations it enrolled since the leak, which keep renewing
   otherwise, leaving out the ones you can account for. List them:

    ```sql
    SELECT DISTINCT device_id, installation_id
    FROM dusk.enrollments
    WHERE credential_ref = '<credential_ref>' AND operation = 'enroll' AND outcome = 'issued'
      AND time > parseDateTime64BestEffort('<first suspicious enrollment>', 9, 'UTC')
    ```

    and revoke each (`admin`):

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"lifecycle": "revoked", "reason": "enrolled with leaked fleet token <credential_ref>"}' \
      "$TWILIGHT/api/v1/nodes/<device_id>/<installation_id>/lifecycle"
    ```

3. Ship nodes built with a new token ([node images](../deploy.md#node-images)).

Until they are revoked, nodes enrolled with it join campaigns their reported
facts match and count in those campaigns' health gates: pause campaigns whose
gates they could sway.

## How to resolve

Resolve it by hand once the burst is explained, or the token retired and its
nodes revoked: *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve`. The next minute over the threshold after
that opens a new alert.
