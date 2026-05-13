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
untracked_probe="$ROOT/.completion-audit-untracked-probe"
trap 'chmod -R u+rwX "$workdir" 2>/dev/null || true; rm -rf "$workdir"; rm -f "$untracked_probe"' EXIT

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
    env ARCHIVE_SHA256_MANIFEST='' ARCHIVE_EXPECTED_SHA256S='' ARCHIVE_UNSAFE_MEMBER_PATTERN='[invalid' \
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

stale_report="$workdir/stale-report.md"
cat >"$stale_report" <<'EOF'
Stale dispatcher smoke evidence:
- implementation head ae937e79e60af2d1c6f02e303be8956bddf6a133
- log path /tmp/hyperlane-merge-order-logs.eGzQ7Q
EOF
expect_fail \
    report-hygiene-stale-dispatcher-evidence \
    'stale local report evidence found' \
    env REPORT_FILES="$stale_report" \
    bash scripts/report-hygiene-check.sh

stale_goal_audit="$workdir/stale-goal-audit.md"
cat >"$stale_goal_audit" <<'EOF'
Dispatcher smoke evidence:
- head under test `f656de36ea7b7099ab3a1eabcb89b672d9eb9c4d`
- Latest command logs from the refreshed review-gates bundle were written under `/tmp/hyperlane-merge-order-logs.QUAn6t`.
EOF
expect_fail \
    report-hygiene-stale-goal-audit-moving-evidence \
    'stale moving goal-audit evidence found' \
    env REPORT_FILES="$stale_goal_audit" GOAL_AUDIT_FILE="$stale_goal_audit" \
    bash scripts/report-hygiene-check.sh

tmp_only_repro_report="$workdir/tmp-only-repro-report.md"
cat >"$tmp_only_repro_report" <<'EOF'
Latest checkout-v6 clean-layout repro evidence:
- log: /tmp/hyperlane-review-checkout-v6-repro-1778695627.log
- SHA256: 9384e858bdd00f88665969a91cfa583048c4437177b615c7ddf49c676a4c2c12
EOF
expect_fail \
    report-hygiene-missing-latest-repro-archive \
    'latest repro durable archive missing' \
    env REPORT_FILES="$tmp_only_repro_report" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$tmp_only_repro_report" \
        GOAL_AUDIT_FILE="$tmp_only_repro_report" \
    bash scripts/report-hygiene-check.sh

path_only_repro_report="$workdir/path-only-repro-report.md"
cat >"$path_only_repro_report" <<'EOF'
Latest checkout-v6 clean-layout repro evidence:
- archive: /home/hein_/projects/hyperlane/.codex-backups/hyperlane-checkout-v6-repro-1778695627.tgz
- log SHA256: 9384e858bdd00f88665969a91cfa583048c4437177b615c7ddf49c676a4c2c12
EOF
expect_fail \
    report-hygiene-missing-latest-repro-archive-hash \
    'latest repro durable archive hash missing' \
    env REPORT_FILES="$path_only_repro_report" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$path_only_repro_report" \
        GOAL_AUDIT_FILE="$path_only_repro_report" \
    bash scripts/report-hygiene-check.sh

missing_archive_report="$workdir/missing-archive-report.md"
missing_archive_path="$workdir/missing-archive.tgz"
missing_archive_hash="0000000000000000000000000000000000000000000000000000000000000000"
cat >"$missing_archive_report" <<EOF
Latest checkout-v6 clean-layout repro evidence:
- archive: $missing_archive_path
- archive SHA256: $missing_archive_hash
EOF
expect_fail \
    report-hygiene-missing-latest-repro-archive-file \
    'latest repro durable archive file not found' \
    env REPORT_FILES="$missing_archive_report" \
        LATEST_REPRO_ARCHIVE_PATH="$missing_archive_path" \
        LATEST_REPRO_ARCHIVE_SHA256="$missing_archive_hash" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$missing_archive_report" \
        GOAL_AUDIT_FILE="$missing_archive_report" \
    bash scripts/report-hygiene-check.sh

hash_mismatch_report="$workdir/hash-mismatch-report.md"
hash_mismatch_archive="$workdir/hash-mismatch.tgz"
printf 'not the expected archive\n' >"$hash_mismatch_archive"
hash_mismatch_expected="1111111111111111111111111111111111111111111111111111111111111111"
cat >"$hash_mismatch_report" <<EOF
Latest checkout-v6 clean-layout repro evidence:
- archive: $hash_mismatch_archive
- archive SHA256: $hash_mismatch_expected
EOF
expect_fail \
    report-hygiene-latest-repro-archive-hash-mismatch \
    'latest repro durable archive hash mismatch' \
    env REPORT_FILES="$hash_mismatch_report" \
        LATEST_REPRO_ARCHIVE_PATH="$hash_mismatch_archive" \
        LATEST_REPRO_ARCHIVE_SHA256="$hash_mismatch_expected" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$hash_mismatch_report" \
        GOAL_AUDIT_FILE="$hash_mismatch_report" \
    bash scripts/report-hygiene-check.sh

mkdir -p "$workdir/env-artifacts"
printf 'local env placeholder\n' >"$workdir/env-artifacts/.env.bridge"
expect_fail \
    secret-hygiene-env-artifact \
    'runtime artifact path includes secret-like file names' \
    bash scripts/secret-hygiene-check.sh "$workdir/env-artifacts"

mkdir -p "$workdir/private-key-artifacts"
cat >"$workdir/private-key-artifacts/config.json" <<'EOF'
{"signer":{"privateKey":"0x1111111111111111111111111111111111111111111111111111111111111111"}}
EOF
expect_fail \
    secret-hygiene-private-key-artifact \
    'runtime artifact scan found' \
    bash scripts/secret-hygiene-check.sh "$workdir/private-key-artifacts"

mkdir -p "$workdir/secret-artifacts"
printf 'safe log\n' >"$workdir/secret-artifacts/unreadable.log"
chmod 000 "$workdir/secret-artifacts/unreadable.log"
expect_fail \
    secret-hygiene-unreadable-artifact \
    'runtime artifact secret text scan failed' \
    bash scripts/secret-hygiene-check.sh "$workdir/secret-artifacts"

printf 'temporary completion audit probe\n' >"$untracked_probe"
expect_fail \
    completion-audit-untracked-source \
    'dusk has untracked source paths' \
    bash scripts/completion-audit-status.sh
rm -f "$untracked_probe"

info "Fail-closed self-test passed"
