CREATE TABLE IF NOT EXISTS dusk.process_results ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    id UUID,
    schema LowCardinality(String),
    pid UInt64,
    campaign_id String,
    attempt Nullable(UInt32),
    device_id String,
    installation_id String,
    namespace_id String,
    action_kind LowCardinality(String),
    status LowCardinality(String),
    delivered Bool,
    error Nullable(String),
    started_at DateTime64(9, 'UTC'),
    finished_at Nullable(DateTime64(9, 'UTC')),
    dawn_instance LowCardinality(String),
    output_digest String,
    output_count UInt64,
    output_truncated Bool,
    reported Nullable(String),
    kafka_partition UInt32,
    kafka_offset UInt64,
    INDEX pid_index pid TYPE bloom_filter(0.01) GRANULARITY 4,
    INDEX campaign_id_index campaign_id TYPE bloom_filter(0.01) GRANULARITY 4
)
ENGINE = ReplicatedReplacingMergeTree
PARTITION BY toYYYYMM(time)
ORDER BY (device_id, installation_id, time, id)
TTL time + INTERVAL 30 DAY DELETE;
