#!/usr/bin/env bash
# Scan local review reports for stale evidence literals that are not covered by
# the GitHub PR/issue export hygiene guard.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

REPORT_FILES="${REPORT_FILES:-GOAL_AUDIT.md TEST_REPORT.md}"
STALE_REPORT_PATTERNS="${STALE_REPORT_PATTERNS:-25790821003|25791389384|75755608816|75757564450|0c5a7fa10152b117c1e60fbbcfd2618db370aec4|1778686937|1778686993|1778687061|https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4442208973|https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443848247}"

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

if rg -n -e "$STALE_REPORT_PATTERNS" $REPORT_FILES >"$stale_hits"; then
    cat "$stale_hits" >&2
    fail "stale local report evidence found"
fi

info "Report hygiene checks passed"
