# Ledger chain broken in ClickHouse

Kind `clickhouse_ledger_chain_broken`, severity `critical`. Grafana's rule
`dusk-ledger-chain-broken` checks ClickHouse's copy of the ledger,
`dusk.ledger`, through the view `dusk.ledger_chain` over the last hour, and
fires when a chain - one nightfall instance and partition - has had a gap, a
broken link or two entries at one sequence for 10 minutes. It resolves by
itself once the last hour of every chain is intact again.

## What it means

ClickHouse holds a copy of the ledger that Vector loads from Kafka. The view
orders each chain's entries by sequence and counts `gaps` (missing sequence
numbers), `broken_links` (an entry whose `previous_hash` is not the hash of
the entry before it) and `conflicting_entries` (two entries at one sequence).
The view does not check hashes or signatures. A break here is in the copy;
whether the ledger itself is broken is what twilight's reconcile and
`nightfall verify-ledger` decide, from Kafka:

* twilight also has an open [`ledger_chain_broken`](ledger_chain_broken.md)
  alert for the same instance and partition: the ledger itself is broken. Follow
  that page.
* twilight has none: the ledger is intact and the copy is not. Gaps usually
  mean Vector dropped ledger records that failed their contract
  ([messages dropped by Vector](vector_messages_dropped.md)); Vector copies the
  evidence bucket straight from Kafka, so the evidence copy keeps them.

## How to confirm

The chains that are not intact, in Grafana's **Ledger** dashboard (*Chain
status*), or with the **ClickHouse** data source ([where to run SQL](index.md#sql)):

```sql
SELECT instance, partition, entries, first_sequence, last_sequence, gaps, broken_links, conflicting_entries, last_checkpoint_sequence, last_entry_time
FROM dusk.ledger_chain(since = now64(9) - INTERVAL 1 HOUR)
WHERE NOT intact
```

twilight's open alerts, for a `ledger_chain_broken` on the same chain:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts?state=open&limit=500" \
  | jq '.items[] | select(.kind == "ledger_chain_broken") | {id, detail}'
```

Vector's drops of ledger records:

```sh
kubectl -n dusk logs deploy/vector --since=2h | grep 'dropped a message that fails its contract schema' | grep dusk.ledger
```

To settle it, verify the Kafka partition with `nightfall verify-ledger`, as in
[ledger chain broken](ledger_chain_broken.md#how-to-confirm).

## What to do

* The Kafka ledger verifies: the ledger is intact. Fix what kept records out of
  ClickHouse - see [messages dropped by Vector](vector_messages_dropped.md) - and
  do not use ClickHouse's copy of that stretch as evidence; use Kafka (30 days)
  or the evidence bucket `dusk-ledger-evidence` instead.
* The Kafka ledger does not verify: follow
  [ledger chain broken](ledger_chain_broken.md#what-to-do).

## How to resolve

Nothing to do in Grafana: the rule looks at the last hour, so it resolves once
the broken stretch is more than an hour old and no new break has appeared.
