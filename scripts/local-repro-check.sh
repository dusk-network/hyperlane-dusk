#!/usr/bin/env bash
# Run the repeatable local verification subset for the Dusk Hyperlane branch.
#
# This is intentionally local-runner oriented: the workspace has private Rusk
# path dependencies, so GitHub-hosted CI needs an explicit checkout/credential
# design before it can run the same commands.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEFAULT_RUSK_DIR="$(cd "$ROOT/../.." && pwd)/rusk-private"
RUSK_DIR="${RUSK_DIR:-$DEFAULT_RUSK_DIR}"
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
  - make clippy-contracts
  - cargo test -p hyperlane-dusk-types
  - cargo test -p hyperlane-dusk-integration-tests
  - cargo test -p hyperlane-dusk-data-driver
  - cargo test -p dusk-tx
  - make data-driver
  - make secret-hygiene

Options:
  --agent-check        Also run the Hyperlane Rust agent cargo check.
  --rusk-dir DIR       Rusk checkout to use for private path dependencies.
                       Default: $RUSK_DIR
  --monorepo-dir DIR   Hyperlane monorepo checkout for --agent-check.
                       Default: $MONOREPO_DIR
  -h, --help           Show this help.

Prerequisite:
  The Dusk Cargo workspace expects private Rusk path dependencies at:
    $DEFAULT_RUSK_DIR
  If --rusk-dir or RUSK_DIR points elsewhere, this script creates a temporary
  compatible checkout layout and reruns itself from there.

This script does not replace E2E/fault-injection runs in TEST_REPORT.md.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --agent-check)
            RUN_AGENT_CHECK=1
            shift
            ;;
        --rusk-dir)
            RUSK_DIR="${2:-}"
            [ -n "$RUSK_DIR" ] || fail "--rusk-dir requires a path"
            shift 2
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

[ -d "$RUSK_DIR" ] || fail "missing Rusk checkout: $RUSK_DIR"
RUSK_DIR="$(cd "$RUSK_DIR" && pwd -P)"
DEFAULT_RUSK_DIR="$(cd "$(dirname "$DEFAULT_RUSK_DIR")" && pwd -P)/$(basename "$DEFAULT_RUSK_DIR")"

if [ "$RUN_AGENT_CHECK" -eq 1 ]; then
    [ -d "$MONOREPO_DIR" ] || fail "missing Hyperlane monorepo checkout: $MONOREPO_DIR"
    MONOREPO_DIR="$(cd "$MONOREPO_DIR" && pwd -P)"
fi

if [ "$RUSK_DIR" != "$DEFAULT_RUSK_DIR" ] && [ "${HYPERLANE_DUSK_REPRO_LAYOUT:-0}" != "1" ]; then
    [ -d "$RUSK_DIR/core" ] || fail "missing Rusk path dependency: $RUSK_DIR/core"
    [ -d "$RUSK_DIR/vm" ] || fail "missing Rusk path dependency: $RUSK_DIR/vm"
    [ -d "$RUSK_DIR/rusk-prover" ] || fail "missing Rusk path dependency: $RUSK_DIR/rusk-prover"
    [ -d "$RUSK_DIR/data-drivers/data-driver" ] || fail "missing Rusk path dependency: $RUSK_DIR/data-drivers/data-driver"

    dusk_status="$(git -C "$ROOT" status --porcelain --untracked-files=all)"
    [ -z "$dusk_status" ] || fail \
        "custom-Rusk repro requires a clean Dusk checkout; commit/stash changes or use the default compatible layout"
    if [ "$RUN_AGENT_CHECK" -eq 1 ]; then
        monorepo_status="$(git -C "$MONOREPO_DIR" status --porcelain --untracked-files=all)"
        [ -z "$monorepo_status" ] || fail \
            "custom-Rusk agent repro requires a clean Hyperlane monorepo checkout"
    fi

    if [ -n "${HYPERLANE_DUSK_REPRO_WORKDIR:-}" ]; then
        REPRO_DIR="$HYPERLANE_DUSK_REPRO_WORKDIR"
        mkdir -p "$REPRO_DIR"
        CLEANUP_REPRO_DIR=0
    else
        REPRO_DIR="$(mktemp -d /tmp/hyperlane-dusk-repro-XXXXXXXX)"
        CLEANUP_REPRO_DIR=1
    fi

    DUSK_LAYOUT_DIR="$REPRO_DIR/hyperlane/dusk"
    MONOREPO_LAYOUT_DIR="$REPRO_DIR/hyperlane/hyperlane-monorepo"

    cleanup_repro_layout() {
        if [ "$CLEANUP_REPRO_DIR" -eq 1 ]; then
            git -C "$ROOT" worktree remove --force "$DUSK_LAYOUT_DIR" >/dev/null 2>&1 || true
            if [ "$RUN_AGENT_CHECK" -eq 1 ]; then
                git -C "$MONOREPO_DIR" worktree remove --force "$MONOREPO_LAYOUT_DIR" >/dev/null 2>&1 || true
            fi
            rm -rf "$REPRO_DIR"
        fi
    }
    trap cleanup_repro_layout EXIT

    mkdir -p "$REPRO_DIR/hyperlane"
    ln -s "$RUSK_DIR" "$REPRO_DIR/rusk-private"
    git -C "$ROOT" worktree add --detach "$DUSK_LAYOUT_DIR" "$(git -C "$ROOT" rev-parse HEAD)"

    rerun_args=()
    if [ "$RUN_AGENT_CHECK" -eq 1 ]; then
        git -C "$MONOREPO_DIR" worktree add --detach "$MONOREPO_LAYOUT_DIR" "$(git -C "$MONOREPO_DIR" rev-parse HEAD)"
        rerun_args+=(--agent-check --monorepo-dir "$MONOREPO_LAYOUT_DIR")
    fi

    info "Created compatible repro layout at $REPRO_DIR"
    info "Using requested Rusk path through $REPRO_DIR/rusk-private"
    (
        cd "$DUSK_LAYOUT_DIR"
        HYPERLANE_DUSK_REPRO_LAYOUT=1 bash scripts/local-repro-check.sh "${rerun_args[@]}"
    )
    exit 0
fi

[ -d "$DEFAULT_RUSK_DIR/core" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/core"
[ -d "$DEFAULT_RUSK_DIR/vm" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/vm"
[ -d "$DEFAULT_RUSK_DIR/rusk-prover" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/rusk-prover"
[ -d "$DEFAULT_RUSK_DIR/data-drivers/data-driver" ] || fail "missing Rusk path dependency: $DEFAULT_RUSK_DIR/data-drivers/data-driver"

info "Using Rusk path dependencies from $DEFAULT_RUSK_DIR"

info "Building Dusk contract WASMs"
make all

info "Running targeted Dusk wasm clippy checks"
make clippy-contracts

info "Running Dusk type tests"
cargo test -p hyperlane-dusk-types

info "Running Dusk VM integration tests"
cargo test -p hyperlane-dusk-integration-tests

info "Running data-driver tests"
cargo test -p hyperlane-dusk-data-driver

info "Running dusk-tx tests"
cargo test -p dusk-tx

info "Running data-driver tests"
cargo test -p hyperlane-dusk-data-driver

info "Building data-driver release WASM"
make data-driver

info "Checking standalone E2E operator binary"
cargo check -p hyperlane-dusk-e2e

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
