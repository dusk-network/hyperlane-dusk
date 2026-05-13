#!/usr/bin/env bash
# Regression tests for local guard paths that must fail closed on scan errors.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

command -v tar >/dev/null 2>&1 || fail "tar is required"
command -v rg >/dev/null 2>&1 || fail "rg is required"

workdir="$(mktemp -d -t hyperlane-fail-closed-test.XXXXXX)"
trap 'rm -rf "$workdir"' EXIT

expect_fail() {
    local label="$1"
    local pattern="$2"
    shift 2
    local log="$workdir/$label.log"

    if "$@" >"$log" 2>&1; then
        cat "$log" >&2
        fail "$label: expected command to fail"
    fi

    if ! rg -q "$pattern" "$log"; then
        cat "$log" >&2
        fail "$label: failure did not include expected pattern: $pattern"
    fi

    info "$label: failed closed as expected"
}

mkdir -p "$workdir/archive/src" "$workdir/archive/archives"
printf 'safe log\n' >"$workdir/archive/src/log.txt"
tar -czf "$workdir/archive/archives/safe.tgz" -C "$workdir/archive/src" .

expect_fail \
    archive-invalid-pattern \
    'archive member path scan failed' \
    env ARCHIVE_UNSAFE_MEMBER_PATTERN='[invalid' \
    bash scripts/archive-hygiene-check.sh "$workdir/archive/archives"

expect_fail \
    gate-status-invalid-placeholder-pattern \
    'Dusk repo runtime placeholder scan failed' \
    env DUSK_PLACEHOLDER_PATTERN='[invalid' \
    bash scripts/release-gate-status.sh --placeholder-scan-only

expect_fail \
    review-hygiene-invalid-agent-pattern \
    'Dusk agent runtime panic/placeholder scan failed' \
    env AGENT_PLACEHOLDER_PATTERN='[invalid' \
    bash scripts/github-review-hygiene.sh --agent-placeholder-scan-only

expect_fail \
    report-hygiene-invalid-pattern \
    'report hygiene scan failed' \
    env STALE_REPORT_PATTERNS='[invalid' \
    bash scripts/report-hygiene-check.sh

info "Fail-closed self-test passed"
