#!/bin/sh
set -eu

usage() {
    echo "usage: build.sh <output directory>" >&2
    echo "  CAPNP          the capnp compiler to run (default: capnp on PATH)" >&2
    echo "  DUSK_GIT_REV   the git revision to name the bundle after (default: git rev-parse HEAD)" >&2
    exit 2
}

[ "$#" -eq 1 ] || usage
output_directory=$1

script_directory=$(cd "$(dirname "$0")" && pwd)
repository=$(cd "$script_directory/../../.." && pwd)

capnp=${CAPNP:-capnp}
capnp_path=$(command -v "$capnp" || true)
if [ -z "$capnp_path" ]; then
    echo "build.sh: the capnp compiler '$capnp' was not found; set CAPNP to its path" >&2
    exit 1
fi
capnp_include=$(cd "$(dirname "$capnp_path")/.." && pwd)/include

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repository/dusk/src/dusk_capnp/Cargo.toml" | head -n 1)
if [ -z "$version" ]; then
    echo "build.sh: no version in dusk/src/dusk_capnp/Cargo.toml" >&2
    exit 1
fi

git_revision=${DUSK_GIT_REV:-}
if [ -z "$git_revision" ]; then
    git_revision=$(git -C "$repository" rev-parse HEAD 2>/dev/null || true)
fi
case "$git_revision" in
    "" | *[!0-9a-f]*)
        echo "build.sh: no git revision; set DUSK_GIT_REV to at least 16 lowercase hex characters" >&2
        exit 1
        ;;
esac
if [ "${#git_revision}" -lt 16 ]; then
    echo "build.sh: DUSK_GIT_REV '$git_revision' is shorter than 16 hex characters" >&2
    exit 1
fi
git_revision=$(printf '%s' "$git_revision" | cut -c 1-16)

mkdir -p "$output_directory"
bundle="$version-$git_revision"
staging=$(mktemp -d)
partial_bundle="$output_directory/.$bundle.capnp.bin.partial"
partial_manifest="$output_directory/.$bundle.json.partial"
cleanup() {
    rm -rf "$staging" "$partial_bundle" "$partial_manifest"
}
trap cleanup EXIT
mkdir "$staging/capnp"

for schema in \
    "$repository"/dusk/src/dusk_capnp/capnp/*.capnp \
    "$repository"/base/*/capnp/*.capnp \
    "$repository"/base/logs/capnp/otlp/*.capnp; do
    name=$(basename "$schema")
    if [ -e "$staging/capnp/$name" ]; then
        echo "build.sh: two schemas are named $name; every schema is imported as /capnp/$name, so the names must be unique" >&2
        exit 1
    fi
    cp "$schema" "$staging/capnp/$name"
done

"$capnp_path" compile -o- --no-standard-import \
    -I "$staging" -I "$capnp_include" \
    --src-prefix="$staging" \
    "$staging"/capnp/*.capnp >"$partial_bundle"

{
    printf '{"version":"%s","git_revision":"%s","programs":[' "$version" "$git_revision"
    separator=""
    for schema in "$repository"/base/*/capnp/*.capnp; do
        program_id=$(sed -n 's/^const programId :UInt64 = 0x\([0-9a-fA-F]*\);.*$/\1/p' "$schema" | head -n 1)
        [ -n "$program_id" ] || continue
        program_directory=$(dirname "$(dirname "$schema")")
        program_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$program_directory/Cargo.toml" | head -n 1)
        program_id=$(printf '%016s' "$program_id" | tr ' A-F' '0a-f')
        printf '%s{"program_id":"%s","name":"%s","version":"%s"}' \
            "$separator" "$program_id" "$(basename "$program_directory")" "$program_version"
        separator=","
    done
    printf ']}\n'
} >"$partial_manifest"

mv "$partial_bundle" "$output_directory/$bundle.capnp.bin"
mv "$partial_manifest" "$output_directory/$bundle.json"
echo "$output_directory/$bundle.capnp.bin"
