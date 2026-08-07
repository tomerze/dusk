#!/usr/bin/env bash
#
# Drop the build artifacts that are too large to keep in the GitHub Actions
# cache, which holds 10 GB for the whole repository.
#
# dusk_llm embeds the GGUF into the binary with `.incbin` (see
# dusk/src/dusk_llm/src/load.rs), so three artifacts carry the whole model:
# libdusk_llm's rlib, the object file it is built from, and every binary linked
# against it. Together they are over 10 GB on their own.
#
# What survives is everything under target/*/build: the Cap'n Proto compiler
# dusk_capnp builds from vendor/capnproto, the ik_llama.cpp archives dusk_llm
# builds from vendor/ik_llama.cpp, and the warm-up KV snapshot. Those cost
# minutes of C++ compilation and a full pass of the model over the system
# prompt to produce, and are together under 250 MB. Cargo re-runs a build
# script only when one of its declared inputs changes, so removing a crate's
# compiled output forces the crate to be compiled again but leaves its build
# script's output untouched.

set -euo pipefail

readonly MAXIMUM_CACHEABLE_BYTES=$((200 * 1024 * 1024))

if [[ ! -d target ]]; then
    echo "No target directory to prune."
    exit 0
fi

echo "Target directory before pruning: $(du -sh target | cut -f1)"

# Reported rather than deleted silently: a reader of the log can see exactly
# which artifacts the cache does not carry, and so which ones every restoring
# run has to build again.
mapfile -t oversized < <(
    find target -type f -size +"${MAXIMUM_CACHEABLE_BYTES}"c -not -path 'target/*/build/*'
)

if [[ ${#oversized[@]} -eq 0 ]]; then
    echo "Nothing exceeds ${MAXIMUM_CACHEABLE_BYTES} bytes."
else
    for path in "${oversized[@]}"; do
        echo "Dropping $(du -h "${path}" | cut -f1)	${path}"
        rm -f "${path}"
    done
fi

echo "Target directory after pruning: $(du -sh target | cut -f1)"
