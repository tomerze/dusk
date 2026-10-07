#!/bin/bash
set -euo pipefail

usage="usage: kubernetes.sh create|create-new|apply secret|configmap <directory of directories> | kubernetes.sh fetch secret|configmap <name> <directory>"
action=${1:?$usage}
kind=${2:?$usage}
api=${KUBERNETES_API:-https://kubernetes.default.svc}
account=${KUBERNETES_ACCOUNT:-/var/run/secrets/kubernetes.io/serviceaccount}
namespace=$(cat "$account/namespace")
response=$(mktemp)
document=$(mktemp)
value=$(mktemp)
authorization=$(mktemp)
trap 'rm -f "$response" "$document" "$value" "$authorization"' EXIT
printf 'Authorization: Bearer %s\n' "$(cat "$account/token")" > "$authorization"

case "$kind" in
    secret) collection=secrets ;;
    configmap) collection=configmaps ;;
    *) echo "$usage" >&2; exit 2 ;;
esac

request() {
    method=$1
    path=$2
    arguments=()
    if [ "$#" -ge 3 ]; then
        arguments=(--data-binary "@$3")
    fi
    curl --silent --show-error --max-time 30 --retry 6 --retry-connrefused \
        --cacert "$account/ca.crt" \
        --header "@$authorization" \
        --header "Content-Type: application/json" \
        --request "$method" --output "$response" --write-out '%{http_code}' \
        "${arguments[@]}" "$api/api/v1/namespaces/$namespace/$collection$path"
}

build() {
    name=$1
    directory=$2
    type=Opaque
    if [ "$kind" = secret ] && [ -f "$directory/tls.crt" ] && [ -f "$directory/tls.key" ]; then
        type=kubernetes.io/tls
    elif [ "$kind" = secret ] && [ -f "$directory/username" ] && [ -f "$directory/password" ]; then
        type=kubernetes.io/basic-auth
    fi
    jq -n --arg kind "$kind" --arg name "$name" --arg type "$type" '{
        apiVersion: "v1",
        kind: (if $kind == "secret" then "Secret" else "ConfigMap" end),
        metadata: {name: $name, labels: {"app.kubernetes.io/part-of": "dusk-stack", "app.kubernetes.io/managed-by": "dusk-bootstrap"}},
        data: {}
    } + (if $kind == "secret" then {type: $type} else {} end)' > "$document"
    for file in "$directory"/*; do
        [ -f "$file" ] || continue
        if [ "$kind" = secret ]; then
            base64 < "$file" | tr -d '\n' > "$value"
            jq --arg key "$(basename "$file")" --rawfile value "$value" '.data[$key] = $value' "$document" > "$document.next"
        else
            jq --arg key "$(basename "$file")" --rawfile value "$file" '.data[$key] = $value' "$document" > "$document.next"
        fi
        mv "$document.next" "$document"
    done
}

fail() {
    echo "$1 failed with HTTP $2: $(cat "$response")" >&2
    exit 1
}

publish() {
    name=$1
    directory=$2
    build "$name" "$directory"
    status=$(request POST "" "$document")
    case "$status" in
        201)
            echo "$kind $name created"
            return
            ;;
        409)
            if [ "$action" = create-new ]; then
                fail "creating $kind $name" "$status"
            fi
            ;;
        *) fail "creating $kind $name" "$status" ;;
    esac
    if [ "$action" = create ]; then
        echo "$kind $name exists, kept"
        return
    fi
    status=$(request GET "/$name")
    [ "$status" = 200 ] || fail "reading $kind $name" "$status"
    if [ "$(jq -S .data "$response")" = "$(jq -S .data "$document")" ]; then
        echo "$kind $name is current"
        return
    fi
    jq --arg version "$(jq -r .metadata.resourceVersion "$response")" '.metadata.resourceVersion = $version' "$document" > "$document.next"
    mv "$document.next" "$document"
    status=$(request PUT "/$name" "$document")
    [ "$status" = 200 ] || fail "replacing $kind $name" "$status"
    echo "$kind $name replaced"
}

case "$action" in
    create | create-new | apply)
        source=${3:?$usage}
        for directory in "$source"/*/; do
            [ -d "$directory" ] || continue
            publish "$(basename "$directory")" "${directory%/}"
        done
        ;;
    fetch)
        name=${3:?$usage}
        target=${4:?$usage}
        status=$(request GET "/$name")
        case "$status" in
            200) ;;
            404) exit 3 ;;
            *) fail "reading $kind $name" "$status" ;;
        esac
        mkdir -p "$target"
        for key in $(jq -r '.data // {} | keys[]' "$response"); do
            if [ "$kind" = secret ]; then
                jq -r --arg key "$key" '.data[$key]' "$response" | base64 -d > "$target/$key"
            else
                jq -j --arg key "$key" '.data[$key]' "$response" > "$target/$key"
            fi
        done
        ;;
    *)
        echo "$usage" >&2
        exit 2
        ;;
esac
