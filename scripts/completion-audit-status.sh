#!/usr/bin/env bash
# Verify durable preservation evidence for the Dusk Hyperlane revival audit.
#
# This script is intentionally narrow and read-only. It checks the parts of the
# completion audit that are easy to drift or misquote: prototype archive refs,
# local backup artifact hashes, and untracked source state in the active repos.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
BACKUP_DIR="${BACKUP_DIR:-$ROOT/../.codex-backups/dusk-hyperlane-20260511T133907Z}"

DUSK_REMOTE="${DUSK_REMOTE:-origin}"
MONOREPO_REMOTE="${MONOREPO_REMOTE:-origin}"
DUSK_ARCHIVE_BRANCH="${DUSK_ARCHIVE_BRANCH:-archive/dusk-hyperlane-prototype-20260511}"
MONOREPO_ARCHIVE_BRANCH="${MONOREPO_ARCHIVE_BRANCH:-archive/dusk-prototype-20260511}"
DUSK_ACTIVE_BRANCH="${DUSK_ACTIVE_BRANCH:-feat/dusk-hardening-v2}"
MONOREPO_ACTIVE_BRANCH="${MONOREPO_ACTIVE_BRANCH:-feat/dusk-support-v2}"
UNTRACKED_SOURCE_GATE_ONLY="${UNTRACKED_SOURCE_GATE_ONLY:-0}"

EXPECTED_DUSK_ARCHIVE_SHA="${EXPECTED_DUSK_ARCHIVE_SHA:-c5ce2135407dad6420d010bdafe82a0b9b4bb78d}"
EXPECTED_MONOREPO_ARCHIVE_SHA="${EXPECTED_MONOREPO_ARCHIVE_SHA:-8e399103b24673f837c04f4227e49c45c8366e7c}"

EXPECTED_DUSK_TREE_SHA256="${EXPECTED_DUSK_TREE_SHA256:-8b38b322f807735944501c280f554f83c278a4cebe62e2282478e03da6bdd6ee}"
EXPECTED_HYPERLANE_DUSK_UNTRACKED_SHA256="${EXPECTED_HYPERLANE_DUSK_UNTRACKED_SHA256:-52942cc44b0c87e86b7a1ed8533273dc664eb27b5cea1ddf8d62ee46b9ab8900}"
EXPECTED_HYPERLANE_MONOREPO_TRACKED_SHA256="${EXPECTED_HYPERLANE_MONOREPO_TRACKED_SHA256:-2fe0dc466de389167f756eae23d926234c9df31c1de435a4e68006355bfc7d0e}"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

section() {
    printf '\n== %s ==\n' "$1"
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

remote_branch_sha() {
    local repo_dir="$1"
    local remote="$2"
    local branch="$3"
    local sha

    sha="$(git -C "$repo_dir" ls-remote "$remote" "refs/heads/$branch" | awk '{print $1}')"
    [ -n "$sha" ] || fail "$repo_dir remote $remote branch $branch was not found"
    printf '%s\n' "$sha"
}

assert_equal() {
    local label="$1"
    local actual="$2"
    local expected="$3"

    printf '%s: %s\n' "$label" "$actual"
    [ "$actual" = "$expected" ] || fail "$label expected $expected"
}

assert_file_sha256() {
    local path="$1"
    local expected="$2"
    local actual

    [ -f "$path" ] || fail "missing backup artifact: $path"
    actual="$(sha256sum "$path" | awk '{print $1}')"
    printf '%s: %s\n' "$path" "$actual"
    [ "$actual" = "$expected" ] || fail "$path expected sha256 $expected"
}

assert_no_untracked_source() {
    local label="$1"
    local repo_dir="$2"
    local hits

    hits="$(git -C "$repo_dir" ls-files --others --exclude-standard)"
    if [ -n "$hits" ]; then
        printf '%s\n' "$hits" >&2
        fail "$label has untracked source paths"
    fi
    printf '%sUntrackedSource: none\n' "$label"
}

check_untracked_source() {
    section "Untracked Source"
    assert_no_untracked_source "dusk" "$ROOT"
    assert_no_untracked_source "monorepo" "$MONOREPO_DIR"
}

require_cmd git
require_cmd awk
require_cmd sha256sum

git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || fail "$ROOT is not a git repository"
git -C "$MONOREPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || fail "$MONOREPO_DIR is not a git repository"

MONOREPO_DIR="$(cd "$MONOREPO_DIR" && pwd -P)"

if [ "$UNTRACKED_SOURCE_GATE_ONLY" = "1" ]; then
    check_untracked_source
    section "Summary"
    echo "completionAuditStatus: passed"
    exit 0
fi

section "Archive Branches"
dusk_archive_sha="$(remote_branch_sha "$ROOT" "$DUSK_REMOTE" "$DUSK_ARCHIVE_BRANCH")"
monorepo_archive_sha="$(remote_branch_sha "$MONOREPO_DIR" "$MONOREPO_REMOTE" "$MONOREPO_ARCHIVE_BRANCH")"
assert_equal "duskArchiveRef" "$dusk_archive_sha" "$EXPECTED_DUSK_ARCHIVE_SHA"
assert_equal "monorepoArchiveRef" "$monorepo_archive_sha" "$EXPECTED_MONOREPO_ARCHIVE_SHA"

section "Active Branch Refs"
printf 'duskActiveRef: %s\n' "$(remote_branch_sha "$ROOT" "$DUSK_REMOTE" "$DUSK_ACTIVE_BRANCH")"
printf 'monorepoActiveRef: %s\n' "$(remote_branch_sha "$MONOREPO_DIR" "$MONOREPO_REMOTE" "$MONOREPO_ACTIVE_BRANCH")"

section "Backup Artifact Hashes"
assert_file_sha256 "$BACKUP_DIR/dusk-tree.tgz" "$EXPECTED_DUSK_TREE_SHA256"
assert_file_sha256 "$BACKUP_DIR/hyperlane-dusk-untracked.tgz" "$EXPECTED_HYPERLANE_DUSK_UNTRACKED_SHA256"
assert_file_sha256 "$BACKUP_DIR/hyperlane-monorepo-tracked.diff" "$EXPECTED_HYPERLANE_MONOREPO_TRACKED_SHA256"

check_untracked_source

section "Summary"
echo "completionAuditStatus: passed"
