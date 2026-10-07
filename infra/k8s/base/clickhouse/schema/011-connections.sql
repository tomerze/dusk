CREATE TABLE IF NOT EXISTS dusk.connections ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    id UUID,
    schema LowCardinality(String),
    event LowCardinality(String),
    device_id String,
    installation_id String,
    namespace_id String,
    epoch UInt64,
    instance LowCardinality(String),
    inner_address LowCardinality(String),
    remote_address String,
    tenant LowCardinality(Nullable(String)),
    cert_fingerprint String,
    cert_not_after DateTime64(9, 'UTC'),
    connected_at DateTime64(9, 'UTC'),
    disconnect_reason LowCardinality(Nullable(String)),
    kafka_partition UInt32,
    kafka_offset UInt64
)
ENGINE = ReplicatedReplacingMergeTree
PARTITION BY toYYYYMM(time)
ORDER BY (device_id, installation_id, time, id)
TTL time + INTERVAL 30 DAY DELETE;
