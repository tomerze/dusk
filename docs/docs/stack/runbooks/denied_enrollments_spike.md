# Refused enrollment spike

Kind `denied_enrollments_spike`, severity `high`, then `critical` once its count
doubles. The twilight leader raises it from `dusk.enrollments`, with the fingerprint `denied_enrollments_spike:fleet`,
`denied_enrollments_spike:credential:<credential_kind>:<credential>`,
`denied_enrollments_spike:network:<network>` or `denied_enrollments_spike:network`.

## What it means

More `assign` and `enroll` calls were refused (`denied` or `rate_limited`) in
the last `window_seconds` than the threshold allows: the larger of the scope's
floor (`alerts.enrollments.refused_floors`) and `baseline_factor` times the
average for the same hour over the last `baseline_days` days. Renewals are never
counted.

The scope says who is refused:

* **credential**: someone presents a known credential that nightfall refuses -
  a retired token, a capped one, or one pressing against its rate limit. A
  retired token that is still presented is a leak being used, or a device fleet
  you forgot to move to a new token.
* **network**: one /24 (IPv4) or /48 (IPv6) is refused a lot - wrong tokens,
  revoked devices, a penalized address, or a network over its
  `[[limits.cidr]] enrollments_per_hour`. Someone may be guessing tokens.
* **fleet**: refusals rose across the fleet, which a credential or network alert
  usually explains.

At most 10 networks have an alert of their own at a time. Once 10 are open,
every further network above its threshold is named in the one
`denied_enrollments_spike:network` alert instead: its `count` is the number of
those networks, and `networks` lists the ones refused most. Refusals from that
many networks at once are a distributed attempt, not one misconfigured site.

The detail holds the `count`, the `baseline`, the `threshold`, and the
`networks` and `credentials` the refusals came from, with their counts.

## How to confirm

1. The reasons, in ClickHouse ([where to run SQL](index.md#sql)):

    ```sql
    SELECT reason, credential_ref, credential_issuer, count() AS refusals, uniq(remote_address) AS addresses
    FROM dusk.enrollments
    WHERE operation IN ('assign', 'enroll') AND outcome IN ('denied', 'rate_limited')
      AND time > now() - INTERVAL 1 HOUR
    GROUP BY reason, credential_ref, credential_issuer ORDER BY refusals DESC LIMIT 20
    ```

    Add `AND remote_address LIKE '203.0.113.%'` to look at one network.
2. The reasons mean, in [the provisioning reference](../provisioning.md#what-is-recorded):
    * `invalid_credential`, `invalid_install_token`, `malformed_request`: wrong
      credentials, which also fill the penalty box (`nightfall_penalty_box_ips`).
    * `credential_retired`: a retired token is still presented.
    * `credential_quota_reached`: see
      [`credential_quota_reached`](credential_quota_reached.md).
    * `credential_rate`, `enrollment_rate`, `network_enrollment_rate`: a burst
      larger than the rate limits; check `enrollment_rate` alerts for the same
      credential.
    * `install_token_rate`: one install token presented again and again,
      usually one copied off a device.
    * `device_revoked`, `installation_revoked`: revoked devices trying again.
3. Whether an `enrollment_rate` or `enrollment_from_new_network_burst` alert is
    open for the same credential: refusals beside installations are a leak in
    use.

## What to do

* Wrong credentials from a few networks: nothing gets in, and the penalty box
  slows them; block the networks upstream if they persist.
* A retired or capped credential still presented: treat the credential as
  leaked ([When a credential leaks](../provisioning.md#when-a-credential-leaks)),
  or move the devices you still own to a new token.
* Rate limit refusals for a legitimate rollout: they retry and enroll; raise
  `enrollments_per_second_per_credential`, or the network's
  `enrollments_per_hour`, if the rollout must go faster.
* Refusals during a step-ca outage are `error`, not refusals, and do not raise
  this alert.

## How to resolve

twilight resolves the alert itself once the count stays at or below its
threshold for `alerts.enrollments.resolve_after_seconds` (15 minutes by
default). The first time the count reaches `escalation_factor` (2) times the
count the alert opened with, twilight raises it to `critical`: receivers get it
again as `re_escalated`, PagerDuty pages again, and its acknowledgement is
cleared. Each later time the count doubles again, its detail takes the new
count, which sends nothing. Resolved by hand, it is noticed within 15 seconds; twilight then counts
from zero, and the next count above the threshold opens a new alert.
