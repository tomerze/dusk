# Personal data and retention

This page is for the people who run the Dusk stack and for the people who answer
for the data it keeps: what the stack stores about a node and where, how long
each store keeps it, how to change that, how to erase a device or a tenant and
export what is held about a device, and what an erasure leaves behind and why.

A node is something a person often owns or uses: a laptop, a phone, a camera, a
car. What the stack stores about it - its hostname, its facts, its address, its
logs, the files collected from it - is then personal data. The stack identifies
a node by its device id and installation id, which are random or derived from
the hardware and name no one; they become personal data once you link them to a
person, for example in your own records of who owns which device.

## What is stored where, and for how long

| Store | What it holds about a node | Kept for | Set by |
|-------|----------------------------|----------|--------|
| Postgres `nodes`, `node_presence` | Identity, lifecycle and its reason, tenant, the credential it enrolled with (an install token's subject can be a serial number), hostname, locale, country, OS, facts, reported state, the namespaces of its live sessions | Until the installation has not been seen for 120 days | twilight `retention.installation_days` |
| Postgres `devices`, Kafka `dusk.node-state` | A device's or an installation's retired, revoked or quarantined lifecycle, its reason and who set it | While the lifecycle is set; this is what keeps a withdrawn node out | An operator setting the lifecycle |
| Postgres `campaign_nodes`, `campaign_events` | The node's rows in each campaign: state, pid, namespace, last error, breakdown facts | Until the campaign is archived | Archiving the campaign |
| Postgres `intended_processes`, reconcile's state | The processes twilight intended on the node and who asked for them | 31 days | twilight `engine.intended_process_retention_days` |
| Postgres `alerts`, `alert_transitions`, `alert_deliveries` | Alerts that name the node, such as a process without intent, and each notification of them, with the node's hostname | 400 days after the alert is resolved; open alerts are kept | twilight `retention.alert_days` |
| Postgres `enrollment_counts` | Enrollments counted per hour, per credential and per network (the IPv4 /24 or IPv6 /48 they came from), never per node | The enrollment baseline plus a day: 8 days | twilight `alerts.enrollments.baseline_days` |
| Postgres `erasures` | Each erasure: the device or tenant, who asked, when and why | Kept | |
| Kafka `dusk.connections`, `dusk.enrollments`, `dusk.process-results`, `dusk.process-output`, `dusk.files` | Every connection (with the address the node connected from), enrollment (with its address and hostname), process result (with reported facts), output value and collected file's path | 7 days, and at most one more day until the log segment rolls | Topic `retention.ms` and `segment.ms` |
| Kafka `dusk.otel-logs`, `dusk.otel-spans` | Node logs dawn was asked to stream, and the services' own telemetry | 3 days, plus at most one day | Topic `retention.ms` and `segment.ms` |
| Kafka `dusk.ledger` | The ledger (below) | 30 days, plus at most one day | Topic `retention.ms` and `segment.ms` |
| ClickHouse `dusk.connections`, `enrollments`, `process_results`, `process_output`, `files` | The same as the topics, queryable | 30 days | The table's TTL (`infra/k8s/base/clickhouse/schema/`) |
| ClickHouse `dusk.otel_logs`, `otel_spans`, `otel_metrics` | Telemetry and streamed node logs | 7 days | The table's TTL |
| ClickHouse `dusk.ledger` | The ledger's queryable copy | 30 days | The table's TTL |
| The lake, bucket `dusk-lake` | Parquet copies of every table above; connections and enrollments keep only the network of the node's address | 90 days; `ledger/` 400 days | ceph-init `LAKE_RETENTION_DAYS`, `LAKE_LEDGER_RETENTION_DAYS` |
| Bucket `dusk-files` | The files dawn collected from nodes, under `files/<device>/<installation>/<pid>/` | 30 days | ceph-init `FILES_RETENTION_DAYS` |
| Bucket `dusk-ledger-evidence` | The ledger's evidence copy | Locked for 400 days, removed in the two days after | ceph-init `EVIDENCE_RETENTION_DAYS` |
| SigNoz, when you run it | The node logs and traces it reads from `dusk.otel-logs` and `dusk.otel-spans` | Its own retention period | SigNoz's retention settings |
| step-ca's database | The certificates it issued, which name the device, installation and tenant ids | Kept | step-ca |
| The services' log lines | Ids, and in nightfall's lines the addresses clients connect from | Your log collector's retention | Your log collector |
| Your alert receivers | The notifications twilight and Grafana sent to PagerDuty, Slack, Teams, email or a webhook, with the node's hostname | Each receiver's own retention | The receiver |

The ledger records every call made on a node through nightfall and its result.
An entry holds the node's device, installation and namespace ids, the action,
who made the call and the subject behind its process (`campaign:<id>` or the
operator who asked), and an HMAC of the parameters - never a parameter value, a
result or command output.

### A node's address

nightfall sees the address every node and client connects from. It keeps it as
follows:

| Where | What is kept |
|-------|--------------|
| `dusk.connections`, `dusk.enrollments` and their ClickHouse tables | The full address and port - Kafka for 7 days, ClickHouse for 30 - to investigate abuse such as an enrollment flood. An erasure deletes the ClickHouse rows. |
| Postgres `enrollment_counts` | The network of each enrollment, counted per hour, for twilight's enrollment alerts. |
| The lake's `connections/` and `enrollments/` | Only the network: the address's IPv4 /24 or IPv6 /48. Vector truncates it before it writes the lake. |
| The ledger, in all its copies including the evidence bucket | No full address. The `rpc_rejected` events written before a node's identity is known - a provisioning client, or a client asking for a node nobody holds - carry `remote_network`, the /24 or /48, and `remote_address_hash`, an HMAC of the address keyed with nightfall's param key. |
| nightfall's memory | Live sessions, the penalty box of addresses that failed and the clone check's addresses, never written anywhere. |
| nightfall's log lines | The full address, for operating its per-address limits. |

With the param key, an investigator can tell whether a given address is the one
behind a `remote_address_hash`, without the ledger keeping the address:

```sh
nightfall ledger-hash --address 203.0.113.24
```

prints `remote_address_hash` and `remote_network` for that address, read with
the key in `ledger.param_key_file` (or `--key-file`).

## Changing how long data is kept

The object store's expiry is set when the `ceph-init` job runs, from its
environment; run it again after a change:

| Variable | Default | What it sets |
|----------|---------|--------------|
| `LAKE_RETENTION_DAYS` | `90` | Days the lake keeps every table but the ledger. |
| `LAKE_LEDGER_RETENTION_DAYS` | `400` | Days the lake keeps the ledger's copy under `ledger/`. |
| `FILES_RETENTION_DAYS` | `30` | Days `dusk-files` keeps a collected file. |
| `EVIDENCE_RETENTION_DAYS` | `400` | Days each evidence object is locked. A day after its lock ends it is hidden behind a delete marker, and a day later removed. |
| `EVIDENCE_RETENTION_MODE` | `COMPLIANCE` | `COMPLIANCE`: no one can delete a locked object, the object store's administrator included. `GOVERNANCE` lets a user holding `s3:BypassGovernanceRetention` do it. |

The prod overlay sets them in `infra/k8s/overlays/prod/dusk/patches/environment.yaml`.
A lifecycle rule counts from each object's creation, so a new value applies to
objects already stored too. An object locked under COMPLIANCE keeps its lock
whatever the new setting. The object store applies its lifecycle rules once a
day, so an object can outlive its expiry by up to a day.

twilight's settings, under `retention` in its configuration:

| Key | Default | Meaning |
|-----|---------|---------|
| `installation_days` | `120` | An installation that is offline and has not been seen, renewed or changed for this many days leaves the inventory. At least 98: a node certificate lives 7 days and can be renewed for 90 days after it expires, so an installation unseen for longer can never connect again. If you raise nightfall's `provision.renew_grace`, raise this above 7 days plus it. A retired or revoked installation's `dusk.node-state` record stays, so nightfall keeps refusing it. |
| `alert_days` | `400` | A resolved alert is deleted this many days after it was resolved. |
| `erasure_overdue_days` | `20` | An erasure still running this many days after it was asked for raises an `erasure_stalled` alert. |
| `tenant_days` | none | A shorter retention for some tenants, as `tenant: days`; in the environment, `TWILIGHT__RETENTION__TENANT_DAYS=acme=30,globex=60`. |

A tenant's retention can only be shorter than the stores' own: once a day
twilight erases what is older than the tenant's days, about every device the
tenant has, from ClickHouse, the lake and the files bucket - the same machinery
as an erasure. Kafka and the ledger keep their own retention.

The ClickHouse tables expire rows by the TTL in their schema files: 30 days,
and 7 days for telemetry. Kafka topics keep their records for the
`retention.ms` the `kafka-init` job or the prod overlay's `KafkaTopic`
resources set, and roll segments every day, so a record lives at most one day
longer than that.

## Erasing a device

An erasure deletes what the stack holds about every installation of a device,
everywhere it can be deleted, and reports what it could not delete, until when
and why. It needs the admin role and a reason, which is kept with the erasure
and logged.

1. **Withdraw the device.** Retire or revoke it, with
   `POST /api/v1/devices/{device}/lifecycle` or each of its installations, so
   that nightfall refuses it and nothing new about it is stored while it is
   erased. An erasure of a device with an installation that is neither retired
   nor revoked, or online, is refused with 409 `not_ready`.
2. **Ask for the erasure.**

    ```sh
    curl --fail-with-body -X POST https://twilight.example/api/v1/devices/3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/erasure \
      -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
      -d '{"reason": "erasure request 4711"}'
    ```

    The answer is 202 with the erasure; `Location` names it. Asking again while
    it runs answers the running erasure.
3. **Follow it** with `GET /api/v1/erasures/{id}` until `status` is
   `completed`. Each step says what it did:

| Step | What it does |
|------|--------------|
| `scope` | Finds the device's ids and every node namespace it used, in the inventory, ClickHouse and the lake, and writes them to ClickHouse's `dusk.erasure_scope` for the steps after it. |
| `clickhouse` | Deletes the device's rows from `dusk.connections`, `enrollments`, `process_results`, `process_output`, `files`, `otel_logs` and `otel_spans` with ClickHouse mutations, which rewrite the parts that held them, and waits for them on every replica. |
| `lake` | Finds every lake file of those tables that holds a row of the device, writes a copy of it without those rows through ClickHouse, and deletes the original; a file holding only the device's rows is deleted. |
| `files` | Deletes everything under `files/<device>/` in `dusk-files`. |
| `inventory` | Deletes the device's `nodes` and `node_presence` rows, its `campaign_nodes` rows (and recounts finished campaigns), its intended processes once their `dusk.intended-processes` records are tombstoned, and reconcile's windows for it; removes the device's ids from campaign events and its hostname from the notifications of alerts that name it. |

A step that fails is tried again, first within a minute and then after a
delay that doubles up to an hour; `attempts` and `last_error` say why. An
erasure that has not finished 20 days after it was asked for, or whose step
failed five times in a row, raises the alert
[`erasure_stalled`](runbooks/erasure_stalled.md), which resolves by itself when
the erasure finishes. The erasure goes on from where it stopped, also after a
restart or a change of leader. When it finishes, `retained` lists what the
stack still holds, each with `until` (null when it is kept until something else
removes it) and `why`.

A lake rewrite reads every lake file of each table, so an erasure takes longer
the larger the lake is. The device's rows in the lake are gone when the
erasure finishes, at the latest the lake's retention after they were written.

### What an erasure leaves, and why

| What | Until | Why |
|------|-------|-----|
| The ledger: the evidence copy, the lake's `ledger/`, ClickHouse's `dusk.ledger`, Kafka's `dusk.ledger` | The evidence bucket's lock plus two days; the lake's 400 days plus a day; 30 days plus a day; 30 days plus a day | The ledger is the security record of what was done to the fleet. Its entries are chained by hashes and signed, so deleting one breaks the chain that makes the rest evidence, and the evidence bucket's COMPLIANCE lock lets no one delete it before the lock ends. An entry holds only the device's pseudonymous ids, an HMAC of the parameters, the action and who acted - no hostname, no fact, no value, no output and no full address - and nothing in the stack links those ids to a person once the erasure has deleted the inventory. This is the retention a security record needs, for a fixed period, the same for every node. |
| Records in the other Kafka topics | Each topic's retention plus a day | A Kafka topic is append-only: no record can be deleted on its own. They expire within 8 days, telemetry within 4. |
| The device's retired or revoked lifecycle in `devices` and `dusk.node-state` | While it is set | It is what keeps the device out of the fleet. Set the device `active` to lift it. |
| Alerts that name the device | 400 days after each is resolved | Alerts are security records, kept for the same period as the ledger's evidence. |
| The erasure itself | Kept | The record that the erasure was asked for, by whom and why, and done. |
| step-ca's issued certificates | Kept by step-ca | They name only the device, installation and tenant ids. |
| SigNoz's copy of node logs | SigNoz's retention | An erasure does not reach SigNoz; set its retention to the period you promise. |
| Service log lines | Your log collector's retention | |
| Notifications already sent to your alert receivers | Each receiver's retention | They left the stack when they were sent; delete them in PagerDuty, Slack, Teams, the mailbox or the webhook's store. |

A device that is withdrawn but still running keeps trying to connect and
enroll, and every attempt is recorded again: a refused session in the ledger, a
refused enrollment, with its hostname and address, in `dusk.enrollments`. These
expire with the stores' retention.

A backup of Postgres keeps the rows an erasure later deleted, until the backup
itself expires. Keep backups no longer than the period within which you promise
to erase, or, after restoring one, erase again every device and tenant erased
since it was taken: twilight logs `erasure requested` and `erasure finished`
with the subject of each.

## Exporting what is held about a device

```sh
curl --fail-with-body -X POST https://twilight.example/api/v1/devices/3f9c0e2a7b5d4c1e8a6f0b2d9e7c5a13/export \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"reason": "subject access request 4712"}' -o device.ndjson
```

The answer is one JSON document per line: `{"store", "table", "record"}` for
each record the stack holds about the device - the inventory, its campaign
rows, intended processes, alerts with their notifications, and erasures in
Postgres; its rows in each ClickHouse table, the ledger included; the same in
the lake; and the name, size and time of each of its collected files - and a
last line `{"summary": ...}` that counts the records, says whether the export
is `complete` and names what it does not include. A store that could not be read gives a line with `error`
and makes the export incomplete; an export without its summary line was cut
short. The export needs the admin role and a reason, and is logged. One export
runs at a time on each instance; it stops after 30 minutes.

## Offboarding a tenant

`POST /api/v1/tenants/{tenant}/erasure` with `{"reason"}` erases every device of
the tenant - those in the inventory and those the tenant's connections and
enrollments name in ClickHouse and the lake - with the same steps as a device
erasure, and deletes the tenant's campaigns. Every node of the tenant must be
retired or revoked and offline, and none of its campaigns running or paused,
or it is refused with 409 `not_ready`. To withdraw them, revoke every
installation each of the tenant's credentials enrolled with
`POST /api/v1/revocations` ([revocations](twilight.md#revocations)), and retire
those credentials in nightfall so that no node of the tenant can enroll again. A campaign's tenant is a label: deleting a
tenant's campaign also deletes its rows for nodes of other tenants.
