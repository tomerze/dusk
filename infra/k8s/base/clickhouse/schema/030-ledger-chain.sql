CREATE OR REPLACE VIEW dusk.ledger_chain ON CLUSTER dusk AS
WITH
    entries AS
    (
        SELECT DISTINCT instance, partition, sequence, hash, previous_hash, kind, time
        FROM dusk.ledger
        WHERE time >= {since:DateTime64(9, 'UTC')}
    ),
    linked AS
    (
        SELECT
            instance,
            partition,
            sequence,
            hash,
            previous_hash,
            kind,
            time,
            row_number() OVER chain AS position,
            lagInFrame(sequence, 1) OVER chain AS prior_sequence,
            lagInFrame(hash, 1) OVER chain AS prior_hash
        FROM entries
        WINDOW chain AS (PARTITION BY instance, partition ORDER BY sequence, hash ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)
    )
SELECT
    instance,
    partition,
    count() AS entries,
    min(sequence) AS first_sequence,
    max(sequence) AS last_sequence,
    countIf(position > 1 AND sequence = prior_sequence) AS conflicting_entries,
    countIf(position > 1 AND sequence > prior_sequence + 1) AS gaps,
    countIf(position > 1 AND sequence = prior_sequence + 1 AND previous_hash != prior_hash) AS broken_links,
    maxIf(sequence, kind = 'checkpoint') AS last_checkpoint_sequence,
    maxIf(time, kind = 'checkpoint') AS last_checkpoint_time,
    max(time) AS last_entry_time,
    conflicting_entries = 0 AND gaps = 0 AND broken_links = 0 AS intact
FROM linked
GROUP BY instance, partition;
