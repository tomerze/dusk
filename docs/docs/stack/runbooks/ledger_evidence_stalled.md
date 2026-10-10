# Ledger evidence copy stalled

Kind `ledger_evidence_stalled`, severity `critical`. Grafana's rule
`dusk-ledger-evidence-stalled` fires when ledger entries reached ClickHouse
between ten and five minutes ago but Vector's `evidence_ledger` sink sent no
event in the last five minutes, by Vector's own metrics in
`dusk.otel_metrics`. It resolves by itself once the sink sends again.

## What it means

Vector copies every committed ledger record from Kafka into the object-locked
bucket `dusk-ledger-evidence`, with credentials that may only add objects; that
copy cannot be rewritten, and it is what shows a ledger tail removed later
([the ledger](../security.md#the-ledger)). While it is stalled, new ledger
entries exist only in Kafka and ClickHouse. Vector reads them for the evidence
copy in a consumer group of its own (`vector-ledger-evidence`) and acknowledges
them only once the sink has written them, so the copy catches up when the sink
works again - as long as Kafka still holds the records (`dusk.ledger` is kept
for 30 days).

The rule's own annotation names the usual causes: the `ledger-writer`
credentials, the bucket, its object lock configuration. The object store being
down, or Vector's metrics not reaching ClickHouse, look the same.

## How to confirm

Vector's own count of what the sink sent, straight from its metrics port:

```sh
kubectl -n dusk port-forward deploy/vector 9598:9598
curl -s http://127.0.0.1:9598/metrics | grep 'component_id="evidence_ledger"'
```

`vector_component_sent_events_total` for `evidence_ledger` not rising while
ledger entries arrive confirms the stall. If it is rising, the sink works and
the metrics pipeline (the collector scraping Vector into ClickHouse) is what
stopped.

Vector's log for the sink's errors:

```sh
kubectl -n dusk logs deploy/vector --since=1h | grep evidence_ledger
```

The Grafana **Alerts** dashboard plots *Evidence copies per second*.

## What to do

1. Fix what the log names:
    * Access denied: the Secret `ceph-ledger-writer` (`access-key`,
      `secret-key`, mounted at `/run/secrets/evidence/`) no longer matches the
      object store user `ledger-writer`, or the bucket policy changed. ceph-init
      writes the policy; run it again (`kubectl -n dusk delete job ceph-init`
      and apply the overlay) to restore it.
    * The bucket is missing: ceph-init creates it when it runs again. A bucket
      without object lock cannot hold evidence: ceph-init refuses it and says
      so in its log.
    * The object store does not answer: bring Ceph (Rook in the prod overlay)
      back.
2. Restart Vector if it does not recover by itself once the cause is gone:
   `kubectl -n dusk rollout restart deployment vector`.
3. Watch `vector_component_sent_events_total` for `evidence_ledger` climb as the
   backlog is written.

If the stall lasted long enough that Kafka may have dropped records Vector had
not yet copied, the evidence copy has a hole for that time; note it with the
incident.

## How to resolve

Nothing to do in Grafana: the rule resolves at its next evaluation once the
sink has sent events in the last five minutes.
