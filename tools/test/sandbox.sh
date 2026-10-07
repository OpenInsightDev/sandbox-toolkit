#!/bin/sh
# The sandbox the SDK integration tests drive: a container running `sbxtkt serve`
# whose port the test connects to.
#
# `build` produces that Linux binary — a Linux host compiles it, any other host
# cross-compiles it — and publishes it into a named volume; `start` runs the
# volume's binary, so the container the tests talk to mounts no host path.
# `logs` inspects a running one.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/../.." && pwd)
. "$here/container.sh"

# The one artifact that crosses into the sandbox.
volume=sbxtkt-bin

# The glibc floor the release builds to, so a cross-compiled binary runs on the
# oldest image the project publishes for.
glibc=2.31

case "${1:-}" in
build)
    cd "$repo"

    # The tests serve the artifact the release ships, not the dev profile: a
    # difference that only shows under optimization would otherwise pass here.
    #
    # Only the compilation differs by host; a Linux host needs no cross linker,
    # so it builds natively.
    case "$(uname -s)" in
    Linux)
        cargo build --release --manifest-path crates/sandbox-toolkit/Cargo.toml
        built=target/release/sbxtkt
        ;;
    *)
        case "$(uname -m)" in
        arm64 | aarch64) triple=aarch64-unknown-linux-gnu ;;
        *) triple=x86_64-unknown-linux-gnu ;;
        esac

        cargo zigbuild --release --manifest-path crates/sandbox-toolkit/Cargo.toml \
            --target "$triple.$glibc"
        built="target/$triple/release/sbxtkt"
        ;;
    esac

    # The artifact the task cache tracks: a hit replays this file and skips the
    # compilation entirely.
    mkdir -p target/linux
    cp "$built" target/linux/sbxtkt

    # The volume starts root-owned: a root step copies the binary in, which is
    # also the uid the run container reads it as.
    "$SBXTKT_RUNTIME" volume create "$volume" >/dev/null 2>&1 || true
    "$SBXTKT_RUNTIME" run --rm \
        -v "$repo/target/linux:/in:ro" \
        -v "$volume:/out" \
        "$SBXTKT_IMAGE" \
        cp /in/sbxtkt /out/sbxtkt
    ;;
start)
    port=${2:?port}
    name=${3:?name}

    "$SBXTKT_RUNTIME" volume create "$volume" >/dev/null 2>&1 || true
    "$SBXTKT_RUNTIME" run --detach \
        --name "$name" \
        --publish "127.0.0.1:$port:$port" \
        --volume "$volume:/opt/sbxtkt:ro" \
        --env HOME=/tmp/home \
        --env RUST_LOG=warn \
        "$SBXTKT_IMAGE" \
        sh -c "mkdir -p /tmp/home /workspace && exec /opt/sbxtkt/sbxtkt serve --host 0.0.0.0 --port $port"
    ;;
stop)
    name=${2:?name}

    "$SBXTKT_RUNTIME" rm --force "$name" >/dev/null 2>&1 || true
    ;;
logs)
    "$SBXTKT_RUNTIME" logs "${2:?name}" 2>&1 || true
    ;;
*)
    echo "usage: sandbox.sh <build|start <port> <name>|stop <name>|logs <name>>" >&2
    exit 2
    ;;
esac
