#!/usr/bin/env bash
# Export GitHub review surfaces and scan them for stale evidence text.
#
# This is a read-only guardrail for PR/issue bodies and comments. It catches
# stale SHA/run references in reviewer-facing text and then reuses the secret
# hygiene scanner over the exported files.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

DUSK_REPO="${DUSK_REPO:-dusk-network/hyperlane-dusk}"
MONOREPO_REPO="${MONOREPO_REPO:-dusk-network/hyperlane-monorepo}"
DUSK_PRS="${DUSK_PRS:-1 3}"
MONOREPO_PRS="${MONOREPO_PRS:-1}"
DUSK_ISSUES="${DUSK_ISSUES:-2 4 5 6 7 8 9}"
EXPORT_DIR="${EXPORT_DIR:-}"
KEEP_EXPORT="${KEEP_EXPORT:-1}"
STALE_REVIEW_PATTERNS="${STALE_REVIEW_PATTERNS:-356239661c5121e79e81930fd2b18995bf375b11|78b0cfd19b59705c49639cd8200baa2404344d2c|c0036501b26cc98fb64259807e4cbca929487aec|d192269aedee625be4355a34a6ec5672c4141ec8|10721dcb645adc0d7fc8b4952362e57355c2c67e|bb2fbd685e1bccd0c468de9bbd3f2bb49bbac1d4|0cc3732046181576cd57a536bce585c079c776c2|519478c418498d3520aee650e76292ff2aad1e64|7a00aed51e1a39ec6fd58a5fb8added3a0350f37|e2d5db3f01bb2bc0ef5a0d4eda560cc34ee21862|0aca118ce9cae9ae86c7757f207f49d76face371|06dbf75e5f880ec74d29d59e44c4059298a49685|06dbf75e2d67b0bbc5aa450066bfb5743f79bdd2|1778574482|1778576530|1778577847|28d07e01d1bbc0cf59575a811cde55e844a2abb7|Dusk PR #1 head is now .889a00bb11d589d268ee928d0855e3724dfab0fe|Companion Dusk PR #1 head is now .889a00bb11d589d268ee928d0855e3724dfab0fe|runtime commit|Dusk runtime commit|repeatable cargo clippy|only adds evidence-doc updates|evidence-doc updates only|17 0}"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

usage() {
    cat <<EOF
Usage: bash scripts/github-review-hygiene.sh [options]

Exports reviewer-facing GitHub text and checks it for:
  - known stale SHA/run/comment wording patterns
  - explicit PR current-head claims against the live PR heads
  - stale "current/latest" evidence wording in active comments
  - historical status snapshots that are not marked superseded
  - source/artifact secret hygiene regressions

Options:
  --export-dir DIR       Write export files to DIR.
                         Default: /tmp/hyperlane-review-export-<timestamp>
  --no-keep              Remove the export directory on success.
  -h, --help             Show this help.

Environment:
  DUSK_REPO              Dusk contracts/tooling repo.
                         Default: $DUSK_REPO
  MONOREPO_REPO          Hyperlane monorepo fork.
                         Default: $MONOREPO_REPO
  DUSK_PRS               Space-separated Dusk PR numbers to export.
                         Default: $DUSK_PRS
  MONOREPO_PRS           Space-separated monorepo PR numbers to export.
                         Default: $MONOREPO_PRS
  DUSK_ISSUES            Space-separated Dusk issue numbers to export.
                         Default: $DUSK_ISSUES
  STALE_REVIEW_PATTERNS  Extended regex for stale review text.
                         Default: current repository stale-reference set.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --export-dir)
            EXPORT_DIR="${2:-}"
            [ -n "$EXPORT_DIR" ] || fail "--export-dir requires a path"
            shift 2
            ;;
        --no-keep)
            KEEP_EXPORT=0
            shift
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
command -v rg >/dev/null 2>&1 || fail "rg is required"
command -v jq >/dev/null 2>&1 || fail "jq is required"

if [ -z "$EXPORT_DIR" ]; then
    EXPORT_DIR="/tmp/hyperlane-review-export-$(date +%s)"
fi

rm -rf "$EXPORT_DIR"
mkdir -p "$EXPORT_DIR"

cleanup() {
    if [ "$KEEP_EXPORT" -eq 0 ]; then
        rm -rf "$EXPORT_DIR"
    fi
}
trap cleanup EXIT

export_pr() {
    local repo="$1"
    local number="$2"
    local prefix="$3"

    info "Exporting $repo PR #$number"
    gh api "repos/$repo/pulls/$number" --jq .body >"$EXPORT_DIR/$prefix-pr-$number-body.txt"
    gh api "repos/$repo/issues/$number/comments" --paginate \
        --jq '.[] | "COMMENT_ID=\(.id)\n" + .body + "\n---END---"' \
        >"$EXPORT_DIR/$prefix-pr-$number-comments.txt"
}

export_issue() {
    local repo="$1"
    local number="$2"
    local prefix="$3"

    info "Exporting $repo issue #$number"
    gh api "repos/$repo/issues/$number" --jq .body >"$EXPORT_DIR/$prefix-issue-$number-body.txt"
    gh api "repos/$repo/issues/$number/comments" --paginate \
        --jq '.[] | "COMMENT_ID=\(.id)\n" + .body + "\n---END---"' \
        >"$EXPORT_DIR/$prefix-issue-$number-comments.txt"
}

for pr in $DUSK_PRS; do
    export_pr "$DUSK_REPO" "$pr" "dusk"
done

for pr in $MONOREPO_PRS; do
    export_pr "$MONOREPO_REPO" "$pr" "monorepo"
done

for issue in $DUSK_ISSUES; do
    export_issue "$DUSK_REPO" "$issue" "dusk"
done

info "Exported review text to $EXPORT_DIR"

stale_hits="$EXPORT_DIR/stale-review-hits.txt"
if rg -n -e "$STALE_REVIEW_PATTERNS" "$EXPORT_DIR" >"$stale_hits"; then
    cat "$stale_hits" >&2
    fail "stale reviewer-facing text found"
fi
rm -f "$stale_hits"

check_pr_head_claims() {
    local label="$1"
    local repo="$2"
    local pr="$3"
    local patterns="$4"
    local live_head
    local claims
    local filtered
    local mismatches

    live_head="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha)"
    claims="$EXPORT_DIR/pr-head-claims-$pr.txt"
    filtered="$EXPORT_DIR/pr-head-filtered-$pr.txt"
    mismatches="$EXPORT_DIR/pr-head-mismatches-$pr.txt"

    for file in "$EXPORT_DIR"/*-body.txt; do
        printf 'FILE=%s\n' "$file" >>"$filtered"
        cat "$file" >>"$filtered"
        printf '\n---END---\n' >>"$filtered"
    done

    for file in "$EXPORT_DIR"/*-comments.txt; do
        awk -v file="$file" '
            function flush_comment() {
                if (id != "" && !superseded) {
                    print "FILE=" file ":COMMENT_ID=" id
                    printf "%s", body
                    print "\n---END---"
                }
            }

            /^COMMENT_ID=/ {
                flush_comment()
                id = substr($0, 12)
                line_no = 0
                superseded = 0
                body = ""
                next
            }

            /^---END---$/ {
                flush_comment()
                id = ""
                body = ""
                next
            }

            {
                if (id != "") {
                    line_no++
                    if (line_no <= 5 && $0 ~ /^Superseded historical status snapshot\./) {
                        superseded = 1
                    }
                    body = body $0 "\n"
                }
            }

            END {
                flush_comment()
            }
        ' "$file" >>"$filtered"
    done

    if rg -n -o -e "$patterns" "$filtered" >"$claims"; then
        while IFS= read -r claim; do
            claim_sha="$(printf '%s\n' "$claim" | rg -o '[0-9a-f]{40}' | head -n1)"
            if [ "$claim_sha" != "$live_head" ]; then
                printf '%s\n' "$claim" >>"$mismatches"
            fi
        done <"$claims"

        if [ -s "$mismatches" ]; then
            cat "$mismatches" >&2
            fail "stale $label current-head claim found; live head is $live_head"
        fi
    fi
    rm -f "$claims" "$filtered" "$mismatches"
}

check_pr_head_claims \
    "Dusk PR #1" \
    "$DUSK_REPO" \
    1 \
    "(Companion )?Dusk PR #1 head is now .?[0-9a-f]{40}|Dusk PR #1 current head: .?[0-9a-f]{40}|Dusk PR #1 head: .?[0-9a-f]{40}|Current Dusk PR #1 head:? (is )?.?[0-9a-f]{40}|Dusk PR #1: .?[0-9a-f]{40}|current Dusk head .?[0-9a-f]{40}"

check_pr_head_claims \
    "monorepo PR #1" \
    "$MONOREPO_REPO" \
    1 \
    "Monorepo PR #1 current head: .?[0-9a-f]{40}|Monorepo PR #1 head: .?[0-9a-f]{40}|Monorepo PR #1: .?[0-9a-f]{40}"

check_pr_head_claims \
    "workflow dispatcher PR #3" \
    "$DUSK_REPO" \
    3 \
    "Manual workflow dispatcher PR #3 head: .?[0-9a-f]{40}|Manual workflow dispatcher PR #3: .?[0-9a-f]{40}|Workflow dispatcher PR #3: .?[0-9a-f]{40}|Workflow PR #3 head: .?[0-9a-f]{40}"

active_review_text="$EXPORT_DIR/active-review-text.txt"
for file in "$EXPORT_DIR"/*-body.txt; do
    printf 'FILE=%s\n' "$file" >>"$active_review_text"
    cat "$file" >>"$active_review_text"
    printf '\n---END---\n' >>"$active_review_text"
done

for file in "$EXPORT_DIR"/*-comments.txt; do
    awk -v file="$file" '
        function flush_comment() {
            if (id != "" && !historical) {
                print "FILE=" file ":COMMENT_ID=" id
                printf "%s", body
                print "\n---END---"
            }
        }

        /^COMMENT_ID=/ {
            flush_comment()
            id = substr($0, 12)
            line_no = 0
            historical = 0
            body = ""
            next
        }

        /^---END---$/ {
            flush_comment()
            id = ""
            body = ""
            next
        }

        {
            if (id != "") {
                line_no++
                if (line_no <= 5 && $0 ~ /^(Superseded|Historical) /) {
                    historical = 1
                }
                body = body $0 "\n"
            }
        }

        END {
            flush_comment()
        }
    ' "$file" >>"$active_review_text"
done

stale_active_patterns='Current-head agent check refresh|Current monorepo agent-check evidence refresh|Latest clean-layout repro evidence for the tested Dusk commit|Latest local clean-layout repro run `1778582787`|Latest local clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427472641|Latest clean-layout repro evidence: `1778596710`|clean-layout repro run `1778596710` tested Dusk source ref `6ac9a2bc0c3cbdb1d9994335b23f06189d355f40`|Latest clean-layout repro evidence: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4431719578|current-head repro evidence|Final-head clean-layout repro evidence|Latest local final-head repro evidence'
stale_active_hits="$EXPORT_DIR/stale-active-review-wording.txt"
if rg -n -e "$stale_active_patterns" "$active_review_text" >"$stale_active_hits"; then
    cat "$stale_active_hits" >&2
    fail "stale current/latest wording found in active reviewer-facing text"
fi
rm -f "$active_review_text" "$stale_active_hits"

snapshot_markers="Current output after push|Cross-repo handoff refresh on 2026-05-12|Pushed two docs/workflow-only updates|Docs-only wording refresh pushed"
snapshot_mismatches="$EXPORT_DIR/unsuperseded-status-snapshots.txt"
for file in "$EXPORT_DIR"/*-comments.txt; do
    awk -v file="$file" -v out="$snapshot_mismatches" -v markers="$snapshot_markers" '
        function flush_comment() {
            if (id != "" && has_marker && !superseded) {
                print file ":COMMENT_ID=" id >> out
            }
        }

        /^COMMENT_ID=/ {
            flush_comment()
            id = substr($0, 12)
            line_no = 0
            has_marker = 0
            superseded = 0
            next
        }

        /^---END---$/ {
            flush_comment()
            id = ""
            next
        }

        {
            if (id != "") {
                line_no++
                if (line_no <= 5 && $0 ~ /^Superseded historical status snapshot\./) {
                    superseded = 1
                }
                if ($0 ~ markers) {
                    has_marker = 1
                }
            }
        }

        END {
            flush_comment()
        }
    ' "$file"
done

if [ -s "$snapshot_mismatches" ]; then
    cat "$snapshot_mismatches" >&2
    fail "historical status snapshot comments must be marked superseded"
fi
rm -f "$snapshot_mismatches"

bash scripts/secret-hygiene-check.sh "$EXPORT_DIR"

info "GitHub review hygiene checks passed"
echo "$EXPORT_DIR"
