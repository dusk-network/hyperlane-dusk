#!/usr/bin/env bash
# Extract durable evidence archives and scan the extracted contents for secrets.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCHIVE_DIR="${1:-$ROOT/../.codex-backups}"
ARCHIVE_UNSAFE_MEMBER_PATTERN="${ARCHIVE_UNSAFE_MEMBER_PATTERN:-(^/|(^|/)\.\.(/|$))}"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

rg_to_file() {
    out_file="$1"
    label="$2"
    shift 2

    set +e
    rg "$@" >"$out_file"
    rg_status=$?
    set -e

    if [ "$rg_status" -eq 0 ]; then
        return 0
    fi
    if [ "$rg_status" -eq 1 ]; then
        return 1
    fi
    cat "$out_file" >&2
    fail "$label scan failed"
}

command -v rg >/dev/null 2>&1 || fail "rg is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"
[ -d "$ARCHIVE_DIR" ] || fail "archive directory not found: $ARCHIVE_DIR"

archive_list="$(mktemp -t hyperlane-archive-list.XXXXXX)"
member_list="$(mktemp -t hyperlane-archive-members.XXXXXX)"
member_details="$(mktemp -t hyperlane-archive-member-details.XXXXXX)"
unsafe_member_hits="$(mktemp -t hyperlane-archive-unsafe-members.XXXXXX)"
special_member_list="$(mktemp -t hyperlane-archive-special-members.XXXXXX)"
scan_root="$(mktemp -d -t hyperlane-archive-hygiene.XXXXXX)"
trap 'rm -rf "$scan_root"; rm -f "$archive_list" "$member_list" "$member_details" "$unsafe_member_hits" "$special_member_list"' EXIT

find "$ARCHIVE_DIR" -maxdepth 1 -type f -name '*.tgz' -print | sort >"$archive_list"
[ -s "$archive_list" ] || fail "no .tgz archives found in $ARCHIVE_DIR"

info "Extracting archives from $ARCHIVE_DIR"
while IFS= read -r archive; do
    name="$(basename "$archive" .tgz)"
    dest="$scan_root/$name"

    tar -tzf "$archive" >"$member_list"
    if rg_to_file "$unsafe_member_hits" "archive member path" -n "$ARCHIVE_UNSAFE_MEMBER_PATTERN" "$member_list"; then
        cat "$unsafe_member_hits" >&2
        fail "archive contains unsafe member path: $archive"
    fi

    tar -tvzf "$archive" >"$member_details"
    if ! awk '
        substr($0, 1, 1) != "-" && substr($0, 1, 1) != "d" {
            print
            bad = 1
        }
        END { exit bad }
    ' "$member_details"; then
        fail "archive contains non-regular/non-directory members: $archive"
    fi

    mkdir -p "$dest"
    tar --no-same-owner --no-same-permissions -xzf "$archive" -C "$dest"
    find "$dest" \( -type l -o -type p -o -type b -o -type c -o -type s \) -print >"$special_member_list"
    if [ -s "$special_member_list" ]; then
        cat "$special_member_list" >&2
        fail "archive extracted symlinks or special files: $archive"
    fi
    info "Extracted $(basename "$archive")"
done <"$archive_list"

bash scripts/secret-hygiene-check.sh "$scan_root"
info "Archive hygiene checks passed"
