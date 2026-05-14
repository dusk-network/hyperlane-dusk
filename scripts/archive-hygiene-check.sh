#!/usr/bin/env bash
# Extract durable evidence archives and scan the extracted contents for secrets.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCHIVE_DIR="${1:-$ROOT/../.codex-backups}"
ARCHIVE_UNSAFE_MEMBER_PATTERN="${ARCHIVE_UNSAFE_MEMBER_PATTERN:-(^/|(^|/)\.\.(/|$))}"
ARCHIVE_SHA256_MANIFEST="${ARCHIVE_SHA256_MANIFEST-$ROOT/EVIDENCE_ARCHIVES.sha256}"
ARCHIVE_EXPECTED_SHA256S="${ARCHIVE_EXPECTED_SHA256S-hyperlane-clean-repro-current-head-1778750702.tgz=090ed23b59f4a29d5f498fea9019c55164180876ce37a94729128d3ad389a5eb}"

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
command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"
[ -d "$ARCHIVE_DIR" ] || fail "archive directory not found: $ARCHIVE_DIR"

archive_list="$(mktemp -t hyperlane-archive-list.XXXXXX)"
archive_name_list="$(mktemp -t hyperlane-archive-names.XXXXXX)"
manifest_name_list="$(mktemp -t hyperlane-archive-manifest-names.XXXXXX)"
manifest_unsafe_hits="$(mktemp -t hyperlane-archive-manifest-unsafe.XXXXXX)"
member_list="$(mktemp -t hyperlane-archive-members.XXXXXX)"
member_details="$(mktemp -t hyperlane-archive-member-details.XXXXXX)"
unsafe_member_hits="$(mktemp -t hyperlane-archive-unsafe-members.XXXXXX)"
special_member_list="$(mktemp -t hyperlane-archive-special-members.XXXXXX)"
scan_root="$(mktemp -d -t hyperlane-archive-hygiene.XXXXXX)"
trap 'rm -rf "$scan_root"; rm -f "$archive_list" "$archive_name_list" "$manifest_name_list" "$manifest_unsafe_hits" "$member_list" "$member_details" "$unsafe_member_hits" "$special_member_list"' EXIT

find "$ARCHIVE_DIR" -maxdepth 1 -type f -name '*.tgz' -print | sort >"$archive_list"
[ -s "$archive_list" ] || fail "no .tgz archives found in $ARCHIVE_DIR"
sed 's#.*/##' "$archive_list" >"$archive_name_list"

if [ -n "$ARCHIVE_SHA256_MANIFEST" ] && [ -f "$ARCHIVE_SHA256_MANIFEST" ]; then
    info "Checking archive SHA256 manifest $(basename "$ARCHIVE_SHA256_MANIFEST")"
    awk '{print $2}' "$ARCHIVE_SHA256_MANIFEST" | sort >"$manifest_name_list"
    if rg_to_file "$manifest_unsafe_hits" "archive SHA256 manifest path" -n '(^/|/|(^|/)\.\.(/|$))' "$manifest_name_list"; then
        cat "$manifest_unsafe_hits" >&2
        fail "archive SHA256 manifest contains unsafe path"
    fi
    if ! cmp -s "$archive_name_list" "$manifest_name_list"; then
        echo "Archive directory:" >&2
        cat "$archive_name_list" >&2
        echo "Manifest:" >&2
        cat "$manifest_name_list" >&2
        fail "archive SHA256 manifest does not match archive directory"
    fi
    (
        cd "$ARCHIVE_DIR"
        sha256sum -c "$ARCHIVE_SHA256_MANIFEST"
    ) >/dev/null
elif [ -n "$ARCHIVE_EXPECTED_SHA256S" ]; then
    info "Checking expected archive SHA256 values"
    for entry in $ARCHIVE_EXPECTED_SHA256S; do
        archive_name="${entry%%=*}"
        expected_sha256="${entry#*=}"
        archive_path="$ARCHIVE_DIR/$archive_name"
        [ "$archive_name" != "$entry" ] || fail "invalid expected archive SHA256 entry: $entry"
        case "$archive_name" in
            *.tgz) ;;
            *) fail "expected archive SHA256 entry must name a .tgz archive: $archive_name" ;;
        esac
        case "$archive_name" in
            "" | /* | */* | ..)
                fail "expected archive SHA256 entry contains unsafe path: $archive_name"
                ;;
        esac
        [ -f "$archive_path" ] || fail "expected archive not found: $archive_path"
        actual_sha256="$(sha256sum "$archive_path" | awk '{print $1}')"
        [ "$actual_sha256" = "$expected_sha256" ] \
            || fail "$archive_path expected sha256 $expected_sha256 but got $actual_sha256"
    done
fi

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
