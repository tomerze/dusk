#!/bin/bash
set -euo pipefail

bootstrap=${KAFKA_BOOTSTRAP_SERVERS:-kafka:9092}
profile=${TOPIC_PROFILE:-dev}
replication_factor=${REPLICATION_FACTOR:-1}
min_insync_replicas=${MIN_INSYNC_REPLICAS:-1}
ledger_partitions=${LEDGER_PARTITIONS:-}
kafka_scripts=/opt/kafka/bin
client=(--bootstrap-server "$bootstrap")
if [ -n "${KAFKA_COMMAND_CONFIG:-}" ]; then
    client+=(--command-config "$KAFKA_COMMAND_CONFIG")
fi

day=86400000
topics=(
    "dusk.connections 3 48 cleanup.policy=delete,retention.ms=$((7 * day))"
    "dusk.census 1 6 cleanup.policy=compact,max.message.bytes=2097152,segment.ms=600000,min.cleanable.dirty.ratio=0.1,delete.retention.ms=3600000"
    "dusk.ledger 3 32 cleanup.policy=delete,retention.ms=$((30 * day)),unclean.leader.election.enable=false"
    "dusk.enrollments 3 12 cleanup.policy=delete,retention.ms=$((30 * day))"
    "dusk.node-state 1 6 cleanup.policy=compact"
    "dusk.credential-quota 1 6 cleanup.policy=compact"
    "dusk.process-results 3 48 cleanup.policy=delete,retention.ms=$((7 * day))"
    "dusk.process-output 3 24 cleanup.policy=delete,retention.ms=$((7 * day))"
    "dusk.files 3 12 cleanup.policy=delete,retention.ms=$((30 * day))"
    "dusk.intended-processes 3 12 cleanup.policy=compact,segment.ms=600000,min.cleanable.dirty.ratio=0.1,delete.retention.ms=3600000"
    "dusk.otel-logs 3 24 cleanup.policy=delete,retention.ms=$((3 * day)),max.message.bytes=4194304"
    "dusk.otel-spans 3 24 cleanup.policy=delete,retention.ms=$((3 * day)),max.message.bytes=4194304"
    "dusk.otel-metrics 3 24 cleanup.policy=delete,retention.ms=$((3 * day)),max.message.bytes=4194304"
)

attempt=0
until "$kafka_scripts/kafka-topics.sh" "${client[@]}" --list > /dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 30 ]; then
        echo "kafka at $bootstrap did not answer after $attempt attempts" >&2
        exit 1
    fi
    delay=$((RANDOM % (1 << (attempt < 5 ? attempt : 5)) + 1))
    echo "waiting ${delay}s for kafka at $bootstrap (attempt $attempt)"
    sleep "$delay"
done

existing=$("$kafka_scripts/kafka-topics.sh" "${client[@]}" --list)
failures=0
for entry in "${topics[@]}"; do
    read -r name dev_partitions prod_partitions configs <<< "$entry"
    if [ "$profile" = prod ]; then
        partitions=$prod_partitions
    else
        partitions=$dev_partitions
    fi
    if [ "$name" = dusk.ledger ] && [ -n "$ledger_partitions" ]; then
        partitions=$ledger_partitions
    fi
    configs="$configs,min.insync.replicas=$min_insync_replicas"
    if grep -qxF "$name" <<< "$existing"; then
        current=$("$kafka_scripts/kafka-topics.sh" "${client[@]}" --describe --topic "$name" | sed -n 's/.*PartitionCount: *\([0-9]*\).*/\1/p' | head -n 1)
        if [ "$current" -lt "$partitions" ]; then
            echo "topic $name has $current partitions, $partitions are required; add them by hand once every producer and consumer of $name allows it" >&2
            failures=$((failures + 1))
        elif [ "$current" -gt "$partitions" ]; then
            echo "topic $name has $current partitions, more than the $partitions this profile creates; kept"
        fi
        "$kafka_scripts/kafka-configs.sh" "${client[@]}" --alter --entity-type topics --entity-name "$name" --add-config "$configs" > /dev/null
        echo "topic $name exists, configuration applied: $configs"
    else
        config_arguments=()
        IFS=, read -r -a pairs <<< "$configs"
        for pair in "${pairs[@]}"; do
            config_arguments+=(--config "$pair")
        done
        "$kafka_scripts/kafka-topics.sh" "${client[@]}" --create --if-not-exists --topic "$name" \
            --partitions "$partitions" --replication-factor "$replication_factor" "${config_arguments[@]}" > /dev/null
        echo "topic $name created with $partitions partitions, replication factor $replication_factor: $configs"
    fi
done

if [ "$failures" -gt 0 ]; then
    exit 1
fi
