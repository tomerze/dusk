# Messages dropped by Vector

Kind `vector_messages_dropped`, severity `high`. Grafana's rule
`dusk-contract-messages-dropped` fires when Vector's
`vector_component_discarded_events_total` rose in the last ten minutes, by
Vector's metrics in `dusk.otel_metrics`. It resolves by itself once ten minutes
pass without a drop.

## What it means

Vector checks every message it loads from Kafka into ClickHouse and the lake
against its contract and drops one that fails it. Each drop is logged at `warn`
with its `topic`, `partition` and `offset`: `dropped a message that fails its
contract schema` (with the `error`) for the event topics, `dropped a record that
is not an OTLP JSON ... export` for the telemetry topics. A dropped message is
missing from ClickHouse and the lake but stays in Kafka for the topic's
retention. The evidence copy of the ledger is taken from Kafka before that
check, so it keeps every ledger record.

The topic says who wrote the message:

| Topic | Producer |
|-------|----------|
| `dusk.ledger`, `dusk.connections`, `dusk.enrollments` | nightfall |
| `dusk.process-results`, `dusk.process-output`, `dusk.files` | dawn |
| `dusk.otel-logs`, `dusk.otel-spans`, `dusk.otel-metrics` | the OTel collector |

One cause is a producer upgraded to a message its consumers do not know yet: a
changed message is a new contract version, and every consumer must accept it
before any producer writes it
([upgrades and rollout order](../deploy.md#upgrades-and-rollout-order)).

## How to confirm

Vector's log of the drops:

```sh
kubectl -n dusk logs deploy/vector --since=30m | grep 'dropped a'
```

The Grafana **Alerts** dashboard plots *Messages dropped per second*. The
metric counts every event a Vector component discarded; if the log holds no
`dropped a` line, look at Vector's other warnings and errors. The contracts
are in `services/contracts/kafka/` in the repository, one schema per topic; the
`error` in the log says what failed.

## What to do

1. Find the producer from the topic and what changed: a rollout of nightfall,
   dawn or the collector just before the first drop.
2. Bring Vector to a version whose schemas accept the new messages, or roll the
   producer back.
3. For `dusk.ledger` drops, ClickHouse's copy of the ledger now has gaps and
   [ledger chain broken in ClickHouse](clickhouse_ledger_chain_broken.md) may
   follow; the ledger in Kafka and the evidence bucket is intact.

The dropped messages are not loaded again by themselves: Vector has moved past
them. They are in Kafka at the logged partition and offset until the topic's
retention removes them.

## How to resolve

Nothing to do in Grafana: the rule resolves once Vector has dropped nothing for
ten minutes.
