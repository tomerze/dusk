# `credential_quota_reached`

**Severity:** high. **Raised by:** twilight, from `dusk.enrollments`.
**Fingerprint:** `credential_quota_reached:credential:<credential_kind>:<credential>`.

## What it means

A fleet token entry or an install token key enrolled its `max_installations`,
and nightfall refused an enrollment with it as `credential_quota_reached`. No
installation beyond the cap got a certificate. The detail names the credential
(`credential_kind`, `credential`, `tenant`), how many refusals came in the last
`window_seconds` (`count`) and the networks they came from (`networks`,
`distinct_networks`).

It is one of two things: a batch that is genuinely larger than its cap, or a
leaked credential that used the cap up.

## First checks

1. How many installations the credential enrolled, and when. In twilight:

    ```sh
    curl -sS -H "Authorization: Bearer $TWILIGHT_TOKEN" -G \
      --data-urlencode 'selector=credential_kind == "fleet_token" and credential_ref == "<credential>"' \
      --data-urlencode 'limit=1000' https://twilight.example.org/api/v1/nodes
    ```

    For an install token key, select on `credential_issuer == "<key id>"`
    instead.
2. Where they enrolled from, in the events DB:

    ```sql
    SELECT toStartOfHour(time) AS hour, count() AS installations, uniq(remote_address) AS addresses
    FROM dusk.enrollments
    WHERE operation = 'enroll' AND outcome = 'issued'
      AND credential_ref = '<credential>' AND time > now() - INTERVAL 7 DAY
    GROUP BY hour ORDER BY hour
    ```

    (`credential_issuer = '<key id>'` for an install token key.) Installations
    you shipped come from the networks and at the pace your rollout explains; a
    leak shows as installations you cannot account for, often from hosting
    providers' networks and at the credential's rate limit.
3. Whether an `enrollment_rate`, `enrollment_from_new_network_burst` or
    `denied_enrollments_spike` alert for the same credential is open. Together
    they point to a leak.
4. `nightfall_credential_installations_remaining{credential="<credential>"}` is
    0 on every nightfall instance.

## What to do

**The batch is larger than its cap.** Raise `max_installations` on the entry in
`fleet_tokens_file`, or on the key in `install_token_keys`. nightfall loads the
change within 30 seconds, plus the Secret's propagation in Kubernetes; the
refused nodes retry and enroll.

**The credential leaked.** Follow
[When a credential leaks](../provisioning.md#when-a-credential-leaks): retire the
token (`retired = true`) or remove the key, issue a new one for the nodes you
still ship, and revoke what the leaked credential enrolled since the leak with
`POST /api/v1/revocations`, using `enrolled_after` for the moment of the leak.

## Closing it

twilight resolves the alert itself once no enrollment was refused for the cap
for `alerts.enrollments.resolve_after_seconds` (15 minutes by default). A
retired token's attempts are refused as `credential_retired` instead and no
longer keep this alert open. Resolve it by hand
(`POST /api/v1/alerts/{id}/resolve`) once you have acted, if you want it closed
sooner; a new refusal opens a new one.
