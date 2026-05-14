#!/usr/bin/env bash
# Scan local review reports for stale evidence literals that are not covered by
# the GitHub PR/issue export hygiene guard.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REPORT_FILES="${REPORT_FILES:-GOAL_AUDIT.md TEST_REPORT.md CI_REPRO_STRATEGY.md}"
STALE_REPORT_PATTERNS="${STALE_REPORT_PATTERNS:-25790821003|25791389384|75755608816|75757564450|0c5a7fa10152b117c1e60fbbcfd2618db370aec4|1778686937|1778686993|1778687061|https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4442208973|https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443848247|contains only.*manual-repro-check\\.yml.*and.*actionlint\\.yaml|latest clean-layout repro source ref[[:space:]]+\`8d3704e8f5a3ab0976b97fc3a68319e112e8affc\`|latest clean-layout repro monorepo[[:space:]]+ref \`9050143c1ef12f76d117ee97effa79da8df3e334\`|25819297778|25819297742|25804689562|25804689294|75856037468|75856036345|75804009086|75804008587|ae937e79e60af2d1c6f02e303be8956bddf6a133|/tmp/hyperlane-merge-order-logs\\.eGzQ7Q|live [A-Za-z -]*PR head is \`[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+\`|current [A-Za-z -]*PR head is \`[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+\`|live [A-Za-z -]*PR head to \`[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+\`}"
GOAL_AUDIT_FILE="${GOAL_AUDIT_FILE:-GOAL_AUDIT.md}"
GOAL_AUDIT_STALE_PATTERNS="${GOAL_AUDIT_STALE_PATTERNS:-head under test \`[0-9a-f][0-9a-f]+\`|Latest command logs.*hyperlane-merge-order-logs\\.[A-Za-z0-9]+}"
DECISION_RECORD_FILE="${DECISION_RECORD_FILE:-PRODUCTION_REVIEW_DECISIONS.md}"
DECISION_RECORD_STALE_PATTERNS="${DECISION_RECORD_STALE_PATTERNS:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443963744}"
LATEST_REPRO_ARCHIVE_PATH="${LATEST_REPRO_ARCHIVE_PATH:-/home/hein_/projects/hyperlane/.codex-backups/hyperlane-clean-repro-current-head-1778751867.tgz}"
LATEST_REPRO_ARCHIVE_SHA256="${LATEST_REPRO_ARCHIVE_SHA256:-9e08ce22389f4a209d3d1ed79aa90de8d5384ce77ca7c142019c3264b799b7e7}"
LATEST_REPRO_ARCHIVE_REQUIRED_FILES="${LATEST_REPRO_ARCHIVE_REQUIRED_FILES:-GOAL_AUDIT.md TEST_REPORT.md}"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

usage() {
    cat <<EOF
Usage: bash scripts/report-hygiene-check.sh

Scans local report files for stale review-evidence literals that previously
survived GitHub-facing review hygiene checks.

Environment:
  REPORT_FILES           Space-separated report files to scan.
                         Default: $REPORT_FILES
  STALE_REPORT_PATTERNS  Extended regex for stale local report text.
                         Default: current repository stale-report set.
  GOAL_AUDIT_FILE        Goal audit file to scan for moving evidence pins.
                         Default: $GOAL_AUDIT_FILE
  GOAL_AUDIT_STALE_PATTERNS
                         Extended regex for stale moving evidence in
                         the goal audit file.
                         Default: current goal-audit stale-report set.
  DECISION_RECORD_FILE   Production decision record to scan for stale
                         reviewer-facing evidence links.
                         Default: $DECISION_RECORD_FILE
  DECISION_RECORD_STALE_PATTERNS
                         Extended regex for stale decision-record evidence.
                         Default: current decision-record stale-report set.
  LATEST_REPRO_ARCHIVE_PATH
                         Durable archive path required for latest repro evidence.
                         Default: $LATEST_REPRO_ARCHIVE_PATH
  LATEST_REPRO_ARCHIVE_SHA256
                         Durable archive hash required for latest repro evidence.
                         Default: $LATEST_REPRO_ARCHIVE_SHA256
  LATEST_REPRO_ARCHIVE_REQUIRED_FILES
                         Space-separated files that must mention the durable
                         latest repro evidence archive and hash.
                         Default: $LATEST_REPRO_ARCHIVE_REQUIRED_FILES
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    usage
    exit 0
fi

[ "$#" -eq 0 ] || fail "unknown argument: $1"
command -v rg >/dev/null 2>&1 || fail "rg is required"
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"

stale_hits="$(mktemp -t hyperlane-report-hygiene.XXXXXX)"
trap 'rm -f "$stale_hits"' EXIT

for file in $REPORT_FILES; do
    [ -f "$file" ] || fail "report file not found: $file"
done

info "Checking local report hygiene"

set +e
rg -n -e "$STALE_REPORT_PATTERNS" $REPORT_FILES >"$stale_hits"
rg_status=$?
set -e

if [ "$rg_status" -eq 0 ]; then
    cat "$stale_hits" >&2
    fail "stale local report evidence found"
fi
if [ "$rg_status" -ne 1 ]; then
    cat "$stale_hits" >&2
    fail "report hygiene scan failed"
fi

if [ -f "$GOAL_AUDIT_FILE" ]; then
    set +e
    rg -n -e "$GOAL_AUDIT_STALE_PATTERNS" "$GOAL_AUDIT_FILE" >"$stale_hits"
    rg_status=$?
    set -e

    if [ "$rg_status" -eq 0 ]; then
        cat "$stale_hits" >&2
        fail "stale moving goal-audit evidence found"
    fi
    if [ "$rg_status" -ne 1 ]; then
        cat "$stale_hits" >&2
        fail "goal-audit hygiene scan failed"
    fi
fi

if [ -f "$DECISION_RECORD_FILE" ]; then
    set +e
    rg -n -e "$DECISION_RECORD_STALE_PATTERNS" "$DECISION_RECORD_FILE" >"$stale_hits"
    rg_status=$?
    set -e

    if [ "$rg_status" -eq 0 ]; then
        cat "$stale_hits" >&2
        fail "stale production decision evidence found"
    fi
    if [ "$rg_status" -ne 1 ]; then
        cat "$stale_hits" >&2
        fail "production decision hygiene scan failed"
    fi
fi

if [ -n "$LATEST_REPRO_ARCHIVE_PATH" ]; then
    [ -f "$LATEST_REPRO_ARCHIVE_PATH" ] \
        || fail "latest repro durable archive file not found: $LATEST_REPRO_ARCHIVE_PATH"
    if [ -n "$LATEST_REPRO_ARCHIVE_SHA256" ]; then
        actual_latest_repro_sha256="$(sha256sum "$LATEST_REPRO_ARCHIVE_PATH" | awk '{print $1}')"
        [ "$actual_latest_repro_sha256" = "$LATEST_REPRO_ARCHIVE_SHA256" ] \
            || fail "latest repro durable archive hash mismatch: $LATEST_REPRO_ARCHIVE_PATH"
    fi

    for file in $LATEST_REPRO_ARCHIVE_REQUIRED_FILES; do
        [ -f "$file" ] || fail "latest repro archive required file not found: $file"
        if ! rg -q -F "$LATEST_REPRO_ARCHIVE_PATH" "$file"; then
            fail "latest repro durable archive missing from $file"
        fi
        if [ -n "$LATEST_REPRO_ARCHIVE_SHA256" ] \
            && ! rg -q -F "$LATEST_REPRO_ARCHIVE_SHA256" "$file"; then
            fail "latest repro durable archive hash missing from $file"
        fi
    done
fi

info "Report hygiene checks passed"
