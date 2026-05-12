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
SELF_TEST=0

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

usage() {
    cat <<EOF
Usage: bash scripts/dependency-alert-status.sh [options]

Fetch open Dependabot alerts for Cargo.lock and compare each alert's vulnerable
version range with the current local Cargo.lock package versions.

Options:
  --summary-only      Print only counts and grouped current locked versions.
  --self-test         Run the vulnerable-version-range parser self-test and
                     exit without querying GitHub.
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
        --self-test)
            SELF_TEST=1
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

command -v sort >/dev/null 2>&1 || fail "sort is required"

version_at_least() {
    local current="$1"
    local required="$2"
    local lowest

    lowest="$(printf '%s\n%s\n' "$required" "$current" | sort -V | head -n1)"
    [ "$lowest" = "$required" ]
}

version_less_than() {
    local current="$1"
    local required="$2"
    local lowest

    [ "$current" != "$required" ] || return 1
    lowest="$(printf '%s\n%s\n' "$current" "$required" | sort -V | head -n1)"
    [ "$lowest" = "$current" ]
}

version_less_or_equal() {
    local current="$1"
    local required="$2"
    local lowest

    lowest="$(printf '%s\n%s\n' "$current" "$required" | sort -V | head -n1)"
    [ "$lowest" = "$current" ]
}

version_greater_than() {
    local current="$1"
    local required="$2"

    [ "$current" != "$required" ] || return 1
    version_at_least "$current" "$required"
}

trim() {
    local value="$1"

    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "$value"
}

version_satisfies_constraint() {
    local version="$1"
    local constraint
    local required

    constraint="$(trim "$2")"
    case "$constraint" in
        ">="*)
            required="$(trim "${constraint#>=}")"
            version_at_least "$version" "$required"
            ;;
        "<="*)
            required="$(trim "${constraint#<=}")"
            version_less_or_equal "$version" "$required"
            ;;
        ">"*)
            required="$(trim "${constraint#>}")"
            version_greater_than "$version" "$required"
            ;;
        "<"*)
            required="$(trim "${constraint#<}")"
            version_less_than "$version" "$required"
            ;;
        "="*)
            required="$(trim "${constraint#=}")"
            [ "$version" = "$required" ]
            ;;
        "")
            return 2
            ;;
        *)
            return 2
            ;;
    esac
}

version_in_range() {
    local version="$1"
    local range="$2"
    local constraint
    local result

    IFS=',' read -r -a constraints <<<"$range"
    for constraint in "${constraints[@]}"; do
        result=0
        version_satisfies_constraint "$version" "$constraint" || result=$?
        if [ "$result" -eq 2 ]; then
            return 2
        fi
        if [ "$result" -ne 0 ]; then
            return 1
        fi
    done
    return 0
}

assert_range() {
    local version="$1"
    local range="$2"
    local expected="$3"
    local result=0

    version_in_range "$version" "$range" || result=$?
    case "$expected:$result" in
        in:0|out:1|unparsed:2)
            ;;
        *)
            fail "range self-test failed: version=$version range=$range expected=$expected got=$result"
            ;;
    esac
}

run_self_test() {
    assert_range "0.1.5" "< 0.1.6" in
    assert_range "0.1.6" "< 0.1.6" out
    assert_range "0.16.2" ">= 0.9.0, < 0.16.3" in
    assert_range "0.16.4" ">= 0.9.0, < 0.16.3" out
    assert_range "0.8.6" ">= 0.7.0, < 0.8.6" out
    assert_range "0.9.2" ">= 0.9.0, < 0.9.3" in
    assert_range "0.9.3" ">= 0.9.0, < 0.9.3" out
    assert_range "36.0.6" ">= 25.0.0, < 36.0.6" out
    assert_range "25.0.0" ">= 25.0.0, < 36.0.6" in
    assert_range "1.2.3" "= 1.2.3" in
    assert_range "1.2.4" "= 1.2.3" out
    assert_range "1.2.4" "> 1.2.3" in
    assert_range "1.2.3" "<= 1.2.3" in
    assert_range "1.2.4" "~> 1.2" unparsed
}

if [ "$SELF_TEST" -eq 1 ]; then
    run_self_test
    echo "dependency alert range parser self-test: passed"
    exit 0
fi

run_self_test >/dev/null

command -v gh >/dev/null 2>&1 || fail "gh is required"
command -v awk >/dev/null 2>&1 || fail "awk is required"

[ -f "$LOCKFILE" ] || fail "missing lockfile: $LOCKFILE"

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
        (.security_vulnerability.first_patched_version.identifier // "none"),
        (.security_vulnerability.vulnerable_version_range // "none")
    ] | @tsv' >"$alerts"

total=0
no_vulnerable_locked=0
vulnerable_locked=0
without_range=0
without_patch=0

while IFS=$'\t' read -r number package manifest severity ghsa patched_version vulnerable_range; do
    [ -n "$number" ] || continue

    total=$((total + 1))
    locked_versions="$(
        awk -F '\t' -v package="$package" '$1 == package {print $2}' "$lock_versions" \
            | sort -Vu \
            | paste -sd ','
    )"

    status="no_vulnerable_locked_versions"
    vulnerable_versions=""
    if [ -z "$locked_versions" ]; then
        status="no_locked_package"
    elif [ "$patched_version" = "none" ]; then
        status="no_first_patched_version"
    elif [ "$vulnerable_range" = "none" ]; then
        status="no_vulnerable_version_range"
    else
        IFS=',' read -r -a versions <<<"$locked_versions"
        for version in "${versions[@]}"; do
            result=0
            version_in_range "$version" "$vulnerable_range" || result=$?
            if [ "$result" -eq 2 ]; then
                status="unparsed_vulnerable_version_range"
                break
            fi
            if [ "$result" -eq 0 ]; then
                if [ -z "$vulnerable_versions" ]; then
                    vulnerable_versions="$version"
                else
                    vulnerable_versions="$vulnerable_versions,$version"
                fi
            fi
        done

        if [ -n "$vulnerable_versions" ]; then
            status="vulnerable_locked_version_present"
        fi
    fi

    case "$status" in
        no_vulnerable_locked_versions|no_locked_package)
            no_vulnerable_locked=$((no_vulnerable_locked + 1))
            ;;
        no_first_patched_version)
            without_patch=$((without_patch + 1))
            ;;
        no_vulnerable_version_range|unparsed_vulnerable_version_range)
            without_range=$((without_range + 1))
            ;;
        *)
            vulnerable_locked=$((vulnerable_locked + 1))
            ;;
    esac

    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$number" "$package" "$manifest" "$severity" "$ghsa" \
        "$patched_version" "$vulnerable_range" "${locked_versions:-none}" \
        "${vulnerable_versions:-none}" "$status" >>"$details"
done <"$alerts"

echo "openCargoLockAlerts: $total"
echo "alertsWithNoVulnerableLockedVersions: $no_vulnerable_locked"
echo "alertsWithVulnerableLockedVersions: $vulnerable_locked"
echo "alertsWithoutVulnerableRange: $without_range"
echo "alertsWithoutFirstPatchedVersion: $without_patch"

if [ "$SUMMARY_ONLY" -eq 0 ] && [ -s "$details" ]; then
    echo
    echo "number package manifest severity ghsa firstPatched vulnerableRange lockedVersions vulnerableLockedVersions status"
    sort -k2,2 -k1,1n "$details" \
        | awk -F '\t' '{printf "%s %s %s %s %s %s %s %s %s %s\n", $1, $2, $3, $4, $5, $6, $7, $8, $9, $10}'
fi

if [ -s "$details" ]; then
    echo
    echo "currentLockedVersions:"
    awk -F '\t' '
        {
            package = $2
            versions = $8
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

if [ "$vulnerable_locked" -ne 0 ] || [ "$without_range" -ne 0 ] || [ "$without_patch" -ne 0 ]; then
    exit 1
fi
