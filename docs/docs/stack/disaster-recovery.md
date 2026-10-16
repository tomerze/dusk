# Backups and disaster recovery

This page is for the people who run the Dusk stack: what holds state, what
backs each piece up or rebuilds it, what is lost when it goes and why that is
acceptable, and the drills that restore it - each one run end to end, with
what it took.

## What holds state

| What | Where | Protected by | Lost in the worst case |
|------|-------|--------------|------------------------|
| The inventory: nodes and their lifecycles, campaigns and their rows, intended processes, alerts, reconcile's position in the ledger | Postgres, database `inventory` | Continuous WAL archiving and daily base backups to `dusk-postgres-backups`, kept 30 days ([Postgres](#postgres)) | The last minute of changes (`archive_timeout: 60s`) |
| step-ca's record of the one-time tokens it accepted | step-ca's badger database on its volume `database` | Nothing: it is not backed up ([Lose step-ca](#lose-step-ca)) | Every install token not yet expired can be used once more |
| step-ca's keys and configuration | Secrets `step-ca`, `step-ca-password`; `step-ca-root-key` offline | Your copy of the Secrets ([Secrets](#secrets)) | Nothing if the copy exists; otherwise every node must enroll again |
| Every node's lifecycle as nightfall enforces it | Kafka, `dusk.node-state` | Rebuilt from the inventory ([Lose Kafka's node state](#lose-kafkas-node-state)) | Nothing: the inventory is the system of record |
| The processes twilight intends | Kafka, `dusk.intended-processes` | Republished from the inventory ([Lose Kafka](#lose-kafka)) | Nothing |
| The ledger | Kafka, `dusk.ledger`, 30 days | The object-locked evidence copy, the lake and ClickHouse | Entries Vector had not yet copied: seconds |
| Every other topic | Kafka | Copies, or the state they describe | See [Lose Kafka](#lose-kafka) |
| Queryable events and telemetry | ClickHouse | The Parquet lake | Nothing older than what the lake holds |
| The lake, collected files, SigNoz's cold tier | Object store, `dusk-lake`, `dusk-files`, `signoz-cold` | Ceph's replication (three copies in prod) | Everything, with the site |
| The ledger's evidence copy | Object store, `dusk-ledger-evidence`, object-locked | Object lock against deletion; a replica at another site against losing the site ([Lose a whole cluster](#lose-a-whole-cluster)) | Everything, with the site, unless replicated |
| Keys and passwords | Kubernetes Secrets | Your copy of the Secrets ([Secrets](#secrets)) | See the table there |

## Postgres

The prod overlay backs up the CloudNativePG cluster `postgres` with the Barman
Cloud plugin (CloudNativePG 1.30.1 and plugin v0.15.1 were used to validate it;
install the plugin with the operators, see [the prod overlay](deploy.md#the-prod-overlay)):

* every WAL file is archived to `s3://dusk-postgres-backups/postgres/` on Rook's
  object store, compressed with zstd, as soon as Postgres closes it, and
  Postgres closes one at least every minute (`archive_timeout: 60s`);
* the `ScheduledBackup` `postgres-daily` takes a base backup every day at 02:30,
  from a standby when one is healthy, and once as soon as it is created, so a
  new cluster has a backup from its first minutes;
* the `ObjectStore` `postgres-backups` keeps base backups and WAL for 30 days
  (`retentionPolicy: 30d`), so any moment of the last 30 days can be restored.

The object store user `postgres-backup` (keys in the Secret
`ceph-postgres-backup`) may only read and write its own bucket, which ceph-init
creates. The resources are in `infra/k8s/overlays/prod/dusk/postgres/`. See the
state of the backups with:

```sh
kubectl -n dusk get backups,scheduledbackups
kubectl -n dusk get cluster postgres -o jsonpath='{range .status.conditions[*]}{.type}={.status}{"\n"}{end}'
```

`ContinuousArchiving=True` and `LastBackupSucceeded=True` are what you want.
Grafana's rules [Postgres backup failing](runbooks/postgres_backup_failing.md)
and [Postgres WAL archiving lagging](runbooks/postgres_wal_archiving_lagging.md)
page when either stops, and the first also when their metrics stop arriving;
the collector scrapes the instances' metrics for them (`service_name`
`postgres` in `dusk.otel_metrics`).

The bucket lives on the same Rook cluster as everything else. Against losing
the site, point the `ObjectStore`'s `endpointURL` and credentials at an object
store elsewhere, or replicate the bucket there; [Lose a whole
cluster](#lose-a-whole-cluster) assumes one of the two.

## Secrets

secrets-init creates every secret once and never replaces one, so a Secret that
is gone is created again with a new value - which for some of them cannot be
undone. Keep a copy of these outside the cluster (encrypted, with access as
narrow as to the cluster itself), taken after the first deployment and again
whenever one changes:

```sh
kubectl -n dusk get secret fleet-device-id fleet-token fleet-install-token-keys fleet-provisioner \
  step-ca step-ca-password nightfall-ledger nightfall-ledger-verify dawn-output-key \
  ceph-postgres-backup -o yaml > dusk-secrets.yaml
```

| Secret | Without a copy |
|--------|----------------|
| `fleet-device-id` | Every device id is derived from it. With a new key every device becomes a new device and the inventory, campaigns and ledger no longer match the machines. Cannot be undone. |
| `fleet-server-ca` (prod, cert-manager) or `pki-ca` (dev) | Every node trusts this CA alone; with a new one every node must be reinstalled with the new anchor. |
| `step-ca`, `step-ca-password`, and the offline `step-ca-root-key` | Node certificates no longer chain to the CA: every node fails to renew and enrolls again as a new installation, if its credential still allows it. |
| `fleet-token` | The fleet token is in every installer, so the value can be recovered from one, but nightfall refuses enrollments until `fleet-tokens.toml` holds its SHA-256 again. |
| `fleet-install-token-keys` | Public keys only; until the issuer's keys are restored, install tokens are refused. |
| `fleet-provisioner` | step-ca refuses nightfall's one-time tokens until a new key is generated and its public half put in `ca.json`. |
| `nightfall-ledger`, `nightfall-ledger-verify` | A new signing key works at once, but checkpoints signed with the old one verify only with its public key: keep every public key ever used. The param key cannot be recovered, and with it every ledger parameter hash before the loss can no longer be recomputed. |
| `dawn-output-key` | Output digests before the loss can no longer be recomputed. |
| `ceph-postgres-backup` | Needed to read the Postgres backups in a restore, unless the backup store has credentials of its own. |
| `ceph-*`, `postgres-*`, `clickhouse-*`, `grafana-admin`, `signoz-admin` | Nothing: delete the Secret, run secrets-init again, and Rook, CloudNativePG and the services take the new value. |

The Kafka users' Secrets come from Strimzi and the `*-tls` Secrets from
cert-manager; both are issued again from their CAs.

## Drills

Each drill below was run end to end. The Postgres drills ran on a kind cluster
with CloudNativePG 1.30.1, the Barman Cloud plugin v0.15.1, cert-manager
v1.21.2 and the development Ceph container as the object store, against the
`postgres` resources of the prod overlay with two instances; the Kafka drill
ran on the local stack. The times are those of a small
database: a restore's time grows with the size of the base backup and with the
WAL it replays.

### Lose Postgres

**What you see:** twilight is not ready (`the database is unreachable`) and
the UI and API fail. Nodes stay linked and dawn reaches them, and nightfall
and step-ca go on enrolling and renewing them, but no campaign moves.

**Recover:** restore the cluster, in place and under the same name, from its
backups.

1. Pick the point to restore to. Leave it out to restore everything the
   archive holds - when the cluster is gone. Set it to just before the damage
   when the data is what went wrong (a bad migration, a mistaken delete): add
   to `infra/k8s/overlays/prod/postgres-restore/recovery.yaml`, under
   `/spec/bootstrap/recovery`,

    ```yaml
        recoveryTarget:
          targetTime: "2026-10-16 16:52:50+00"
    ```

2. Delete what is left of the cluster. Its volumes go with it, and only one
   cluster may write to the archive:

    ```sh
    kubectl -n dusk delete cluster postgres
    ```

3. Restore:

    ```sh
    kubectl apply -k infra/k8s/overlays/prod/postgres-restore
    kubectl -n dusk wait --for=condition=Ready cluster/postgres --timeout=1h
    ```

    The overlay is the prod cluster with `bootstrap.recovery` from the
    `ObjectStore` `postgres-backups` in place of `initdb`. The restored cluster
    starts a new timeline and archives it to the same place; the annotation
    `cnpg.io/skipEmptyWalArchiveCheck` lets it, since the archive is not empty.
4. Apply the prod overlay again as you normally would. It puts the cluster's
   `bootstrap` back to `initdb`, which CloudNativePG ignores on a cluster that
   exists.
5. Bring the node states and the inventory back in line:

    ```sh
    kubectl -n dusk exec deploy/twilight -c twilight -- twilight node-state rebuild
    ```

    `dusk.node-state` still holds every lifecycle change made after the
    restore point. The rebuild writes each one the topic holds that is later
    than the restored inventory's into the inventory - a revocation made after
    the restore point stays a revocation - and publishes the inventory's where
    it is the later one ([The rebuild](#the-rebuild)).

**What the restore loses:** everything after the restore point that nothing
else holds:

* campaigns created, started or moved after it, and their rows: the engine
  carries on from the restored state, so a row dispatched after the restore
  point is dispatched again, at the same pid, and dawn finds the pid in the
  node's process table and reports `duplicate` - nothing runs twice;
* intended processes recorded after it. nightfall's admission still allows
  them while `dusk.intended-processes` holds them, but reconcile reads the
  ledger again from its restored position and raises `process_without_intent`
  for each call made under one. Those alerts name pids whose ledger entries
  fall between the restore point and the restore: check the entry's
  `principal` and its campaign against what was running then, and resolve
  them; an alert outside that window is real;
* alerts raised, acknowledged or resolved after it; API tokens created after
  it; sessions, so everyone signs in again.

**Drill:** 1000 rows written at 16:52:47 UTC, 1000 more at 16:52:53, the WAL
file holding them archived, the cluster deleted, and the overlay applied with
`targetTime: "2026-10-16 16:52:50+00"`:

```text
cluster.postgresql.cnpg.io/postgres created
cluster.postgresql.cnpg.io/postgres condition met
ready after 53 s
2 instances, 2 ready, timeline 2
ContinuousArchiving=True ContinuousArchivingSuccess
A|1000
```

The first 1000 rows were back and the second 1000 were not; the roles
`twilight`, `grafana` and `signoz` were restored with them. A base backup of the restored cluster succeeded at once,
from its standby. 1000 more rows, the cluster deleted again, and the overlay
applied without a target:

```text
ready after 68 s
A|1000
C|1000
timeline 3
```

Everything on the restored timeline came back, and nothing of the abandoned
one. Applying the prod overlay afterwards changed nothing but `bootstrap`.

### Lose Kafka's node state

**What you see:** after Kafka is lost, or `dusk.node-state` is deleted and
created again, a nightfall instance that starts finds no completion record on
the topic: `/readyz` answers `node state incomplete`, it logs once, at `warn`,
which partitions lack one, and it closes every node link and provisioning connection
before TLS, as it does before it has read the node states at all. An instance
that kept running holds the states it had read and keeps enforcing them.
Partitions added to the topic look the same to a running nightfall: it reads
every partition again and refuses new links and provisioning until the new
ones hold a completion record, while it keeps enforcing every state it holds or
reads on the links it has.

**Recover:** nothing to do by hand. twilight's leader compares the topic's id
and partition count, at every election and every minute after, with those of
the last check that found it complete, and reads the topic when they differ -
as they do once Kafka has created the topic again, or partitions were added. Finding a partition without
a completion record, it writes every lifecycle of the inventory to the topic,
then a completion record on every partition, and logs at `warn` that it wrote
the topic again. A check that fails is tried again after a backoff of at most
five minutes. To do it at once, or after [restoring Postgres](#lose-postgres):

```sh
kubectl -n dusk exec deploy/twilight -c twilight -- twilight node-state rebuild
```

**Drill:** the local stack with two nodes. One of them,
`18eeccf0c3b094f50a3e09587e9614aa`, was revoked through twilight's API;
nightfall closed its session 75 ms later and refused each reconnect
(`refused a node whose lifecycle forbids a session`). Then twilight was
stopped, so that it could not rebuild at once, and `dusk.node-state` was deleted
and created again, empty, by kafka-init. The nightfall that kept running went on
refusing the revoked node from the states it held. Restarted on the empty
topic, nightfall answered:

```text
HTTP/1.0 503 Service Unavailable
not ready: node state incomplete: a partition of the node-state topic holds no completion record
nightfall_handshakes_total{listener="fleet",outcome="not_ready"} 2
```

and refused both nodes. twilight was started at 09:43:18; its leader took over
at 09:44:01, when the stopped instance's lease ran out, and:

```text
09:44:01.364 twilight  rebuilding the node-state topic from the inventory  incomplete_partitions=[0] topic_states=0
09:44:01.432 twilight  the node-state topic is rebuilt from the inventory  published=1 seconds=0.127
09:44:01.435 nightfall read a node-state completion record  partition=0 offset=1
09:44:02.144 nightfall node state caught up  records=1 states=1 complete_partitions=1
```

The active node linked again at 09:44:24, at its next retry; the revoked node
was refused again at 09:44:15 and 09:45:16 with `lifecycle: revoked`. From
twilight's start to nightfall serving took 43 seconds, all of it the leader's
lease. `twilight node-state rebuild` run afterwards printed:

```text
dusk.node-state partitions: 1, without a completion record before the rebuild: 0
lifecycles already on the topic: 1
published from the inventory: 0
later on the topic, written into the inventory: 0
for nodes the inventory does not hold, kept on the topic: 0
completion records written on every partition; nightfall serves nodes once it has read them
```

#### The rebuild

The rebuild first reads every partition of the topic to the end offset it had
when the rebuild started. A partition that reads nothing for
`engine.node_state_stall_seconds` (120 seconds; `--stall-seconds` for the
command) before it gets there - its leader gone, or a transaction left open -
fails the rebuild before it publishes anything or writes a completion record;
the command exits with 1, and the leader tries again later. A read that keeps
moving is never cut short, however large the topic. Then it holds, for every device and
installation, the later of the topic's lifecycle and the inventory's, by when
each was set:

* the inventory's is later, or the topic has none: it is published, with the
  time and actor the inventory recorded;
* the topic's is later: it is written into the inventory - a node revoked or
  released after the inventory's restore point keeps that;
* the topic holds one for a node the inventory does not have: it stays on the
  topic, and the rebuild says so.

Each change is made under the lock every lifecycle change takes - through the
API, a quarantine campaign or a revocation by credential, each of which locks
the row, publishes and changes the inventory in one transaction - so no change
made during the rebuild is overwritten. The completion records go last, once
every change is acknowledged, and a record that does not land on the partition
it was meant for fails the rebuild. `twilight node-state rebuild` prints what
it did, as in the drill above.

After partitions are added, a key's records from before stay on the partition
it hashed to then, and its later ones go to the partition it hashes to now.
Of two records of one key on two partitions, the rebuild and nightfall keep
the one with the later time, and within one partition the later in the log.
Once the completion records are written - which proves the producer sees every
partition - the rebuild writes each key whose latest state sits on its old
partition again, with that state's time, on the partition it hashes to now.
Should that fail, the rebuild fails and `twilight node-state rebuild` moves
them; until then the order by time holds.

The command runs beside the serving process, in its container, with a soft
memory limit of 400 MiB (`--memory-limit`) so that the two stay within the
container's 4 GiB; the read holds every key of the topic, so for a topic of
millions of keys raise the Deployment's memory limit and `--memory-limit`
together first.

### Lose Kafka

Losing the whole cluster loses every topic. Kafka is recreated empty by
Strimzi's `KafkaTopic` resources (kafka-init in development); then, topic by
topic:

| Topic | What is lost | What brings it back |
|-------|--------------|---------------------|
| `dusk.node-state` | Nothing | twilight rebuilds it, above. Until then nightfall serves no node it restarts with. |
| `dusk.intended-processes` | The processes intended before the loss | twilight has them in `intended_processes`, but marks each published once Kafka took it. Until republished, nightfall refuses calls under them `denied: not intended` and their campaign rows fail and are retried under new pids. Republish them at once: `update intended_processes set published_revision = 0 where tombstoned_at is null and reaped_at is null and expires_at > now();` in the inventory database. |
| `dusk.ledger` | Entries Vector had not copied to ClickHouse, the lake and the evidence bucket: normally seconds | nightfall starts a new chain on the empty partition. Reconcile notices its stored position is past the partition's end, reads the partition from its start and raises `ledger_chain_broken` for each instance's new chain: expected after a loss; check that the evidence bucket holds each chain up to the loss, then resolve them. |
| `dusk.credential-quota` | The count of installations each capped credential enrolled | Nothing rebuilds it: only nightfall writes it, and it starts again from zero, so every capped credential may enroll up to its whole limit once more. Accepted because a quota bounds what one leaked credential can enroll, and a Kafka loss is rare and seen. The inventory still records the credential of every node (`credential_kind`, `credential_ref`): where a credential's count matters, lower its limit in nightfall's credential files until its new installations are accounted for, or revoke by credential. |
| `dusk.census`, `dusk.connections` | The online view | Every nightfall writes a full census within `census_interval_seconds` (300 s); twilight's view is degraded until then. |
| `dusk.enrollments` | Enrollments twilight had not read | A node that enrolled and linked is added to the inventory by its first connection. The enrollment itself stays only in ClickHouse and the lake if Vector had copied it. |
| `dusk.process-results` | Results twilight had not read | Their rows time out and follow their policy: an `EnsureVersion` row is resent from state; a `RunScript` row is shown as unknown, not run again. |
| `dusk.process-output`, `dusk.files` | Messages Vector had not copied | Nothing: they are copies; collected files themselves are in `dusk-files`. |
| `dusk.otel-*` | Telemetry Vector and SigNoz had not read | Nothing: three days of telemetry is not worth a backup. |

Consumer group positions go with Kafka; every consumer then starts from the
beginning of the new, empty topics, so nothing is skipped.

### Lose step-ca

step-ca runs as one replica and keeps the one-time token ids it accepted in a
badger database on its volume `database`, which nothing backs up. Its keys and
configuration are the Secrets `step-ca` and `step-ca-password`.

* **Its pod:** nothing is lost. The StatefulSet starts it again on the same
  volume; meanwhile nightfall answers enrollments and renewals
  `step_ca_error`, and nodes keep their certificates, valid up to 168 hours.
* **Its volume:** the list of used one-time token ids is gone. A fleet-token
  enrollment loses nothing - its one-time token is random and valid for five
  minutes - but every install token that has not yet expired can be used once
  more, by whoever holds a copy.

**Recover from a lost volume:**

1. Keep the keys: the Secrets `step-ca` and `step-ca-password` must survive,
   or be restored from your copy ([Secrets](#secrets)), before step-ca starts
   again; without them every node certificate stops chaining to the CA.
2. Let the StatefulSet create the volume again (`kubectl -n dusk delete pvc
   database-step-ca-0`, then delete the pod); step-ca starts with an empty
   database.
3. Close the window the empty database opens, before step-ca serves again or
   at once after: retire every install token that has not expired. Remove the
   keys that signed them from `install-token-jwks.json` in the Secret
   `fleet-install-token-keys` - nightfall reads the file again when it
   changes - and issue new tokens with a new key to the installs still
   waiting for one. Where the issuer cannot do that quickly, the tokens'
   expiry bounds the window instead.
4. Look for what used the window: enrollments with `credential_kind`
   `install_token` since the loss, by `credential_ref`, in ClickHouse's
   `dusk.enrollments`, and revoke what should not be there.

### Lose a whole cluster

Losing the cluster loses everything in it, the Rook object store included.
Recovering assumes three things kept elsewhere: the copy of the Secrets, the
Postgres backups (an `ObjectStore` endpoint outside the cluster, or a replica
of `dusk-postgres-backups`), and, for the ledger's evidence, a replica of
`dusk-ledger-evidence`. Without the first, nodes cannot come back as
themselves; without the second, the inventory starts empty.

In a new cluster:

1. Install the operators ([the prod overlay](deploy.md#the-prod-overlay)).
2. Create the namespace `dusk` and apply the copy of the Secrets **before
   anything else runs**: secrets-init creates whatever is missing, and a new
   `fleet-device-id` makes every device a new one.
3. Restore the cert-manager CAs `fleet-server-ca` and `internal-ca` from your
   copy if you keep one; otherwise nodes need a new trust anchor.
4. Point the `ObjectStore` at the surviving backups and apply the
   `postgres-restore` overlay ([Lose Postgres](#lose-postgres)), then the prod
   overlay. Kafka starts empty: twilight's leader rebuilds `dusk.node-state`
   within a minute, and republish the intended processes ([Lose
   Kafka](#lose-kafka)).
5. step-ca starts with an empty database: retire the install tokens that have
   not expired ([Lose step-ca](#lose-step-ca), step 3).
6. Point DNS for `fleet.` and `provision.` at the new nightfall Service.

Nodes reconnect with the certificates they hold: the same fleet-server anchor
and the same step-ca chain mean no node enrolls again, and the same device id
key means a node that does enroll again keeps its device id. What the backups
do not hold - the lake, collected files, SigNoz's cold tier, ClickHouse beyond
what the lake refills - is lost with the site.
