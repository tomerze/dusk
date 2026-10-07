CREATE TABLE IF NOT EXISTS dusk.otel_logs ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    observed_time DateTime64(9, 'UTC'),
    service_name LowCardinality(String),
    severity_number UInt8,
    severity_text LowCardinality(String),
    body String,
    trace_id String,
    span_id String,
    flags UInt32,
    event_name LowCardinality(String),
    attributes Map(LowCardinality(String), String),
    resource_attributes Map(LowCardinality(String), String),
    scope_name LowCardinality(String),
    scope_version LowCardinality(String),
    scope_attributes Map(LowCardinality(String), String),
    kafka_partition UInt32,
    kafka_offset UInt64,
    INDEX trace_id_index trace_id TYPE bloom_filter(0.01) GRANULARITY 4
)
ENGINE = ReplicatedMergeTree
PARTITION BY toDate(time)
ORDER BY (service_name, time)
TTL time + INTERVAL 7 DAY DELETE;

CREATE TABLE IF NOT EXISTS dusk.otel_spans ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    end_time DateTime64(9, 'UTC'),
    duration_nanoseconds UInt64,
    service_name LowCardinality(String),
    name LowCardinality(String),
    kind LowCardinality(String),
    trace_id String,
    span_id String,
    parent_span_id String,
    trace_state String,
    flags UInt32,
    status_code LowCardinality(String),
    status_message String,
    attributes Map(LowCardinality(String), String),
    resource_attributes Map(LowCardinality(String), String),
    scope_name LowCardinality(String),
    scope_version LowCardinality(String),
    events String,
    links String,
    kafka_partition UInt32,
    kafka_offset UInt64,
    INDEX trace_id_index trace_id TYPE bloom_filter(0.01) GRANULARITY 4
)
ENGINE = ReplicatedMergeTree
PARTITION BY toDate(time)
ORDER BY (service_name, name, time)
TTL time + INTERVAL 7 DAY DELETE;

CREATE TABLE IF NOT EXISTS dusk.otel_metrics ON CLUSTER dusk
(
    time DateTime64(9, 'UTC'),
    start_time DateTime64(9, 'UTC'),
    service_name LowCardinality(String),
    metric_name LowCardinality(String),
    metric_description String,
    metric_unit LowCardinality(String),
    metric_type LowCardinality(String),
    aggregation_temporality LowCardinality(String),
    is_monotonic Bool,
    value Float64,
    count UInt64,
    sum Float64,
    min Nullable(Float64),
    max Nullable(Float64),
    bucket_counts Array(UInt64),
    explicit_bounds Array(Float64),
    exponential_scale Int32,
    exponential_zero_count UInt64,
    exponential_positive_offset Int32,
    exponential_positive_bucket_counts Array(UInt64),
    exponential_negative_offset Int32,
    exponential_negative_bucket_counts Array(UInt64),
    quantiles Array(Tuple(quantile Float64, value Float64)),
    flags UInt32,
    attributes Map(LowCardinality(String), String),
    resource_attributes Map(LowCardinality(String), String),
    scope_name LowCardinality(String),
    scope_version LowCardinality(String),
    kafka_partition UInt32,
    kafka_offset UInt64
)
ENGINE = ReplicatedMergeTree
PARTITION BY toDate(time)
ORDER BY (service_name, metric_name, time)
TTL time + INTERVAL 7 DAY DELETE;
