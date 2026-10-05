#!/bin/sh
# Runs the end-to-end suite in a Linux container: CI has docker, macOS has
# `container`.
#
# The container always sees the repository at /w, so the paths it compiles are
# the same whatever checkout is mounted: one cache directory outside the
# worktrees is reused by all of them instead of one copy per checkout.
# Arguments are passed on to `cargo test`.
set -eu

case "$(uname -s)" in
Darwin) default_runtime=container ;;
*) default_runtime=docker ;;
esac

runtime=${SBXTKT_TEST_RUNTIME:-$default_runtime}
image=${SBXTKT_TEST_IMAGE:-rust:1.97-bookworm}
cache=${SBXTKT_TEST_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/sandbox-toolkit}

case "$runtime" in
docker) flags='' ;;
# Apple's defaults, five CPUs and one GiB, cannot build this workspace.
container) flags='--progress none --cpus 8 --memory 8g' ;;
*)
    echo "unknown container runtime: $runtime" >&2
    exit 2
    ;;
esac

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
tests="$repo/crates/sandbox-toolkit/tests"

targets=''
for file in "$tests"/*.rs; do
    targets="$targets --test $(basename "$file" .rs)"
done

# `cargo test` runs in the container, so the server and everything it spawns are
# Linux processes. Its home is writable whatever user it runs as.
command="mkdir -p \"\$HOME\" && exec cargo test -p sandbox-toolkit$targets \"\$@\""

mkdir -p "$cache/cargo-home" "$cache/target"

# shellcheck disable=SC2086
"$runtime" run --rm $flags \
    -v "$repo:/w" \
    -v "$cache:/cache" \
    -w /w \
    -e HOME=/tmp/home \
    -e CARGO_HOME=/cache/cargo-home \
    -e CARGO_TARGET_DIR=/cache/target \
    -u "$(id -u):$(id -g)" \
    "$image" \
    sh -c "$command" e2e "$@"
