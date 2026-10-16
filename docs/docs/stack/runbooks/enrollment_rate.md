# Enrollment spike

Kind `enrollment_rate`, severity `high`, then `critical` once its count doubles.
The twilight leader counts the new
installations nightfall issues - an `enroll` that was `issued`, never a renewal
or a refusal - over the last five minutes (`alerts.enrollments.window_seconds`),
for the whole fleet, for each credential and for each /24 or /48 network, and
raises this alert when a count passes its threshold: the larger of its floor
(`alerts.enrollments.issued_floors`: 100, 30 and 20) and four times
(`baseline_factor`) the average for the same hour over the last seven days. Each
has its own fingerprint: `enrollment_rate:fleet`,
`enrollment_rate:credential:<credential_kind>:<credential>`,
`enrollment_rate:network:<network>`, and `enrollment_rate:network` for the
networks beyond the 10 that have an alert of their own (see
[enrollment alerts](../twilight.md#enrollment-alerts)). The detail carries the
`scope`, the `count`, the `threshold`, the `baseline`, the credential or the
network, and the networks and credentials the count came from.

## What it means

Many new installations at once, with one credential, from one network, or
across the fleet. A fleet token, a provision token or an install token key
extracted from one node enrolls as many nodes as its holder likes, from as many addresses as they like,
up to the credential's `max_installations`
([the fleet token](../security.md#the-fleet-token)); a large rollout looks the
same in the count. Refused attempts raise
[Refused enrollment spike](denied_enrollments_spike.md) instead.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Where one credential's installations came from, in ClickHouse
([where to run SQL](index.md#sql)); `credential_ref` names a fleet token or a
provision token by its entry's `name`, and `credential_issuer` an install token
key by its id:

```sql
SELECT remote_address, count() AS installations, uniqExact(hardware_fingerprint_hash) AS fingerprints, min(time) AS first_installation, max(time) AS last_installation
FROM dusk.enrollments
WHERE credential_kind = '<credential_kind>' AND credential_ref = '<credential>'
  AND operation = 'enroll' AND outcome = 'issued'
  AND time > now() - INTERVAL 1 DAY
GROUP BY remote_address
ORDER BY installations DESC
LIMIT 50
```

* Installations from the networks and at the pace a rollout explains: the
  rollout. Nothing to do; the alert resolves itself.
* Installations nobody can account for, often from hosting providers'
  networks, beside an `enrollment_from_new_network_burst` or
  `credential_quota_reached` alert for the same credential: a leaked credential.

## What to do

For a leaked credential, follow
[When a credential leaks](../provisioning.md#when-a-credential-leaks):

1. Retire it: `retired = true` on its entry in `fleet_tokens_file` or
   `provision_tokens_file`, or remove the key from `install_token_keys`. Every nightfall instance refuses it within
   30 seconds of the file changing, with no restart.
2. Revoke what it enrolled since the leak, in one request (`admin`). Count first:

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"credential_kind": "<credential_kind>", "credential": "<credential>", "enrolled_after": "<first suspicious enrollment>", "reason": "leaked token", "dry_run": true}' \
      "$TWILIGHT/api/v1/revocations"
    ```

    then send the same request without `dry_run`.
3. Give the nodes that may still have to enroll a new provision token, with a
   `max_installations` (see
   [Rotating a provision token](../provisioning.md#rotating-a-provision-token)).
   A fleet token cannot be replaced on the nodes built with it.

Until they are revoked, nodes enrolled with it join campaigns their reported
facts match and count in those campaigns' health gates: pause campaigns whose
gates they could sway.

## How to resolve

twilight resolves the alert itself once the count stays at or below its
threshold for `alerts.enrollments.resolve_after_seconds` (15 minutes by
default). The first time the count reaches `escalation_factor` (2) times the
count the alert opened with, twilight raises it to `critical`: receivers get it
again as `re_escalated`, PagerDuty pages again, and its acknowledgement is
cleared. Each later time the count doubles again, its detail takes the new
count, which sends nothing. Resolved by hand - *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve` - it is noticed within 15 seconds; twilight
then counts from zero, and the next count above the threshold opens a new alert.
