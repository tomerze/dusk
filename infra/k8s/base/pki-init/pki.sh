#!/bin/sh
set -eu
umask 077

usage="usage: pki.sh init <pki directory> <step-ca directory> <secrets directory> | pki.sh renew <pki directory> | pki.sh kubernetes <work directory> <step-ca directory>"
mode=${1:?$usage}
pki=${2:?$usage}

domain=${DUSK_DOMAIN:-dusk.test}
namespace=${DUSK_NAMESPACE:-}
leaf_lifetime=${LEAF_LIFETIME:-720h}
renew_before=${RENEW_BEFORE:-240h}
ca_lifetime=${CA_LIFETIME:-87600h}
renew_interval=${RENEW_INTERVAL_SECONDS:-3600}
step_ca_dns_names=${STEP_CA_DNS_NAMES:-step-ca,localhost}
certificate_lifetime=${NODE_CERTIFICATE_LIFETIME:-168h}
offline_ca_url=https://127.0.0.1:9
layout=directories
leaf_root=$pki

templates=$(mktemp -d)
trap 'rm -rf "$templates"' EXIT

write_template() {
    cat > "$templates/$1.tpl" <<EOF
{
  "subject": {{ toJson .Subject }},
  "sans": {{ toJson .SANs }},
  "keyUsage": ["digitalSignature"],
  "extKeyUsage": $2
}
EOF
}
write_template server '["serverAuth"]'
write_template client '["clientAuth"]'
write_template both '["serverAuth", "clientAuth"]'

service_names() {
    printf -- '--san %s ' "$1"
    if [ -n "$namespace" ]; then
        printf -- '--san %s.%s.svc ' "$1" "$namespace"
        printf -- '--san %s.%s.svc.cluster.local ' "$1" "$namespace"
    fi
}

issue() {
    directory=$1
    name=$2
    issuer=$3
    template=$4
    common_name=$5
    shift 5
    step certificate create "$common_name" "$directory/.$name.crt.new" "$directory/.$name.key.new" \
        --ca "$pki/pki-ca/$issuer.crt" --ca-key "$pki/pki-ca/$issuer.key" \
        --template "$templates/$template.tpl" --not-after "$leaf_lifetime" \
        --kty EC --crv P-256 --no-password --insecure --force "$@" > /dev/null
    chmod 0444 "$directory/.$name.crt.new" "$directory/.$name.key.new"
    if [ "$layout" = kubernetes ]; then
        mv "$directory/.$name.key.new" "$directory/$name.key"
        mv "$directory/.$name.crt.new" "$directory/$name.crt"
        cp "$pki/pki-ca/$issuer.crt" "$directory/ca.crt"
    else
        swap_in "$directory" "$name"
    fi
    echo "issued $directory/$name.crt for $common_name"
}

swap_in() {
    generation=$(mktemp -d "$1/..generation.XXXXXX")
    if [ -d "$1/..data" ]; then
        cp -p "$1/..data/"* "$generation/"
    fi
    mv "$1/.$2.key.new" "$generation/$2.key"
    mv "$1/.$2.crt.new" "$generation/$2.crt"
    chmod 0555 "$generation"
    ln -s "${generation##*/}" "$1/..data.new"
    mv -T "$1/..data.new" "$1/..data"
    for extension in crt key; do
        if [ ! -L "$1/$2.$extension" ]; then
            ln -sf "..data/$2.$extension" "$1/$2.$extension"
        fi
    done
    find "$1" -mindepth 1 -maxdepth 1 -name '..generation.*' ! -name "${generation##*/}" -exec rm -rf {} +
}

leaves() {
    action=$1
    $action nightfall-tls fleet fleet-server server "fleet.$domain" --san "fleet.$domain"
    $action nightfall-tls provision fleet-server server "provision.$domain" --san "provision.$domain"
    $action nightfall-tls inner internal server "nightfall-inner" --san "*.fleet.$domain"
    $action nightfall-tls admin internal server "nightfall" $(service_names nightfall) --san nightfall-admin
    $action dawn-tls tls internal both "dawn-0" $(service_names dawn) --san dawn-0 --san "urn:dusk:principal:dawn-0"
    $action twilight-tls tls internal both "twilight-0" $(service_names twilight) --san "urn:dusk:principal:twilight-0"
    $action admin-tls tls internal client "admin-0" --san "urn:dusk:principal:admin-0"
}

place() {
    target_group=$1
    target_name=$2
    if [ "$layout" = kubernetes ] && [ "$target_name" != tls ]; then
        target_group="${1%-tls}-$2-tls"
        target_name=tls
    fi
}

issue_into_group() {
    place "$1" "$2"
    shift 2
    mkdir -p "$leaf_root/$target_group"
    issue "$leaf_root/$target_group" "$target_name" "$@"
}

renew_if_due() {
    place "$1" "$2"
    shift 2
    certificate="$leaf_root/$target_group/$target_name.crt"
    set +e
    step certificate needs-renewal "$certificate" --expires-in "$renew_before" > /dev/null
    status=$?
    set -e
    case "$status" in
        0) issue "$leaf_root/$target_group" "$target_name" "$@" ;;
        1) ;;
        2)
            echo "$certificate is missing, issuing it" >&2
            mkdir -p "$leaf_root/$target_group"
            issue "$leaf_root/$target_group" "$target_name" "$@"
            ;;
        *)
            echo "checking $certificate for renewal failed with status $status" >&2
            renewal_failures=$((renewal_failures + 1))
            ;;
    esac
}

renew_all() {
    renewal_failures=0
    leaves "$1"
    if [ "$renewal_failures" -gt 0 ]; then
        echo "certificates that could not be checked for renewal: $renewal_failures" >&2
        exit 1
    fi
}

ensure_published() {
    place "$1" "$2"
    if ! fetch secret "$target_group" "$leaf_root/$target_group"; then
        issue_into_group "$@"
        return
    fi
    renew_if_due "$@"
}

fetch() {
    set +e
    "$helper" fetch "$@"
    status=$?
    set -e
    case "$status" in
        0) return 0 ;;
        3) return 1 ;;
        *) exit "$status" ;;
    esac
}

require() {
    if ! fetch secret "$1" "$secrets/$1"; then
        echo "the secret $1 is missing: secrets-init creates it" >&2
        exit 1
    fi
}

publish_step_ca() {
    steppath=$1
    if fetch secret step-ca "$pki/step-ca"; then
        echo "step-ca configuration exists, kept"
        return
    fi
    require fleet-provisioner
    require step-ca-password
    init_step_ca "$steppath" "$secrets"
    mkdir -p "$pki/new/step-ca" "$pki/root/step-ca-root-key"
    jq '.root = "/etc/step-ca/certs/root_ca.crt"
        | .crt = "/etc/step-ca/certs/intermediate_ca.crt"
        | .key = "/etc/step-ca/secrets/intermediate_ca_key"
        | .db.dataSource = "/var/lib/step-ca/db"' \
        "$steppath/config/ca.json" > "$pki/new/step-ca/ca.json"
    cp "$steppath/certs/root_ca.crt" "$steppath/certs/intermediate_ca.crt" "$steppath/secrets/intermediate_ca_key" "$pki/new/step-ca/"
    cp "$steppath/secrets/root_ca_key" "$pki/root/step-ca-root-key/"
    if ! "$helper" create-new secret "$pki/root"; then
        echo "the root key of the new step-ca was not stored; if the secret step-ca-root-key exists, an earlier run stopped before it created step-ca: delete step-ca-root-key and run pki-init again" >&2
        exit 1
    fi
    "$helper" create secret "$pki/new"
    rm -rf "$pki/new" "$pki/root"
    fetch secret step-ca "$pki/step-ca"
}

publish_roots() {
    if fetch secret pki-ca "$pki/pki-ca"; then
        return
    fi
    mkdir -p "$pki/pki-ca"
    create_root fleet-server "Dusk fleet-server root"
    create_root internal "Dusk internal root"
    mkdir -p "$pki/new"
    mv "$pki/pki-ca" "$pki/new/pki-ca"
    "$helper" create secret "$pki/new"
    rm -rf "$pki/new"
    fetch secret pki-ca "$pki/pki-ca"
}

create_root() {
    step certificate create "$2" "$pki/pki-ca/$1.crt" "$pki/pki-ca/$1.key" \
        --profile root-ca --not-after "$ca_lifetime" --kty EC --crv P-256 --no-password --insecure > /dev/null
    echo "created root $2"
}

init_step_ca() {
    steppath=$1
    secrets=$2
    if [ -e "$steppath/config/ca.json" ]; then
        echo "step-ca configuration exists, kept"
        return
    fi
    export STEPPATH="$steppath"
    bootstrap_password=$(mktemp)
    step crypto rand --format alphanumeric 40 > "$bootstrap_password"
    step ca init --deployment-type standalone --name "Dusk fleet-client CA" \
        --dns "$step_ca_dns_names" --address ":9000" --provisioner bootstrap \
        --password-file "$secrets/step-ca-password/password" \
        --provisioner-password-file "$bootstrap_password" > /dev/null
    rm -f "$bootstrap_password"
    step ca provisioner remove bootstrap --ca-config "$steppath/config/ca.json" --ca-url "$offline_ca_url"
    step ca provisioner add nightfall --type JWK \
        --public-key "$secrets/fleet-provisioner/provisioner.pub.jwk" \
        --x509-template "$(dirname "$0")/node.tpl" \
        --x509-min-dur 5m --x509-max-dur "$certificate_lifetime" --x509-default-dur "$certificate_lifetime" \
        --disable-renewal --ca-config "$steppath/config/ca.json" --ca-url "$offline_ca_url"
    jq '.logger = {"format": "json"} | (.authority.provisioners[] | select(.name == "nightfall") | .claims.enableSSHCA) = false' \
        "$steppath/config/ca.json" > "$steppath/config/ca.json.new"
    mv "$steppath/config/ca.json.new" "$steppath/config/ca.json"
    if [ "$(id -u)" = 0 ]; then
        chown -R 1000:1000 "$steppath"
    fi
    echo "step-ca configured with the nightfall provisioner"
}

case "$mode" in
    init)
        steppath=${3:?$usage}
        secrets=${4:?$usage}
        mkdir -p "$pki"
        init_step_ca "$steppath" "$secrets"
        if [ ! -e "$pki/pki-ca/internal.crt" ]; then
            mkdir -p "$pki/pki-ca"
            create_root fleet-server "Dusk fleet-server root"
            create_root internal "Dusk internal root"
        fi
        mkdir -p "$pki/trust-anchors"
        cp "$pki/pki-ca/fleet-server.crt" "$pki/trust-anchors/fleet-server-ca.crt"
        cp "$pki/pki-ca/internal.crt" "$pki/trust-anchors/internal-ca.crt"
        cp "$steppath/certs/root_ca.crt" "$pki/trust-anchors/fleet-client-ca.crt"
        chmod 0444 "$pki/trust-anchors/"*
        if [ ! -e "$pki/admin-tls/tls.crt" ]; then
            leaves issue_into_group
        else
            echo "leaf certificates exist, kept"
        fi
        find "$pki" -mindepth 1 -maxdepth 1 -type d ! -name pki-ca -exec chmod 0555 {} +
        chmod 0700 "$pki/pki-ca"
        ;;
    renew)
        while true; do
            renew_all renew_if_due
            sleep "$renew_interval"
        done
        ;;
    kubernetes)
        steppath=${3:?$usage}
        helper=${KUBERNETES_HELPER:?KUBERNETES_HELPER names kubernetes.sh}
        scope=${PKI_SCOPE:-all}
        layout=kubernetes
        leaf_root=$pki/leaves
        secrets=$pki/secrets
        mkdir -p "$pki" "$leaf_root" "$secrets" "$pki/publish/dusk-trust-anchors"
        publish_step_ca "$steppath"
        anchors=$pki/publish/dusk-trust-anchors
        cp "$pki/step-ca/root_ca.crt" "$anchors/fleet-client-ca.crt"
        case "$scope" in
            all)
                publish_roots
                renew_all ensure_published
                "$helper" apply secret "$leaf_root"
                cp "$pki/pki-ca/fleet-server.crt" "$anchors/fleet-server-ca.crt"
                cp "$pki/pki-ca/internal.crt" "$anchors/internal-ca.crt"
                ;;
            step-ca)
                for issuer in fleet-server internal; do
                    if ! fetch secret "$issuer-ca" "$pki/issuers/$issuer"; then
                        echo "the secret $issuer-ca is missing: cert-manager issues it from the $issuer-ca Certificate" >&2
                        exit 1
                    fi
                    cp "$pki/issuers/$issuer/tls.crt" "$anchors/$issuer-ca.crt"
                done
                ;;
            *)
                echo "PKI_SCOPE is all or step-ca, not $scope" >&2
                exit 2
                ;;
        esac
        "$helper" apply configmap "$pki/publish"
        ;;
    *)
        echo "$usage" >&2
        exit 2
        ;;
esac
