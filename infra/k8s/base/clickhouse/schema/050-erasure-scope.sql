CREATE TABLE IF NOT EXISTS dusk.erasure_scope ON CLUSTER dusk
(
    erasure_id UUID,
    kind LowCardinality(String),
    value String,
    created DateTime DEFAULT now()
)
ENGINE = ReplicatedReplacingMergeTree
ORDER BY (erasure_id, kind, value)
TTL created + INTERVAL 30 DAY DELETE;
