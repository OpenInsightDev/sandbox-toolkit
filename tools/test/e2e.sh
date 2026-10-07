#!/bin/sh
# Runs the end-to-end suite in a Linux container: CI has docker, macOS has
# `container`.
#
# The container always sees the repository at /w, so the paths it compiles are
# the same whatever checkout is mounted: one cache directory outside the
# worktrees is reused by all of them instead of one copy per checkout.
# Arguments are passed on to `cargo test`.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$here/container.sh"

# Apple's default container (5 CPUs, 1 GiB) is OOM-killed while linking this
# workspace. The sandbox the SDK tests start only runs the server, so it needs
# no such raise and does not share this one.
case "$SBXTKT_RUNTIME" in
docker) flags='' ;;
container) flags='--progress none --cpus 8 --memory 8g' ;;
esac

repo=$(CDPATH= cd -- "$here/../.." && pwd)

# `cargo test` runs in the container, so the server and everything it spawns are
# Linux processes. Its home is writable whatever user it runs as.
#
# `--test '*'` is the integration targets and nothing else: the unit tests and
# the binding generators live in `src`, and `rust:test` and `rust:bindings` own
# those.
command="mkdir -p \"\$HOME\" && exec cargo test -p sandbox-toolkit --test '*' \"\$@\""

mkdir -p "$SBXTKT_CACHE/cargo-home" "$SBXTKT_CACHE/target"

# shellcheck disable=SC2086
"$SBXTKT_RUNTIME" run --rm $flags \
    -v "$repo:/w" \
    -v "$SBXTKT_CACHE:/cache" \
    -w /w \
    -e HOME=/tmp/home \
    -e CARGO_HOME=/cache/cargo-home \
    -e CARGO_TARGET_DIR=/cache/target \
    -u "$(id -u):$(id -g)" \
    "$SBXTKT_IMAGE" \
    sh -c "$command" e2e "$@"
