#!/usr/bin/env bash
# Scan local review reports for stale evidence literals that are not covered by
# the GitHub PR/issue export hygiene guard.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REPORT_FILES="${REPORT_FILES:-GOAL_AUDIT.md TEST_REPORT.md CI_REPRO_STRATEGY.md}"
STALE_REPORT_PATTERNS="${STALE_REPORT_PATTERNS:-25790821003|25791389384|75755608816|75757564450|0c5a7fa10152b117c1e60fbbcfd2618db370aec4|1778686937|1778686993|1778687061|https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4442208973|https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443848247|contains only.*manual-repro-check\\.yml.*and.*actionlint\\.yaml|latest clean-layout repro source ref[[:space:]]+\`8d3704e8f5a3ab0976b97fc3a68319e112e8affc\`|latest clean-layout repro monorepo[[:space:]]+ref \`9050143c1ef12f76d117ee97effa79da8df3e334\`|25819297778|25819297742|25804689562|25804689294|75856037468|75856036345|75804009086|75804008587|ae937e79e60af2d1c6f02e303be8956bddf6a133|/tmp/hyperlane-merge-order-logs\\.eGzQ7Q|live [A-Za-z -]*PR head is \`[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+\`|current [A-Za-z -]*PR head is \`[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]+\`}"
GOAL_AUDIT_FILE="${GOAL_AUDIT_FILE:-GOAL_AUDIT.md}"
GOAL_AUDIT_STALE_PATTERNS="${GOAL_AUDIT_STALE_PATTERNS:-head under test \`[0-9a-f][0-9a-f]+\`|Latest command logs.*hyperlane-merge-order-logs\\.[A-Za-z0-9]+}"

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
EOF
}

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    usage
    exit 0
fi

[ "$#" -eq 0 ] || fail "unknown argument: $1"
command -v rg >/dev/null 2>&1 || fail "rg is required"

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

info "Report hygiene checks passed"
