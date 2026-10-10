CREATE TABLE IF NOT EXISTS dusk.enrollments ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    id UUID,
    schema LowCardinality(String),
    operation LowCardinality(String),
    outcome LowCardinality(String),
    reason Nullable(String),
    device_id String,
    installation_id String,
    tenant LowCardinality(Nullable(String)),
    credential_kind LowCardinality(String),
    credential_ref LowCardinality(Nullable(String)),
    credential_issuer LowCardinality(Nullable(String)),
    hardware_fingerprint_hash Nullable(String),
    remote_address String,
    cert_serial Nullable(String),
    cert_fingerprint Nullable(String),
    cert_not_after Nullable(DateTime64(9, 'UTC')),
    dusk_version LowCardinality(Nullable(String)),
    impl LowCardinality(Nullable(String)),
    target_os LowCardinality(Nullable(String)),
    target_arch LowCardinality(Nullable(String)),
    hostname Nullable(String),
    instance LowCardinality(String),
    kafka_partition UInt32,
    kafka_offset UInt64
)
ENGINE = ReplicatedReplacingMergeTree
PARTITION BY toYYYYMM(time)
ORDER BY (device_id, installation_id, time, id)
TTL time + INTERVAL 30 DAY DELETE;
