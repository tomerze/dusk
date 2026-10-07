CREATE TABLE IF NOT EXISTS dusk.process_output ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    id UUID,
    schema LowCardinality(String),
    pid UInt64,
    campaign_id String,
    device_id String,
    installation_id String,
    namespace_id String,
    `index` UInt64,
    value String,
    truncated Bool,
    kafka_partition UInt32,
    kafka_offset UInt64
)
ENGINE = ReplicatedReplacingMergeTree
PARTITION BY toYYYYMM(time)
ORDER BY (campaign_id, pid, `index`)
TTL time + INTERVAL 30 DAY DELETE;
