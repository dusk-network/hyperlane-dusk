#!/usr/bin/env bash
# Compare open GitHub Dependabot Cargo.lock alerts with the local lockfile.
#
# This does not replace GitHub's alert state. It is a reviewer aid for feature
# branches where Dependabot may still report default-branch or pre-rescan state.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DUSK_REPO="${DUSK_REPO:-dusk-network/hyperlane-dusk}"
LOCKFILE="${LOCKFILE:-Cargo.lock}"
SUMMARY_ONLY=0

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

usage() {
    cat <<EOF
Usage: bash scripts/dependency-alert-status.sh [options]

Fetch open Dependabot alerts for Cargo.lock and compare each alert's first
patched version with the current local Cargo.lock package versions.

Options:
  --summary-only      Print only counts and grouped current locked versions.
  --lockfile PATH     Cargo.lock path to inspect.
                     Default: $LOCKFILE
  -h, --help          Show this help.

Environment:
  DUSK_REPO           GitHub repo whose Dependabot alerts should be queried.
                     Default: $DUSK_REPO
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --summary-only)
            SUMMARY_ONLY=1
            shift
            ;;
        --lockfile)
            LOCKFILE="${2:-}"
            [ -n "$LOCKFILE" ] || fail "--lockfile requires a path"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown argument: $1"
            ;;
    esac
done

command -v gh >/dev/null 2>&1 || fail "gh is required"
command -v awk >/dev/null 2>&1 || fail "awk is required"
command -v sort >/dev/null 2>&1 || fail "sort is required"

[ -f "$LOCKFILE" ] || fail "missing lockfile: $LOCKFILE"

version_at_least() {
    local current="$1"
    local required="$2"
    local lowest

    lowest="$(printf '%s\n%s\n' "$required" "$current" | sort -V | head -n1)"
    [ "$lowest" = "$required" ]
}

tmpdir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT

lock_versions="$tmpdir/lock-versions.tsv"
alerts="$tmpdir/dependabot-alerts.tsv"
details="$tmpdir/dependabot-alert-details.tsv"

awk '
    /^\[\[package\]\]/ {
        if (name != "" && version != "") {
            print name "\t" version
        }
        name = ""
        version = ""
        next
    }

    /^name = / {
        name = $0
        sub(/^name = "/, "", name)
        sub(/"$/, "", name)
        next
    }

    /^version = / {
        version = $0
        sub(/^version = "/, "", version)
        sub(/"$/, "", version)
        next
    }

    END {
        if (name != "" && version != "") {
            print name "\t" version
        }
    }
' "$LOCKFILE" | sort -u >"$lock_versions"

gh api "repos/$DUSK_REPO/dependabot/alerts?state=open&per_page=100" --paginate \
    --jq '.[] | select(.dependency.manifest_path == "Cargo.lock") | [
        .number,
        .dependency.package.name,
        .dependency.manifest_path,
        .security_advisory.severity,
        .security_advisory.ghsa_id,
        (.security_vulnerability.first_patched_version.identifier // "none")
    ] | @tsv' >"$alerts"

total=0
satisfied=0
missing=0
without_patch=0

while IFS=$'\t' read -r number package manifest severity ghsa patched_version; do
    [ -n "$number" ] || continue

    total=$((total + 1))
    locked_versions="$(
        awk -F '\t' -v package="$package" '$1 == package {print $2}' "$lock_versions" \
            | sort -Vu \
            | paste -sd ','
    )"

    status="missing_patched_floor"
    if [ -z "$locked_versions" ]; then
        status="missing_locked_package"
    elif [ "$patched_version" = "none" ]; then
        status="no_first_patched_version"
    else
        IFS=',' read -r -a versions <<<"$locked_versions"
        for version in "${versions[@]}"; do
            if version_at_least "$version" "$patched_version"; then
                status="patched_floor_satisfied"
                break
            fi
        done
    fi

    case "$status" in
        patched_floor_satisfied)
            satisfied=$((satisfied + 1))
            ;;
        no_first_patched_version)
            without_patch=$((without_patch + 1))
            ;;
        *)
            missing=$((missing + 1))
            ;;
    esac

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$number" "$package" "$manifest" "$severity" "$ghsa" \
        "$patched_version" "${locked_versions:-none}" "$status" >>"$details"
done <"$alerts"

echo "openCargoLockAlerts: $total"
echo "patchedFloorSatisfied: $satisfied"
echo "missingPatchedFloor: $missing"
echo "alertsWithoutFirstPatchedVersion: $without_patch"

if [ "$SUMMARY_ONLY" -eq 0 ] && [ -s "$details" ]; then
    echo
    echo "number package manifest severity ghsa firstPatched lockedVersions status"
    sort -k2,2 -k1,1n "$details" \
        | awk -F '\t' '{printf "%s %s %s %s %s %s %s %s\n", $1, $2, $3, $4, $5, $6, $7, $8}'
fi

if [ -s "$details" ]; then
    echo
    echo "currentLockedVersions:"
    awk -F '\t' '
        {
            package = $2
            versions = $7
            if (seen[package] == "") {
                seen[package] = versions
            }
        }

        END {
            for (package in seen) {
                print "  " package ": " seen[package]
            }
        }
    ' "$details" | sort
fi

if [ "$missing" -ne 0 ] || [ "$without_patch" -ne 0 ]; then
    exit 1
fi
