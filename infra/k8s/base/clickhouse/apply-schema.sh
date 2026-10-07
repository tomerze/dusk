#!/bin/bash
set -euo pipefail
shopt -s nullglob

host=${CLICKHOUSE_HOST:-clickhouse}
user=${CLICKHOUSE_USER:-default}
CLICKHOUSE_PASSWORD=$(cat "${CLICKHOUSE_PASSWORD_FILE:?CLICKHOUSE_PASSWORD_FILE names the admin password file}")
export CLICKHOUSE_PASSWORD
schema=${SCHEMA_DIRECTORY:-/schema}
client=(clickhouse-client --host "$host" --user "$user")
if [ -n "${CLICKHOUSE_CLIENT_CONFIG:-}" ]; then
    client+=(--config-file "$CLICKHOUSE_CLIENT_CONFIG")
fi

attempt=0
until "${client[@]}" --query "SELECT 1" > /dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 30 ]; then
        echo "clickhouse at $host did not answer after $attempt attempts" >&2
        exit 1
    fi
    delay=$((RANDOM % (1 << (attempt < 5 ? attempt : 5)) + 1))
    echo "waiting ${delay}s for clickhouse at $host (attempt $attempt)"
    sleep "$delay"
done

files=("$schema"/*.sql)
if [ "${#files[@]}" -eq 0 ]; then
    echo "no schema files in $schema" >&2
    exit 1
fi
for file in "${files[@]}"; do
    "${client[@]}" --multiquery --queries-file "$file"
    echo "applied $(basename "$file")"
done
