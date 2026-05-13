#!/usr/bin/env bash
# Extract durable evidence archives and scan the extracted contents for secrets.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCHIVE_DIR="${1:-$ROOT/../.codex-backups}"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

command -v rg >/dev/null 2>&1 || fail "rg is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"
[ -d "$ARCHIVE_DIR" ] || fail "archive directory not found: $ARCHIVE_DIR"

archive_list="$(mktemp -t hyperlane-archive-list.XXXXXX)"
member_list="$(mktemp -t hyperlane-archive-members.XXXXXX)"
scan_root="$(mktemp -d -t hyperlane-archive-hygiene.XXXXXX)"
trap 'rm -rf "$scan_root"; rm -f "$archive_list" "$member_list"' EXIT

find "$ARCHIVE_DIR" -maxdepth 1 -type f -name '*.tgz' -print | sort >"$archive_list"
[ -s "$archive_list" ] || fail "no .tgz archives found in $ARCHIVE_DIR"

info "Extracting archives from $ARCHIVE_DIR"
while IFS= read -r archive; do
    name="$(basename "$archive" .tgz)"
    dest="$scan_root/$name"

    tar -tzf "$archive" >"$member_list"
    if rg -n '(^/|(^|/)\.\.(/|$))' "$member_list"; then
        fail "archive contains unsafe member path: $archive"
    fi

    mkdir -p "$dest"
    tar -xzf "$archive" -C "$dest"
    info "Extracted $(basename "$archive")"
done <"$archive_list"

bash scripts/secret-hygiene-check.sh "$scan_root"
info "Archive hygiene checks passed"
