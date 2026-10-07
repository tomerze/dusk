CREATE TABLE IF NOT EXISTS dusk.files ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    id UUID,
    schema LowCardinality(String),
    pid UInt64,
    campaign_id String,
    device_id String,
    installation_id String,
    namespace_id String,
    node_path String,
    bucket LowCardinality(String),
    object_key String,
    size_bytes UInt64,
    sha256 String,
    content_type LowCardinality(String),
    uploaded_at DateTime64(9, 'UTC'),
    dawn_instance LowCardinality(String),
    kafka_partition UInt32,
    kafka_offset UInt64,
    INDEX pid_index pid TYPE bloom_filter(0.01) GRANULARITY 4
)
ENGINE = ReplicatedReplacingMergeTree
PARTITION BY toYYYYMM(time)
ORDER BY (device_id, installation_id, time, id)
TTL time + INTERVAL 30 DAY DELETE;
