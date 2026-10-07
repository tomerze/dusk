#!/bin/sh
set -eu
umask 077

output=${1:?usage: generate.sh <output directory>}
mkdir -p "$output"

hex_key() {
    printf '%s' "$(step crypto rand --format hex 64)"
}

password() {
    printf '%s' "$(step crypto rand --format alphanumeric 40)"
}

device_id_key() {
    hex_key > "$1/device-id.key"
}

fleet_token() {
    token=$(step crypto rand --format alphanumeric 48)
    printf '%s' "$token" > "$1/fleet-token"
    digest=$(printf '%s' "$token" | sha256sum | cut -d ' ' -f 1)
    {
        printf '[[token]]\n'
        printf 'name = "%s"\n' "${FLEET_TOKEN_NAME:-default}"
        printf 'value_sha256 = "%s"\n' "$digest"
        if [ -n "${FLEET_TOKEN_TENANT:-}" ]; then
            printf 'tenant = "%s"\n' "$FLEET_TOKEN_TENANT"
        fi
    } > "$1/fleet-tokens.toml"
}

install_token_keys() {
    printf '{"keys":[]}\n' > "$1/install-token-jwks.json"
}

provisioner() {
    step crypto jwk create "$1/provisioner.pub.jwk" "$1/provisioner.jwk" \
        --kty EC --crv P-256 --use sig --alg ES256 --no-password --insecure
}

step_ca_password() {
    password > "$1/password"
}

ledger_keys() {
    step crypto keypair "$1/ledger-signing.pub" "$1/ledger-signing.key" \
        --kty OKP --crv Ed25519 --no-password --insecure
    step crypto jwk create "$1/ledger-signing.pub.jwk" "$1/.ledger-signing.jwk" \
        --from-pem "$1/ledger-signing.key" --use sig --no-password --insecure
    rm -f "$1/.ledger-signing.jwk"
    step crypto jwk keyset add "$1/ledger-verify-jwks.json" < "$1/ledger-signing.pub.jwk"
    hex_key > "$1/ledger-param.key"
}

ledger_verify_keys() {
    cp "$output/nightfall-ledger/ledger-verify-jwks.json" "$1/"
}

dawn_output_key() {
    hex_key > "$1/output.key"
}

ceph_user() {
    printf '%s' "$(step crypto rand --format upper 20)" > "$1/access-key"
    password > "$1/secret-key"
}

single_password() {
    password > "$1/password"
}

postgres_login() {
    printf '%s' "$1" > "$2/username"
    password > "$2/password"
    printf '*:*:*:%s:%s\n' "$1" "$(cat "$2/password")" > "$2/pgpass"
}

create() {
    group=$1
    shift
    if [ -e "$output/$group" ]; then
        echo "secret $group exists, kept"
        return
    fi
    partial="$output/.$group.partial"
    rm -rf "$partial"
    mkdir "$partial"
    "$@" "$partial"
    find "$partial" -type f -exec chmod 0444 {} +
    chmod 0555 "$partial"
    mv "$partial" "$output/$group"
    echo "secret $group created"
}

create fleet-device-id device_id_key
create fleet-token fleet_token
create fleet-install-token-keys install_token_keys
create fleet-provisioner provisioner
create step-ca-password step_ca_password
create nightfall-ledger ledger_keys
create nightfall-ledger-verify ledger_verify_keys
create dawn-output-key dawn_output_key
for user in admin vector ledger-writer dawn reader signoz; do
    create "ceph-$user" ceph_user
done
create postgres-superuser postgres_login postgres
for role in twilight grafana signoz; do
    create "postgres-$role" postgres_login "$role"
done
for user in admin vector grafana signoz; do
    create "clickhouse-$user" single_password
done
create grafana-admin single_password
