#!/usr/bin/env bash
# Run the repeatable local verification subset for the Dusk Hyperlane branch.
#
# This is intentionally local-runner oriented: the workspace has private Rusk
# path dependencies, so GitHub-hosted CI needs an explicit checkout/credential
# design before it can run the same commands.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEFAULT_RUSK_DIR="$(cd "$ROOT/../.." && pwd)/rusk-private"
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
RUN_AGENT_CHECK=0

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

usage() {
    cat <<EOF
Usage: bash scripts/local-repro-check.sh [options]

Runs the local reproducibility checks that do not require a live E2E network:
  - make all
  - cargo test -p hyperlane-dusk-types
  - cargo test -p hyperlane-dusk-integration-tests
  - cargo test -p dusk-tx
  - make secret-hygiene

Options:
  --agent-check        Also run the Hyperlane Rust agent cargo check.
  --monorepo-dir DIR   Hyperlane monorepo checkout for --agent-check.
                       Default: $MONOREPO_DIR
  -h, --help           Show this help.

Prerequisite:
  The Dusk Cargo workspace currently expects private Rusk path dependencies at:
    $DEFAULT_RUSK_DIR

This script does not replace E2E/fault-injection runs in TEST_REPORT.md.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --agent-check)
            RUN_AGENT_CHECK=1
            shift
            ;;
        --monorepo-dir)
            MONOREPO_DIR="${2:-}"
            [ -n "$MONOREPO_DIR" ] || fail "--monorepo-dir requires a path"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

cd "$ROOT"

command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v make >/dev/null 2>&1 || fail "make is required"
command -v git >/dev/null 2>&1 || fail "git is required"

[ -d "$DEFAULT_RUSK_DIR/core" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/core"
[ -d "$DEFAULT_RUSK_DIR/vm" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/vm"
[ -d "$DEFAULT_RUSK_DIR/rusk-prover" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/rusk-prover"
[ -d "$DEFAULT_RUSK_DIR/data-drivers/data-driver" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/data-drivers/data-driver"

info "Using Rusk path dependencies from $DEFAULT_RUSK_DIR"

info "Building Dusk contract WASMs"
make all

info "Running Dusk type tests"
cargo test -p hyperlane-dusk-types

info "Running Dusk VM integration tests"
cargo test -p hyperlane-dusk-integration-tests

info "Running dusk-tx tests"
cargo test -p dusk-tx

info "Running secret hygiene checks"
make secret-hygiene

if [ "$RUN_AGENT_CHECK" -eq 1 ]; then
    [ -d "$MONOREPO_DIR/rust/main" ] || fail "missing Hyperlane monorepo rust/main: $MONOREPO_DIR"
    info "Running Hyperlane Rust agent check from $MONOREPO_DIR/rust/main"
    (
        cd "$MONOREPO_DIR/rust/main"
        cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
    )
fi

info "Local repro checks passed"
