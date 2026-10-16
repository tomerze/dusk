# Postgres WAL archiving lagging

Kind `postgres_wal_archiving_lagging`, severity `high`. Grafana's rule
`dusk-postgres-wal-archiving-lagging` fires when, for five minutes, the
inventory's Postgres primary has failed to archive the WAL file it tried last
(`cnpg_pg_stat_archiver_last_failed_time` after
`cnpg_pg_stat_archiver_last_archived_time`) or holds more than ten WAL files it
has not archived (`cnpg_collector_pg_wal_archive_status{value="ready"}`), by the
instances' metrics of the last 10 minutes in `dusk.otel_metrics`
(`service_name` `postgres`). It resolves by itself once archiving catches up.
Only the prod overlay archives WAL and exports them, so the rule stays silent
in the local stack and the dev overlay. When the metrics stop arriving, the
prod overlay's `dusk-postgres-metrics-missing` fires instead, as
[Postgres backup failing](postgres_backup_failing.md) says.

## What it means

The prod overlay archives every WAL file of the CloudNativePG cluster
`postgres` to the bucket `dusk-postgres-backups`, and switches to a new WAL file
at least every minute (`archive_timeout: 60s`), so a restore loses at most the
last minute of inventory changes
([Backups and disaster recovery](../disaster-recovery.md#postgres)). While
archiving lags, everything since the last archived file exists only on the
cluster's own volumes: losing them loses those changes, lifecycle changes
included - though a lifecycle change is also on `dusk.node-state`, which
`twilight node-state rebuild` writes back into a restored inventory.

Unarchived WAL also stays on the primary's volume. Left alone, it fills the
volume and Postgres stops accepting writes.

One failed attempt right after a primary starts - a failover, or a restore
that promotes a new timeline - is normal: Postgres retries every file until it
is archived, and the rule waits five minutes before firing.

## How to confirm

On the primary:

```sh
PRIMARY=$(kubectl -n dusk get cluster postgres -o jsonpath='{.status.currentPrimary}')
kubectl -n dusk exec $PRIMARY -c postgres -- psql -c 'select * from pg_stat_archiver'
kubectl -n dusk exec $PRIMARY -c postgres -- bash -c 'ls /var/lib/postgresql/data/pgdata/pg_wal/archive_status | grep -c ready'
kubectl -n dusk get cluster postgres -o jsonpath='{range .status.conditions[*]}{.type}={.status} {.message}{"\n"}{end}'
```

`last_failed_wal` and `last_failed_time` name the file that fails;
`ContinuousArchiving=False` gives CloudNativePG's view. The plugin's sidecar
logs each attempt:

```sh
kubectl -n dusk logs $PRIMARY -c plugin-barman-cloud --since=1h | grep -i -E 'error|fail'
```

## What to do

1. Fix what the sidecar's log names - the causes are those of a failing
   backup: the credentials in `ceph-postgres-backup`, the bucket or its policy
   (run ceph-init again), the object store being down, the plugin in
   `cnpg-system`. See [Postgres backup failing](postgres_backup_failing.md).
2. Watch the backlog drain: `ready` falls and `archived_count` climbs as
   Postgres retries each file.
3. If the primary's volume is close to full and the cause cannot be fixed
   quickly, grow the volume (`spec.storage.size` of the Cluster); never delete
   files from `pg_wal` by hand.

Once archiving works again, take a base backup if the outage was long (see
[Postgres backup failing](postgres_backup_failing.md)): a restore then
replays less WAL.

## How to resolve

Nothing to do in Grafana: the rule resolves at its next evaluation once the
last attempt succeeded and ten or fewer files wait.
