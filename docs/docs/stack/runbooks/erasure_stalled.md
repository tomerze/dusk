# Erasure stalled

Kind `erasure_stalled`, severity `high`. The twilight leader raises it, at most
every five minutes, for an erasure that is still running
`retention.erasure_overdue_days` (20) days after it was asked for, or whose
current step has failed five times in a row. The detail carries `message`,
`erasure_id`, `scope` (`device` or `tenant`), `subject` (the device id or the
tenant), `requested_at`, `attempts` and `last_error`. One alert stays open per
erasure. It resolves by itself, with `twilight` as the resolver, when the
erasure finishes.

## What it means

An erasure deletes a device's or a tenant's data step by step and goes on from
where it stopped after every failure
([erasing a device](../data.md#erasing-a-device)). While it is stalled, the data
its unfinished steps would delete is still there. If you promised to erase
within a period - often 30 days - the time left is that period minus the age
of the erasure.

## How to confirm

The erasure, with the state of each step, what it did, and why the current one
failed:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/erasures/<erasure_id>"
```

The first step whose `state` is not `done` is the one stalled; `attempts` and
`last_error` say how often and why it failed. Reading an erasure needs the
`admin` role. twilight's log has every attempt:

```sh
kubectl -n dusk logs -l app.kubernetes.io/name=twilight -c twilight --prefix --since=24h | grep <erasure_id>
```

## What to do

1. Fix what `last_error` names, by step:
    * `scope`, `clickhouse`: twilight reaches ClickHouse at `clickhouse.url` as
      the user `twilight`, with the password in the Secret
      `clickhouse-twilight`. Check that ClickHouse answers and that the user
      exists with its grants (`users.d/dusk.yaml` in
      `infra/k8s/base/clickhouse/`). A delete that keeps failing shows its
      reason in ClickHouse:

        ```sql
        SELECT table, command, latest_fail_reason FROM system.mutations
        WHERE database = 'dusk' AND NOT is_done AND position(command, '<erasure_id>') > 0;
        ```

    * `lake`: an error from ClickHouse that names the object store means the
      `lake_writer` collection (the `ceph-vector` keys) cannot write to
      `dusk-lake`; an error from the object store itself means twilight's user
      (`ceph-twilight`) cannot delete there. ceph-init writes the bucket
      policies; run it again (`kubectl -n dusk delete job ceph-init` and apply
      the overlay) to restore them.
    * `files`: twilight's user needs `s3:ListBucket` and `s3:DeleteObject` on
      `dusk-files`; run ceph-init again as above.
    * `inventory` waiting for intended processes: the leader withdraws them
      from `dusk.intended-processes` every 5 seconds. Check that one instance
      has `twilight_leader` 1 and that it reaches Kafka.
2. Nothing needs restarting: the erasure goes on at its next attempt, at most
   an hour after the last failure.

## How to resolve

Nothing to do: the alert resolves when the erasure finishes. Resolving it by
hand while the erasure is still stalled only raises it again at the next check.
