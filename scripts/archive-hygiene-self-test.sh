#!/usr/bin/env bash
# Regression tests for the evidence archive hygiene scanner.

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

workdir="$(mktemp -d -t hyperlane-archive-hygiene-test.XXXXXX)"
trap 'rm -rf "$workdir"' EXIT

make_case_dir() {
    local name="$1"
    mkdir -p "$workdir/$name/src" "$workdir/$name/archives"
    printf '%s\n' "$workdir/$name"
}

expect_pass() {
    local label="$1"
    local archive_dir="$2"
    local log="$workdir/$label.log"

    if ARCHIVE_EXPECTED_SHA256S="${ARCHIVE_EXPECTED_SHA256S-}" \
        bash scripts/archive-hygiene-check.sh "$archive_dir" >"$log" 2>&1; then
        info "$label: passed as expected"
    else
        cat "$log" >&2
        fail "$label: expected archive hygiene to pass"
    fi
}

expect_fail() {
    local label="$1"
    local archive_dir="$2"
    local pattern="$3"
    local log="$workdir/$label.log"

    if ARCHIVE_EXPECTED_SHA256S="${ARCHIVE_EXPECTED_SHA256S-}" \
        bash scripts/archive-hygiene-check.sh "$archive_dir" >"$log" 2>&1; then
        cat "$log" >&2
        fail "$label: expected archive hygiene to fail"
    fi

    if ! rg -q "$pattern" "$log"; then
        cat "$log" >&2
        fail "$label: failure did not include expected pattern: $pattern"
    fi

    info "$label: failed as expected"
}

case_dir="$(make_case_dir safe)"
printf 'safe log\n' >"$case_dir/src/log.txt"
tar -czf "$case_dir/archives/safe.tgz" -C "$case_dir/src" .
safe_sha256="$(sha256sum "$case_dir/archives/safe.tgz" | awk '{print $1}')"
ARCHIVE_EXPECTED_SHA256S="safe.tgz=$safe_sha256" expect_pass "safe" "$case_dir/archives"
ARCHIVE_EXPECTED_SHA256S="safe.tgz=0000000000000000000000000000000000000000000000000000000000000000" \
    expect_fail "sha-mismatch" "$case_dir/archives" 'expected sha256'

case_dir="$(make_case_dir traversal)"
printf 'unsafe path\n' >"$case_dir/src/file.txt"
tar -czf "$case_dir/archives/traversal.tgz" \
    -C "$case_dir/src" \
    --transform='s#file.txt#../evil.txt#' \
    file.txt 2>/dev/null
expect_fail "traversal" "$case_dir/archives" 'unsafe member path'

case_dir="$(make_case_dir symlink)"
ln -s target "$case_dir/src/link"
tar -czf "$case_dir/archives/symlink.tgz" -C "$case_dir/src" link
expect_fail "symlink" "$case_dir/archives" 'non-regular/non-directory'

case_dir="$(make_case_dir secret)"
printf 'secret_key_bls = do-not-archive\n' >"$case_dir/src/log.txt"
tar -czf "$case_dir/archives/secret.tgz" -C "$case_dir/src" .
expect_fail "secret" "$case_dir/archives" 'runtime artifact scan found'

info "Archive hygiene self-test passed"
