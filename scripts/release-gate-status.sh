#!/usr/bin/env bash
# Print the current machine-checkable release gate status for Dusk Hyperlane.
#
# This script is intentionally read-only except when --fetch-upstream is used,
# which refreshes the local Hyperlane upstream/main ref before reporting drift.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
DUSK_REPO="${DUSK_REPO:-dusk-network/hyperlane-dusk}"
MONOREPO_REPO="${MONOREPO_REPO:-dusk-network/hyperlane-monorepo}"
UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
FETCH_UPSTREAM=0

usage() {
    cat <<EOF
Usage: bash scripts/release-gate-status.sh [options]

Prints the current machine-checkable review status:
  - local Dusk and Hyperlane worktree state
  - Dusk and monorepo PR state, mergeability, reviews, and status checks
  - production sign-off checklist counts from dusk-network/hyperlane-dusk#2
  - workflow visibility for dusk-network/hyperlane-dusk
  - Hyperlane upstream/main drift for the local monorepo checkout
  - Dusk agent placeholder scan in rust/main/chains/hyperlane-dusk

Options:
  --fetch-upstream     Run git fetch <upstream-remote> main before drift check.
  --monorepo-dir DIR   Hyperlane monorepo checkout.
                       Default: $MONOREPO_DIR
  -h, --help           Show this help.

Environment:
  DUSK_REPO            GitHub repo for Dusk contract/tooling PRs.
                       Default: $DUSK_REPO
  MONOREPO_REPO        GitHub repo for Hyperlane integration PRs.
                       Default: $MONOREPO_REPO
  UPSTREAM_REMOTE      Hyperlane upstream remote name.
                       Default: $UPSTREAM_REMOTE
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --fetch-upstream)
            FETCH_UPSTREAM=1
            shift
            ;;
        --monorepo-dir)
            MONOREPO_DIR="${2:-}"
            [ -n "$MONOREPO_DIR" ] || {
                echo "[FAIL] --monorepo-dir requires a path" >&2
                exit 1
            }
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "[FAIL] unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

command -v git >/dev/null 2>&1 || {
    echo "[FAIL] git is required" >&2
    exit 1
}
command -v gh >/dev/null 2>&1 || {
    echo "[FAIL] gh is required" >&2
    exit 1
}

if [ -d "$MONOREPO_DIR" ]; then
    MONOREPO_DIR="$(cd "$MONOREPO_DIR" && pwd -P)"
fi

section() {
    printf '\n== %s ==\n' "$1"
}

print_pr() {
    local repo="$1"
    local number="$2"
    local label="$3"

    section "$label PR"
    gh pr view "$number" --repo "$repo" \
        --json state,mergeable,headRefOid,reviewDecision,reviewRequests,statusCheckRollup,url \
        --jq '
            "url: \(.url)\n" +
            "state: \(.state)\n" +
            "mergeable: \(.mergeable)\n" +
            "head: \(.headRefOid)\n" +
            "reviewDecision: \(.reviewDecision // "")\n" +
            "reviewRequests: \([.reviewRequests[].login] | join(", "))\n" +
            "statusChecks: \(.statusCheckRollup | length)"
        '
}

unchecked_count() {
    grep -c '^- \[ \]' || true
}

checked_count() {
    grep -c '^- \[x\]' || true
}

section "Local Worktrees"
echo "dusk: $(git -C "$ROOT" rev-parse HEAD)"
git -C "$ROOT" status --short --branch
if [ -d "$MONOREPO_DIR/.git" ] || git -C "$MONOREPO_DIR" rev-parse --git-dir >/dev/null 2>&1; then
    echo "monorepo: $(git -C "$MONOREPO_DIR" rev-parse HEAD)"
    git -C "$MONOREPO_DIR" status --short --branch
else
    echo "monorepo: missing checkout at $MONOREPO_DIR"
fi

print_pr "$DUSK_REPO" 1 "Dusk"
print_pr "$MONOREPO_REPO" 1 "Hyperlane monorepo"

section "Production Sign-Off Issue"
issue_body="$(gh issue view 2 --repo "$DUSK_REPO" --json body --jq .body)"
echo "url: https://github.com/$DUSK_REPO/issues/2"
echo "uncheckedItems: $(printf '%s\n' "$issue_body" | unchecked_count)"
echo "checkedItems: $(printf '%s\n' "$issue_body" | checked_count)"

section "Workflow Visibility"
workflow_output="$(gh workflow list --repo "$DUSK_REPO" --all || true)"
if [ -n "$workflow_output" ]; then
    printf '%s\n' "$workflow_output"
else
    echo "no workflows visible through GitHub API"
fi

section "Hyperlane Upstream Drift"
if [ -d "$MONOREPO_DIR" ]; then
    if git -C "$MONOREPO_DIR" remote get-url "$UPSTREAM_REMOTE" >/dev/null 2>&1; then
        if [ "$FETCH_UPSTREAM" -eq 1 ]; then
            git -C "$MONOREPO_DIR" fetch "$UPSTREAM_REMOTE" main
        fi
        upstream_ref="$UPSTREAM_REMOTE/main"
        if git -C "$MONOREPO_DIR" rev-parse --verify "$upstream_ref" >/dev/null 2>&1; then
            echo "upstreamHead: $(git -C "$MONOREPO_DIR" rev-parse "$upstream_ref")"
            echo "mergeBase: $(git -C "$MONOREPO_DIR" merge-base HEAD "$upstream_ref")"
            echo "aheadBehind: $(git -C "$MONOREPO_DIR" rev-list --left-right --count HEAD..."$upstream_ref")"
        else
            echo "missing local ref: $upstream_ref"
        fi
    else
        echo "missing remote: $UPSTREAM_REMOTE"
    fi
else
    echo "missing monorepo checkout: $MONOREPO_DIR"
fi

section "Runtime Placeholder Scan"
if [ -d "$MONOREPO_DIR/rust/main/chains/hyperlane-dusk" ]; then
    if git -C "$MONOREPO_DIR" grep -n -E 'todo!|unimplemented!|panic!' -- rust/main/chains/hyperlane-dusk >/tmp/hyperlane-dusk-placeholder-scan.$$; then
        cat /tmp/hyperlane-dusk-placeholder-scan.$$
        rm -f /tmp/hyperlane-dusk-placeholder-scan.$$
    else
        rm -f /tmp/hyperlane-dusk-placeholder-scan.$$
        echo "no matches in rust/main/chains/hyperlane-dusk"
    fi
else
    echo "missing Dusk chain crate in monorepo checkout"
fi

section "Summary"
echo "This script reports machine-checkable gates only."
echo "The objective remains blocked until the unchecked sign-off items, reviews,"
echo "CI/default-branch workflow decision, internal PR merge, and upstream-prep"
echo "gate are resolved by Dusk reviewers."
