# Ledger chain broken

Kind `ledger_chain_broken`, severity `critical`. twilight's reconcile raises it
when an entry it reads from the Kafka topic `dusk.ledger` breaks its chain: the
hash, the link to the entry before it, the sequence, a chain start or link, or a
checkpoint signature does not verify, or entries went too long without a
checkpoint. The detail carries `message`, `break` (what kind of break),
`instance` (the nightfall instance whose chain it is), `partition` and
`sequence` (the ledger partition and the entry's place in the chain), and
`kafka_partition` and `offset` (where the record is in Kafka). One alert stays
open per nightfall instance, partition and kind of break. It never resolves by
itself.

## What it means

The ledger is the record of every call that crossed nightfall, hash-chained and
signed ([the ledger](../ledger.md)). A break means an entry was edited, removed,
reordered or forged, or that the chain or the keys twilight checks it with do
not match what nightfall wrote. Until you know which, treat that partition's
entries from the break on as unproven.

| `break` | Look at |
|---------|---------|
| `hash_mismatch`, `previous_hash_mismatch`, `sequence_gap`, `out_of_order`, `conflicting_entry`, `stale_entry`, `invalid_chain_start`, `unlinked_chain_start`, `chain_link_mismatch`, `chain_resumed_mismatch`, `forged_checkpoint` | The entries themselves; each is described in [the ledger's breaks](../ledger.md#verifying). |
| `unknown_key` | A checkpoint signed with a key twilight does not have. After a nightfall key rotation, `reconcile.ledger_keys` lacks the new public key. |
| `unsigned_tail` | Entries went more than twice `reconcile.checkpoint_interval_ms` without a checkpoint after them. nightfall signs every `ledger.checkpoint_interval_ms` and when it shuts down cleanly: check that the two settings match and whether the instance stopped uncleanly then. |
| `wrong_partition` | An entry of one ledger partition was written to another Kafka partition. Check `instance` and `ledger.partition` of the nightfall instances: each must write its own partition. |

## How to confirm

Read the alert:

```sh
curl -fsS -H "Authorization: Bearer $TOKEN" "$TWILIGHT/api/v1/alerts/<id>"
```

Verify the Kafka partition with nightfall's own verifier, from the nightfall pod,
starting a few thousand offsets before the alert's `offset`. The public keys are
in the Secret `nightfall-ledger-verify`; `--keys /dev/stdin` reads them from the
pipe:

```sh
kubectl -n dusk get secret nightfall-ledger-verify -o jsonpath='{.data.ledger-verify-jwks\.json}' | base64 --decode \
  | kubectl -n dusk exec -i nightfall-0 -- nightfall verify-ledger --keys /dev/stdin \
      --brokers kafka:9092 --partition <kafka_partition> --start <offset minus a few thousand>
```

In the prod overlay Kafka needs TLS; pass the settings nightfall itself uses:

```sh
  ... --brokers dusk-kafka-bootstrap:9093 \
      --property security.protocol=ssl \
      --property ssl.ca.location=/etc/nightfall/kafka/ca.crt \
      --property ssl.certificate.location=/etc/nightfall/kafka/user.crt \
      --property ssl.key.location=/etc/nightfall/kafka/user.key
```

It prints `ledger verified: ...` or `ledger verification failed: ...` followed by
every break, and exits non-zero on a break ([verifying](../ledger.md#verifying)).
`nightfall verify-ledger --help` lists the other options (`--end`, `--topic`,
`--timeout-seconds`, `--input` for a file of JSON lines).

ClickHouse's copy of the same chain, in Grafana's **Ledger** dashboard (*Chain
status*) or directly ([where to run SQL](index.md#sql)):

```sql
SELECT *
FROM dusk.ledger_chain(since = now64(9) - INTERVAL 1 DAY)
WHERE instance = '<instance>' AND partition = <partition>
```

The evidence copy, which cannot be rewritten, is in the object-locked bucket
`dusk-ledger-evidence` under `ledger/dt=<date>/`, as zstd-compressed JSON lines,
readable with the object store user `admin` (Secret `ceph-admin`).

## What to do

* **`unknown_key`** after a key rotation: add the new public key to
  `ledger-verify-jwks.json` in the Secret `nightfall-ledger-verify`, keeping the
  old ones, and restart twilight, which reads the keys when it starts:
  `kubectl -n dusk rollout restart deployment twilight`.
* **`unsigned_tail`** with the instance's restart at that time, or with the two
  interval settings apart: fix the setting; nothing was altered.
* **`wrong_partition`**: fix the nightfall instance's `ledger.partition` or
  `instance` and restart it.
* **Any other break** that the verifier also finds in Kafka: the ledger itself
  was altered or forged. Keep the evidence copy and the Kafka partition as they
  are (Kafka keeps `dusk.ledger` for 30 days), compare the two around the
  sequence in the alert, and escalate: whoever did it controls Kafka or a
  nightfall instance ([compromised components](../security.md#compromised-components)).

## How to resolve

Resolve it by hand once the cause is known and fixed: *Resolve* on the UI's
Alerts page, or `POST /api/v1/alerts/<id>/resolve`. A break found again after
that opens a new alert.
