#!/bin/bash
set -euo pipefail

secrets=${DUSK_SECRETS_DIRECTORY:-/run/secrets}
users=${CEPH_USERS:-admin vector ledger-writer dawn reader signoz}

attempt=0
until radosgw-admin user list > /dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 60 ]; then
        echo "the ceph cluster did not answer radosgw-admin after $attempt attempts" >&2
        exit 1
    fi
    delay=$((RANDOM % (1 << (attempt < 5 ? attempt : 5)) + 1))
    echo "waiting ${delay}s for the ceph cluster (attempt $attempt)"
    sleep "$delay"
done

for user in $users; do
    access_key=$(cat "$secrets/ceph-$user/access-key")
    secret_key=$(cat "$secrets/ceph-$user/secret-key")
    if radosgw-admin user info --uid "$user" > /dev/null 2>&1; then
        radosgw-admin key create --uid "$user" --key-type s3 --access-key "$access_key" --secret-key "$secret_key" > /dev/null
        echo "ceph user $user exists, its key is in place"
    else
        radosgw-admin user create --uid "$user" --display-name "dusk $user" --access-key "$access_key" --secret-key "$secret_key" > /dev/null
        echo "ceph user $user created"
    fi
done
