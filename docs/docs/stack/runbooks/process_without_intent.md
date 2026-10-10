# Process nobody intended

Kind `process_without_intent`, severity `critical`. twilight's reconcile raises
it when nightfall's ledger shows a client creating a process (`Dusk.process`)
on a node at a pid twilight never recorded in `intended_processes`. The detail
carries `message`, `pid`, `principal` (the client certificate's principal),
`session_id` (the client's connection to nightfall), `call_id`, `device_id`,
`installation_id`, `namespace_id`, `action`, `time`, and where the entry is in
the ledger: `instance` (the nightfall instance), `partition` and `sequence`. One
alert stays open per pid. It never resolves by itself.

## What it means

A client created a process on a node without twilight asking for it, and
nightfall forwarded the call. Whoever can create a process controls the node
([the path of a process](../security.md#the-path-of-a-process)).

nightfall's [admission](../security.md#admission) refuses `Dusk.process` at a
pid twilight did not intend for the node, so this call got past it: a role
exempt from admission - an operator's break-glass role - used outside the work
twilight intended, a nightfall instance that does not hold calls to the intended
processes, compromised or misconfigured, or a defect. Treat it as a compromise
until you have found which.

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Everything the same client connection did, in ClickHouse
([where to run SQL](index.md#sql)); a `result_code` of `NULL` means nightfall
forwarded the call:

```sql
SELECT time, action, pid, device_id, installation_id, result_code
FROM dusk.ledger
WHERE session_id = toUUID('<session_id>') AND kind = 'call' AND time > now() - INTERVAL 1 DAY
ORDER BY time
```

What dawn reported about the pid, if anything:

```sql
SELECT time, action_kind, status, delivered, dawn_instance, campaign_id, error
FROM dusk.process_results
WHERE pid = <pid>
ORDER BY time
```

When `principal` is a dawn instance (`dawn-0`, `dawn-1`, ... - the name of its
pod), look for the pid, in hexadecimal, in that pod's log:

```sh
kubectl -n dusk logs <principal> --since=24h | grep "$(python3 -c 'print(format(<pid>, "x"))')"
```

A line `opened an interactive session` names the caller of dawn's API in its
`principal` field: that caller connected at a pid twilight never gave out.

Whether a role exempt from admission made the calls: every call of such a role
that admission would have refused carries an `admission_override` event in the
ledger, which names the role and the rule
([admission](../security.md#admission)). In ClickHouse:

```sql
SELECT time, principal, action, pid, event_detail
FROM dusk.ledger
WHERE device_id = '<device_id>' AND installation_id = '<installation_id>'
  AND event = 'admission_override' AND time > now() - INTERVAL 1 DAY
ORDER BY time
```

No row means a nightfall instance forwarded the calls without holding them to
the intended processes. The calls admission refused never reached the node: they
are `denied` entries in the ledger and count in
`nightfall_admission_refused_total{rule}`.

## What to do

Contain first:

1. Quarantine the node. nightfall drops every client's access to it at once and
   from then on lets clients call only `Dusk.hostname`, `Dusk.programs`,
   `Dusk.namespaceId` and `Dusk.time` on it
   ([revocation and quarantine](../security.md#revocation-and-quarantine)):

    ```sh
    curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      -d '{"lifecycle": "quarantined", "reason": "process_without_intent alert <id>"}' \
      "$TWILIGHT/api/v1/nodes/<device_id>/<installation_id>/lifecycle"
    ```

    twilight still sends campaign work to the node, and nightfall may deny it,
    which pauses that campaign ([acting on a node](../ui.md#acting-on-a-node)).

2. Cut the principal off at nightfall. Add it to `deny_principals` in nightfall's
   permissions file, the ConfigMap `nightfall-permissions`
   ([the permissions file](../membrane.md#the-permissions-file)):

    ```sh
    kubectl -n dusk edit configmap nightfall-permissions
    ```

    ```toml
    deny_principals = ["<principal>"]
    ```

    Once the kubelet has updated the file in the pods, nightfall reads it within
    30 seconds and drops every connection of that principal: watch
    `nightfall_membranes_dropped_total{reason="principal_revoked"}` rise on port
    9100. Make the same change in `infra/k8s/base/nightfall/permissions.toml`, or
    the next `kubectl apply -k` puts the old file back. A denied dawn instance
    reaches no node until you remove the entry.

3. If the dawn log named a caller of dawn's API, take its access away: remove
   its entry from dawn's tokens file (`/etc/dawn/secrets/tokens.toml`, read again
   when it changes), or its role at the OpenID Connect provider
   ([dawn's authentication](../dawn.md#authentication-and-roles)).

4. If the node cannot be trusted any more, revoke it (`admin`): the same call
   with `"lifecycle": "revoked"`. nightfall closes its sessions and refuses it
   from then on.

Then find who holds the certificate. In the prod overlay a `dawn-*` principal
certificate is signed only for pods of the ServiceAccount `dawn`, for 24 hours
at most, and an `admin-*` principal for anyone who may create a Certificate in
the namespace `dusk` ([the prod overlay](../deploy.md#the-prod-overlay)). The
ledger records which calls reached the node, not what a script did there
([what stays undetectable](../security.md#what-stays-undetectable)); release the
node (`"lifecycle": "active"`) only once you know what ran on it.

## How to resolve

Resolve it by hand once the principal is cut off or cleared and the node is
released or revoked: *Resolve* on the UI's Alerts page, or
`POST /api/v1/alerts/<id>/resolve`. A later process at the same pid opens a new
alert.
