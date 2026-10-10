# `enrollment_from_new_network_burst`

**Severity:** high. **Raised by:** twilight, from `dusk.enrollments`.
**Fingerprints:** `enrollment_from_new_network_burst:fleet`,
`enrollment_from_new_network_burst:credential:<credential_kind>:<credential>`.

## What it means

More installations enrolled in the last `window_seconds` from networks that no
installation came from in the `baseline_days` (7) days before - a /24 for IPv4,
a /48 for IPv6 - than the threshold allows: the larger of the scope's floor
(`alerts.enrollments.new_network_floors`) and `baseline_factor` times the
average for the same hour over the last `baseline_days` days.

A leaked credential used from rented machines spreads over many networks
nobody enrolled from before, a few installations each, which keeps every
network under its own `enrollment_rate` floor; this alert counts them together.
A product launch in a new market, or a new mobile carrier, looks the same.

The detail names the credential and tenant (credential scope), the `count`, the
`baseline`, the `threshold`, the new networks with the most installations and
how many distinct ones there were.

## First checks

1. Which credentials and networks, in the events DB:

    ```sql
    SELECT credential_ref, credential_issuer, IPv4CIDRToRange(toIPv4OrZero(splitByChar(':', remote_address)[1]), 24).1 AS network, count() AS installations
    FROM dusk.enrollments
    WHERE operation = 'enroll' AND outcome = 'issued' AND time > now() - INTERVAL 1 HOUR
    GROUP BY credential_ref, credential_issuer, network ORDER BY installations DESC LIMIT 50
    ```

    (IPv6 addresses are written `[address]:port` and show as `0.0.0.0` in this
    query.)
2. Who the networks belong to (`whois <address>`): hosting providers and VPN
    exits are rarely where your devices are installed.
3. What the new installations report about themselves in twilight - hostname,
    impl, version, country - against the nodes you know:

    ```sh
    curl -sS -H "Authorization: Bearer $TWILIGHT_TOKEN" -G \
      --data-urlencode 'selector=credential_ref == "<credential>" and lifecycle == "enrolled"' \
      https://twilight.example.org/api/v1/nodes
    ```

4. Whether `enrollment_rate` or `denied_enrollments_spike` alerts are open for
    the same credential.

## What to do

* A launch or a new network you expected: nothing; the networks become part of
  the baseline, and the alert resolves itself.
* A leak: follow [When a credential leaks](../provisioning.md#when-a-credential-leaks) -
  retire the token or remove the key, revoke what it enrolled since the burst
  started with `POST /api/v1/revocations` and `enrolled_after`, and give every
  token you ship a `max_installations`.

## Closing it

twilight resolves the alert itself once the count stays at or below its
threshold for `alerts.enrollments.resolve_after_seconds` (15 minutes by
default), and notifies again while it is open each time the count reaches
`escalation_factor` (2) times the count it last notified.
