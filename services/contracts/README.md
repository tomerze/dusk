# Dusk stack contracts

The messages the services of the Dusk stack - nightfall, dawn and twilight - exchange
over Kafka. Each topic has one JSON Schema (draft 2020-12) in
`kafka/<topic>.schema.json` and fixtures in `kafka/examples/<topic>/`.

## Topics

| topic | key | producer | consumers | cleanup | partitions dev / prod |
|-------|-----|----------|-----------|---------|-----------------------|
| `dusk.connections` | `<device_id>/<installation_id>/<namespace_id>` | nightfall | twilight, nightfall, vector | delete, 7 d | 3 / 48 |
| `dusk.census` | `<instance>/header`, `<instance>/<generation>/<index>` | nightfall | twilight, nightfall | compact | 1 / 6 |
| `dusk.ledger` | `<instance>` | nightfall | vector, twilight | delete, 30 d | 3 / 32 |
| `dusk.enrollments` | `<device_id>/<installation_id>` | nightfall | twilight, vector | delete, 30 d | 3 / 12 |
| `dusk.node-state` | `device/<device_id>`, `installation/<device_id>/<installation_id>` | twilight | nightfall | compact | 1 / 6 |
| `dusk.process-results` | `<device_id>/<installation_id>` | dawn | twilight, vector | delete, 7 d | 3 / 48 |
| `dusk.process-output` | `<device_id>/<installation_id>` | dawn | vector | delete, 7 d | 3 / 24 |
| `dusk.files` | `<device_id>/<installation_id>` | dawn | vector | delete, 30 d | 3 / 12 |
| `dusk.intended-processes` | `<device_id>/<installation_id>/<pid>` | twilight | nightfall | compact | 3 / 12 |
| `dusk.otel-logs`, `dusk.otel-spans`, `dusk.otel-metrics` | none | otel collector | vector, signoz | delete, 3 d | 3 / 24 |

In prod every topic has replication factor 3 and `min.insync.replicas` 2.

* `dusk.census` also sets `max.message.bytes` 2097152, `segment.ms` 600000,
  `min.cleanable.dirty.ratio` 0.1 and `delete.retention.ms` 3600000. nightfall writes
  every record of one instance to partition `murmur2(instance) mod partition_count`
  (the hash of Kafka's Java default partitioner). A chunk holds at most 2000 sessions
  and 512 KiB of JSON; the schema enforces the 2000, the producer the bytes.
* `dusk.ledger` sets `unclean.leader.election.enable` false. Each nightfall instance
  writes to its own partition, by default the one numbered by its StatefulSet ordinal,
  in transactions, so consumers read it with `isolation.level=read_committed`. The prod
  partition count is at least the largest number of nightfall replicas.
* `dusk.intended-processes` holds every process twilight intends: under its key, the
  latest record of it, written before twilight asks dawn for the process and again on
  every resend, and a tombstone once it expires or twilight reaps it. nightfall reads
  every partition from the beginning and refuses a call that no record allows. The
  topic sets `segment.ms` 600000, `min.cleanable.dirty.ratio` 0.1 and
  `delete.retention.ms` 3600000, so compaction keeps it close to the processes still
  intended.
* The otel topics set `max.message.bytes` 4194304. The OTel collector writes them in
  its own `otlp_json` format, so they have no schema here.

In dev (compose and the dev overlay) `kafka-init` creates the topics; in prod Strimzi
`KafkaTopic` resources do. Nothing else creates one: brokers run with
`auto.create.topics.enable=false` and every client with `allow.auto.create.topics=false`.

## Messages

Every message is one JSON object per Kafka record and starts with the same three
fields:

* `schema`: `<topic>/v1`, a constant in each topic's schema. A later version of a
  topic's messages is `<topic>/v2`, then `/v3` (see [Changing a message](#changing-a-message)).
* `id`: a lowercase UUID version 7.
* `time`: RFC 3339 in UTC with exactly nine fractional digits,
  `2026-10-06T09:14:03.512408117Z`. Every other timestamp field has the same format.

`kafka/common.schema.json` holds the definitions every topic shares - `id`, the
timestamp, device and installation ids (32 lowercase hex digits), namespace ids (16
lowercase hex digits), pids (a u64 as a decimal string, `0` to
`18446744073709551615`, since a JSON number cannot hold every u64 exactly), hashes (64
lowercase hex digits) and the rest - and the topic schemas refer to them with `$ref`
relative to their own file, so a validator that loads the schemas from disk resolves
them without a network.

Epochs, census generations and the ledger's partitions, sequences and capability ids are
at most 2^53 (9007199254740992), up to which an IEEE 754 double holds every integer
exactly: nightfall hashes ledger entries as RFC 8785 bytes, which write every number as a
double.

Every field of a topic is in every message. A field with no value is `null`, never
absent, and a message with a field its schema does not name is invalid. Every object
has `additionalProperties: false` except three open ones: the ledger's `event_detail`,
the process-results `reported.facts` (any key but `dusk.device.id`) and the
process-output `value` (any JSON). Where one field decides another - for example the
ledger's `kind` and `event`, the census `record` or a connection's `event` - the schema
says so with `if`/`then`, so a validator reports each broken rule as its own error.

`format` is an annotation only, as draft 2020-12 defines it by default; the `pattern`
next to it is what every validator enforces.

A tombstone (a record with a null value) has no message to validate: the census
tombstones the chunk keys of a generation it has replaced, a node-state tombstone
means the node is back to its default lifecycle, and an intended-processes tombstone
means twilight no longer intends a process at that pid on that node: it expired or was
reaped. Consumers handle a null value before validating.

Every consumer must validate each message against its topic's schema and drop one that
fails, counting it in a metric and logging a `warn` with the record's topic, partition
and offset. Every producer and consumer must have tests that check its messages against
these files: Rust with the `jsonschema` crate (a dev-dependency without default features,
with `resolve-file`), Go with `github.com/santhosh-tekuri/jsonschema/v6`, Python with
`jsonschema`.

## Validating

From the repository root:

```bash
uv run --no-project --with jsonschema --with referencing python services/contracts/validate.py
uv run --no-project --with pytest --with jsonschema --with referencing pytest services/contracts/tests -q
```

`--no-project` keeps uv from building the repository's own Python package, which
`validate.py` does not need.

`validate.py` prints every failure and exits 1 when there is one. It checks that:

* every schema is a valid draft 2020-12 schema, and every topic has examples and every
  examples directory a schema;
* every example whose name does not start with `invalid-` passes its schema;
* every `invalid-*.json` fails its schema with exactly one error, so each one shows
  that one rule holds;
* every value of every `enum` in a schema, `null` included, appears in at least one
  valid example.

## Changing a message

A `<topic>/v1` schema never changes once it is released. Any change to a topic's
messages, an added field or enum value included, is a new schema whose `schema` value
is `<topic>/v2`, in `kafka/<topic>.v2.schema.json` beside the v1 file and with its
examples in `kafka/examples/<topic>.v2/`. `common.schema.json` is part of every released
schema, so a definition in it never changes either; a new version that needs a
different one adds a new definition.

Every field is required and no other field is allowed, so a consumer that knows only v1
drops every v2 message. A new version therefore reaches every consumer before any
producer:

1. **Schema.** Copy `kafka/<topic>.schema.json` to `kafka/<topic>.v2.schema.json`, set
   its `schema` constant to `<topic>/v2` and make the change there. A new field goes in
   `properties` and in `required`. A field that can have no value allows `null`:
   `"type": ["string", "null"]`, `"anyOf": [{"$ref": ...}, {"type": "null"}]`, or
   `null` as a value of its `enum`. An id, hash or timestamp refers to its definition in
   `common.schema.json`. If a field's value depends on another field, add an
   `if`/`then` beside the ones already there.
2. **Examples.** Copy `kafka/examples/<topic>/` to `kafka/examples/<topic>.v2/` with
   every `schema` value set to `<topic>/v2`, and apply the change to every valid
   example. Give every new `enum` value a valid example that uses it. Add an
   `invalid-*.json` for each rule the change brings - a wrong type, a wrong pattern, a
   value outside the `enum`, a missing field - each a copy of a valid example with that
   one defect.
3. **Validate.** Add `<topic>.v2` to the topics in `tests/test_contracts.py` and run
   both commands above.
4. **Consumers.** Make every consumer in the topic's row of the table accept both
   versions, validating each message against the schema its `schema` field names, with
   contract tests for each: nightfall, twilight, the topic's Vector remap and its
   `vector test` cases, its ClickHouse table and its lake Parquet schema.
5. **Producers.** Once every consumer accepts v2, switch the topic's producer to it.
6. **Removal.** Once the topic holds no v1 message, remove v1 support from the
   consumers. Keep `kafka/<topic>.schema.json` and `kafka/examples/<topic>/` as long as
   the ledger evidence bucket or the lake can hold v1 data; the evidence bucket keeps
   ledger entries 400 days in prod. Then delete them and remove `<topic>` from the
   topics in `tests/test_contracts.py`.
