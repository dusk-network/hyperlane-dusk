#!/usr/bin/env bash
# Verify that the manual workflow dispatcher PR and full implementation PR
# can land in either order without drifting the dispatcher-owned files.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

REMOTE="${REMOTE:-origin}"
BASE_BRANCH="${BASE_BRANCH:-main}"
DISPATCHER_BRANCH="${DISPATCHER_BRANCH:-ci/manual-repro-workflow}"
IMPLEMENTATION_BRANCH="${IMPLEMENTATION_BRANCH:-feat/dusk-hardening-v2}"
FETCH_REFS="${FETCH_REFS:-1}"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t hyperlane-merge-order-logs.XXXXXX)}"

BASE_REF="${BASE_REF:-$REMOTE/$BASE_BRANCH}"
DISPATCHER_REF="${DISPATCHER_REF:-$REMOTE/$DISPATCHER_BRANCH}"
IMPLEMENTATION_REF="${IMPLEMENTATION_REF:-$REMOTE/$IMPLEMENTATION_BRANCH}"

TRACKED_PATHS=(
    ".github/workflows/manual-repro-check.yml"
    ".github/actionlint.yaml"
)

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

merge_order() {
    local worktree="$1"
    local first_ref="$2"
    local second_ref="$3"
    local first_log="$4"
    local second_log="$5"
    local diff_log="$6"

    git -C "$ROOT" worktree add --detach "$worktree" "$BASE_REF" >/dev/null
    git -C "$worktree" merge --no-edit "$first_ref" >"$first_log" 2>&1
    git -C "$worktree" merge --no-edit "$second_ref" >"$second_log" 2>&1
    git -C "$worktree" diff --exit-code "$IMPLEMENTATION_REF" -- \
        "${TRACKED_PATHS[@]}" >"$diff_log" 2>&1
}

require_cmd git

[ -d "$ROOT/.git" ] || fail "$ROOT is not a git repository"
mkdir -p "$LOG_DIR"

if [ "$FETCH_REFS" = "1" ]; then
    git -C "$ROOT" fetch "$REMOTE" \
        "+refs/heads/$BASE_BRANCH:refs/remotes/$REMOTE/$BASE_BRANCH" \
        "+refs/heads/$DISPATCHER_BRANCH:refs/remotes/$REMOTE/$DISPATCHER_BRANCH" \
        "+refs/heads/$IMPLEMENTATION_BRANCH:refs/remotes/$REMOTE/$IMPLEMENTATION_BRANCH" \
        --prune
fi

base_sha="$(git -C "$ROOT" rev-parse "$BASE_REF")"
dispatcher_sha="$(git -C "$ROOT" rev-parse "$DISPATCHER_REF")"
implementation_sha="$(git -C "$ROOT" rev-parse "$IMPLEMENTATION_REF")"

workdir="$(mktemp -d -t hyperlane-merge-order.XXXXXX)"
worktree_a="$workdir/order-dispatcher-then-implementation"
worktree_b="$workdir/order-implementation-then-dispatcher"

cleanup() {
    git -C "$ROOT" worktree remove --force "$worktree_a" >/dev/null 2>&1 || true
    git -C "$ROOT" worktree remove --force "$worktree_b" >/dev/null 2>&1 || true
    rm -rf "$workdir"
}
trap cleanup EXIT

merge_order \
    "$worktree_a" \
    "$DISPATCHER_REF" \
    "$IMPLEMENTATION_REF" \
    "$LOG_DIR/order-a-merge-dispatcher.log" \
    "$LOG_DIR/order-a-merge-implementation.log" \
    "$LOG_DIR/order-a-dispatcher-file-diff.log"

merge_order \
    "$worktree_b" \
    "$IMPLEMENTATION_REF" \
    "$DISPATCHER_REF" \
    "$LOG_DIR/order-b-merge-implementation.log" \
    "$LOG_DIR/order-b-merge-dispatcher.log" \
    "$LOG_DIR/order-b-dispatcher-file-diff.log"

cat <<EOF
dispatcherMergeOrderSmoke: passed
base: $base_sha
dispatcher: $dispatcher_sha
implementation: $implementation_sha
orderA: dispatcher then implementation merged cleanly; dispatcher file diff vs implementation branch is empty
orderB: implementation then dispatcher merged cleanly; dispatcher file diff vs implementation branch is empty
logDir: $LOG_DIR
EOF
