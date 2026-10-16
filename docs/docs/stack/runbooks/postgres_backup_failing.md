# Postgres backup failing

Kind `postgres_backup_failing`, severity `high`. Grafana's rule
`dusk-postgres-backup-failing` fires when the inventory's Postgres cluster has
no base backup from the last 26 hours, or when its last failed backup is more
recent than its last good one, by the Barman Cloud plugin's metrics in
`dusk.otel_metrics` (`barman_cloud_cloudnative_pg_io_last_available_backup_timestamp`
and `..._last_failed_backup_timestamp`, `service_name` `postgres`) of the last
15 minutes. It resolves by itself once a base backup succeeds. Only the prod
overlay backs Postgres up and exports them, so the rule stays silent in the
local stack and the dev overlay.

The prod overlay adds a second rule with this kind,
`dusk-postgres-metrics-missing`, from
`infra/k8s/overlays/prod/dusk/configuration/grafana-postgres-alerts.yaml`: it
fires while no Postgres instance has sent the plugin's last-backup metric or
the WAL archive status (`cnpg_collector_pg_wal_archive_status`) for 15
minutes - then nothing watches the backups or the archiving - and keeps firing
for as long as they stay away.

## What it means

The prod overlay takes a base backup of the CloudNativePG cluster `postgres`
every day at 02:30 (`ScheduledBackup` `postgres-daily`) and archives its WAL
continuously, both to the bucket `dusk-postgres-backups`
([Backups and disaster recovery](../disaster-recovery.md#postgres)). A restore
starts from a base backup and replays WAL from there. While WAL archiving
works, nothing is lost yet: every change can still be restored from the last
good base backup onwards. What a failing backup costs is time and reach:

* a restore replays every WAL file since the last good base backup, so it takes
  longer the older that backup is;
* retention (`retentionPolicy: 30d`) always keeps the newest good base backup
  and the WAL it needs, but drops the older backups as they pass 30 days, so
  the earliest point a restore can reach moves forward while no new backup
  succeeds.

## How to confirm

The backups and their errors:

```sh
kubectl -n dusk get backups --sort-by=.metadata.creationTimestamp
kubectl -n dusk describe backup <the failed one>
kubectl -n dusk get scheduledbackup postgres-daily
kubectl -n dusk get cluster postgres -o jsonpath='{range .status.conditions[*]}{.type}={.status} {.message}{"\n"}{end}'
```

`LastBackupSucceeded=False` names the failure. If the alert is
`dusk-postgres-metrics-missing`, the metrics stopped instead: check that the
instances run, the collector's scraper (its `postgres` job, in
`postgres-scrape.yaml`), the Service `postgres-metrics` and the network
policies that let the scraper reach port 9187. The plugin's sidecar on the
instance that took the backup has the details of a failure:

```sh
kubectl -n dusk logs <instance pod> -c plugin-barman-cloud --since=26h | grep -i -E 'error|fail'
```

## What to do

1. Fix what the error names:
    * Access denied or invalid credentials: the Secret `ceph-postgres-backup`
      no longer matches the object store user `postgres-backup`, or the
      bucket policy changed. ceph-init writes the policy; run it again
      (`kubectl -n dusk delete job ceph-init` and apply the overlay).
    * No such bucket: ceph-init creates `dusk-postgres-backups` when it runs
      again.
    * The object store does not answer: bring Rook's gateway back.
    * The plugin is not installed or its certificates expired: check the
      `barman-cloud` Deployment in `cnpg-system`.
    * No space: the instance's volume or the object store is full.
2. Take a backup now rather than waiting for 02:30:

    ```sh
    kubectl -n dusk apply -f - <<'EOF'
    apiVersion: postgresql.cnpg.io/v1
    kind: Backup
    metadata:
      name: postgres-manual
    spec:
      method: plugin
      pluginConfiguration:
        name: barman-cloud.cloudnative-pg.io
      cluster:
        name: postgres
    EOF
    kubectl -n dusk wait --for=jsonpath='{.status.phase}'=completed backup/postgres-manual --timeout=1h
    ```

3. Delete the manual Backup resource once it has completed; the backup itself
   stays in the bucket until retention removes it.

If no base backup succeeded for a long time, note in the incident how far back
a restore can still reach: the newest good backup's time, from `kubectl -n dusk
get backups`.

## How to resolve

Nothing to do in Grafana: `dusk-postgres-backup-failing` resolves at its next
evaluation once a base backup newer than 26 hours exists and no failure follows
it, `dusk-postgres-metrics-missing` once the metrics arrive.
