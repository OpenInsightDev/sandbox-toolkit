#!/bin/sh
# The container runtime, image and cache every container-backed test script runs
# against. Kept in one place so `e2e.sh` and `sandbox.sh` cannot drift on which
# image a binary has to run in, or on where the shared cargo cache lives.
#
# Sourced by those scripts; each resolves them from the same environment
# overrides and fails the same way on a runtime it does not know.

case "$(uname -s)" in
Darwin) SBXTKT_RUNTIME=${SBXTKT_TEST_RUNTIME:-container} ;;
*) SBXTKT_RUNTIME=${SBXTKT_TEST_RUNTIME:-docker} ;;
esac

SBXTKT_IMAGE=${SBXTKT_TEST_IMAGE:-rust:1.97-bookworm}
SBXTKT_CACHE=${SBXTKT_TEST_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/sandbox-toolkit}

case "$SBXTKT_RUNTIME" in
docker | container) ;;
*)
    echo "unknown container runtime: $SBXTKT_RUNTIME" >&2
    exit 2
    ;;
esac
