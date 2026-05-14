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
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
DUSK_PRS="${DUSK_PRS:-1 3}"
MONOREPO_PRS="${MONOREPO_PRS:-1}"
DUSK_ISSUES="${DUSK_ISSUES:-2 4 5 6 7 8 9}"
EXPORT_DIR="${EXPORT_DIR:-}"
KEEP_EXPORT="${KEEP_EXPORT:-1}"
STALE_REVIEW_PATTERNS="${STALE_REVIEW_PATTERNS:-356239661c5121e79e81930fd2b18995bf375b11|78b0cfd19b59705c49639cd8200baa2404344d2c|c0036501b26cc98fb64259807e4cbca929487aec|d192269aedee625be4355a34a6ec5672c4141ec8|10721dcb645adc0d7fc8b4952362e57355c2c67e|bb2fbd685e1bccd0c468de9bbd3f2bb49bbac1d4|0cc3732046181576cd57a536bce585c079c776c2|519478c418498d3520aee650e76292ff2aad1e64|7a00aed51e1a39ec6fd58a5fb8added3a0350f37|e2d5db3f01bb2bc0ef5a0d4eda560cc34ee21862|0aca118ce9cae9ae86c7757f207f49d76face371|06dbf75e5f880ec74d29d59e44c4059298a49685|06dbf75e2d67b0bbc5aa450066bfb5743f79bdd2|1778574482|1778576530|1778577847|28d07e01d1bbc0cf59575a811cde55e844a2abb7|2dcc3409c38107caf2b1e67c913a265fba51df7e|d25d18155dd28ffdee30793b416e6956dd4c4799|25771131928|25771131956|25771131995|25771347654|25771347630|25771347609|75694299679|75694299650|75694300152|75694938855|75694938818|75694949161|25802657899|25802657910|75796678440|75796678527|54e56fa139343df5250ac01b1589ae7052cbedeb|ahead/behind .31 0|ahead/behind .32 0|Dusk PR #1 head is now .889a00bb11d589d268ee928d0855e3724dfab0fe|Companion Dusk PR #1 head is now .889a00bb11d589d268ee928d0855e3724dfab0fe|runtime commit|Dusk runtime commit|repeatable cargo clippy|only adds evidence-doc updates|evidence-doc updates only|17 0}"
REVIEWER_ROUTING_URL="${REVIEWER_ROUTING_URL:-https://github.com/dusk-network/hyperlane-dusk/blob/feat/dusk-hardening-v2/REVIEWERS.md}"
LATEST_REPRO_ARCHIVE_PATH="${LATEST_REPRO_ARCHIVE_PATH:-/home/hein_/projects/hyperlane/.codex-backups/hyperlane-clean-repro-current-head-1778750702.tgz}"
LATEST_REPRO_ARCHIVE_SHA256="${LATEST_REPRO_ARCHIVE_SHA256:-090ed23b59f4a29d5f498fea9019c55164180876ce37a94729128d3ad389a5eb}"
AGENT_PLACEHOLDER_PATTERN="${AGENT_PLACEHOLDER_PATTERN:-todo!|unimplemented!|panic!|expect\(}"
AGENT_PLACEHOLDER_SCAN_ONLY=0
DISPATCHER_COMMENT_SCAN_ONLY_FILE=""
STALE_DISPATCHER_REPRO_PATTERN='https://github\.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895|https://github\.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443963744'

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

git_grep_to_file() {
    out_file="$1"
    label="$2"
    repo="$3"
    shift 3

    set +e
    git -C "$repo" grep "$@" >"$out_file"
    grep_status=$?
    set -e

    if [ "$grep_status" -eq 0 ]; then
        return 0
    fi
    if [ "$grep_status" -eq 1 ]; then
        return 1
    fi
    cat "$out_file" >&2
    fail "$label scan failed"
}

usage() {
    cat <<EOF
Usage: bash scripts/github-review-hygiene.sh [options]

Exports reviewer-facing GitHub text and checks it for:
  - known stale SHA/run/comment wording patterns
  - explicit PR current-head claims against the live PR heads
  - stale "current/latest" evidence wording in active comments
  - JSON-escaped PR/issue/comment body rendering
  - historical status snapshots that are not marked superseded
  - missing reviewer-facing links and review-gate handoff text
  - stale make review-gates descriptions that omit archive/dispatcher coverage
  - stale monorepo queued-check status after inherited Depot workflow guards
  - source/artifact secret hygiene regressions

Options:
  --export-dir DIR       Write export files to DIR.
                         Default: /tmp/hyperlane-review-export-<timestamp>
  --no-keep              Remove the export directory on success.
  --agent-placeholder-scan-only
                         Run only the local Dusk agent runtime
                         panic/placeholder scan.
  --dispatcher-comment-scan-only FILE
                         Run only the workflow PR #3 stale dispatcher
                         evidence-link scan against an exported comments file.
  -h, --help             Show this help.

Environment:
  DUSK_REPO              Dusk contracts/tooling repo.
                         Default: $DUSK_REPO
  MONOREPO_REPO          Hyperlane monorepo fork.
                         Default: $MONOREPO_REPO
  MONOREPO_DIR           Local Hyperlane monorepo checkout for Dusk agent
                         runtime panic/placeholder scanning.
                         Default: $MONOREPO_DIR
  DUSK_PRS               Space-separated Dusk PR numbers to export.
                         Default: $DUSK_PRS
  MONOREPO_PRS           Space-separated monorepo PR numbers to export.
                         Default: $MONOREPO_PRS
  DUSK_ISSUES            Space-separated Dusk issue numbers to export.
                         Default: $DUSK_ISSUES
  STALE_REVIEW_PATTERNS  Extended regex for stale review text.
                         Default: current repository stale-reference set.
  AGENT_PLACEHOLDER_PATTERN
                         Extended regex for Dusk agent runtime
                         panic/placeholder scans.
                         Default: $AGENT_PLACEHOLDER_PATTERN
  REVIEWER_ROUTING_URL   Advisory reviewer routing URL expected in active
                         PR/issue bodies.
                         Default: $REVIEWER_ROUTING_URL
  LATEST_REPRO_ARCHIVE_PATH
                         Durable latest-repro archive path expected in active
                         reviewer-facing text.
                         Default: $LATEST_REPRO_ARCHIVE_PATH
  LATEST_REPRO_ARCHIVE_SHA256
                         Durable latest-repro archive hash expected in active
                         reviewer-facing text.
                         Default: $LATEST_REPRO_ARCHIVE_SHA256
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
        --agent-placeholder-scan-only)
            AGENT_PLACEHOLDER_SCAN_ONLY=1
            shift
            ;;
        --dispatcher-comment-scan-only)
            DISPATCHER_COMMENT_SCAN_ONLY_FILE="${2:-}"
            [ -n "$DISPATCHER_COMMENT_SCAN_ONLY_FILE" ] || fail "--dispatcher-comment-scan-only requires a path"
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

command -v git >/dev/null 2>&1 || fail "git is required"
command -v rg >/dev/null 2>&1 || fail "rg is required"

check_agent_placeholder_scan() {
    info "Checking local Dusk agent runtime panic/placeholder paths"
    [ -d "$MONOREPO_DIR/.git" ] || fail "missing local monorepo checkout at $MONOREPO_DIR"
    agent_hits="${EXPORT_DIR:-/tmp}/dusk-agent-panic-placeholder-hits.txt"
    if git_grep_to_file "$agent_hits" "Dusk agent runtime panic/placeholder" "$MONOREPO_DIR" -n -E "$AGENT_PLACEHOLDER_PATTERN" -- rust/main/chains/hyperlane-dusk/src; then
        cat "$agent_hits" >&2
        fail "Dusk agent runtime panic/placeholder path found"
    fi
    rm -f "$agent_hits"
}

if [ "$AGENT_PLACEHOLDER_SCAN_ONLY" -eq 1 ]; then
    check_agent_placeholder_scan
    exit 0
fi

check_stale_dispatcher_comments() {
    local comments_file="$1"
    local hits_file="$2"

    [ -f "$comments_file" ] || fail "missing dispatcher comments file: $comments_file"
    if rg -n -e "$STALE_DISPATCHER_REPRO_PATTERN" "$comments_file" >"$hits_file"; then
        cat "$hits_file" >&2
        fail "$comments_file contains stale dispatcher clean-layout repro evidence"
    fi
}

if [ -n "$DISPATCHER_COMMENT_SCAN_ONLY_FILE" ]; then
    check_stale_dispatcher_comments "$DISPATCHER_COMMENT_SCAN_ONLY_FILE" "${EXPORT_DIR:-/tmp}/stale-pr-3-comments.txt"
    exit 0
fi

command -v gh >/dev/null 2>&1 || fail "gh is required"
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

json_escaped_hits="$EXPORT_DIR/json-escaped-review-text.txt"
if rg_to_file "$json_escaped_hits" "JSON-escaped review text" -n '^".*\\n' "$EXPORT_DIR"; then
    cat "$json_escaped_hits" >&2
    fail "JSON-escaped reviewer-facing text found; update bodies/comments with raw Markdown"
fi
rm -f "$json_escaped_hits"

stale_hits="$EXPORT_DIR/stale-review-hits.txt"
if rg_to_file "$stale_hits" "stale review text" -n -e "$STALE_REVIEW_PATTERNS" "$EXPORT_DIR"; then
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
                    if (line_no <= 5 && $0 ~ /^(Superseded|Historical) /) {
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

    if rg_to_file "$claims" "$label current-head claim" -n -o -e "$patterns" "$filtered"; then
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
    "(Companion )?Dusk PR #1 head is now .?[0-9a-f]{40}|Dusk PR #1 live head is .?[0-9a-f]{40}|Dusk PR #1 current head: .?[0-9a-f]{40}|Dusk PR #1 head: .?[0-9a-f]{40}|Dusk PR #1 head .?[0-9a-f]{40}|Current Dusk PR #1 head:? (is )?.?[0-9a-f]{40}|Dusk PR #1: .?[0-9a-f]{40}|current Dusk head .?[0-9a-f]{40}"

check_pr_head_claims \
    "monorepo PR #1" \
    "$MONOREPO_REPO" \
    1 \
    "Monorepo PR #1 live head is .?[0-9a-f]{40}|Monorepo PR #1 current head: .?[0-9a-f]{40}|Monorepo PR #1 head: .?[0-9a-f]{40}|Monorepo PR #1: .?[0-9a-f]{40}"

check_pr_head_claims \
    "workflow dispatcher PR #3" \
    "$DUSK_REPO" \
    3 \
    "Workflow dispatcher PR #3 live head is .?[0-9a-f]{40}|Manual workflow dispatcher PR #3 head: .?[0-9a-f]{40}|Manual workflow dispatcher PR #3: .?[0-9a-f]{40}|Workflow dispatcher PR #3: .?[0-9a-f]{40}|Workflow PR #3 head: .?[0-9a-f]{40}"

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

stale_active_patterns='Current-head agent check refresh|Current monorepo agent-check evidence refresh|Current latest clean-layout repro evidence|Latest clean-layout repro evidence for the tested Dusk commit|Latest local clean-layout repro run `1778582787`|Latest local clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427472641|Latest clean-layout repro evidence: `1778596710`|clean-layout repro run `1778596710` tested Dusk source ref `6ac9a2bc0c3cbdb1d9994335b23f06189d355f40`|Latest clean-layout repro evidence: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4431719578|Latest clean-layout repro evidence: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432215201|Latest local clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4431719578|Latest local clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432215201|Use the latest clean-layout repro evidence for the current handoff instead: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4431719578|Use the latest clean-layout repro evidence for the current handoff instead: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432215201|`make repro-check-agent` latest clean-layout repro evidence, including the `make clippy-contracts` wrapper step: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4431719578|`make repro-check-agent` latest clean-layout repro evidence, including the `make clippy-contracts` wrapper step: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432215201|Companion Dusk PR head after evidence-doc refresh: `6ac9a2bc0c3cbdb1d9994335b23f06189d355f40`|New monorepo PR head: `a2db5731e385634268071d39b0554883d11d8ac5`|latest post-rebase monorepo head `a2db5731e385634268071d39b0554883d11d8ac5`|Hyperlane upstream `main` advanced to `66e8c1f4644cea0392b33007225e6611b8f06804`|upstream drift `25 0` against `66e8c1f4644cea0392b33007225e6611b8f06804`|Upstream drift: `25 0`; merge-base equals upstream `66e8c1f4644cea0392b33007225e6611b8f06804`|current-head repro evidence|Final-head clean-layout repro evidence|Latest local final-head repro evidence|Clean-layout repro evidence for Dusk tested source ref `de9b7fa`: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432879665|clean-layout repro evidence for Dusk tested source ref `de9b7fa` at https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432879665|Current local clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432879665|Current clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432879665|Use the latest clean-layout repro evidence for the current handoff instead: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432879665|latest clean-layout repro tested Dusk source ref: `aa278208b2c2b5f4abc38c32ec792295080014a9`|current evidence points to https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4432879665|companion Dusk repo has no owner-routing file|patched-floor comparison|first-patched-floor comparison|patchedFloorSatisfied|missingPatchedFloor|zero status checks|no status checks configured|Neither repo is currently enforcing required status checks|CI/status checks, branch protection|CI/default-branch workflow setup, branch protection/status checks|missing repo-level runner/secret visibility|Current local clean-layout repro evidence is https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433179148|Manual repro dispatcher gate` passed on PR #3: https://github.com/dusk-network/hyperlane-dusk/actions/runs/25767591188/job/75683510054|QUEUED:=52|52 queued inherited|86 checks: one expected'
stale_active_patterns="$stale_active_patterns|87f7ab90d2a15e9b86b1f59eec0ffacb1b68f3c0|25810152097|25810152068|75823888731|75823887863|25801754973|25801754962|75793438590|75793438604|COMPLETED:SUCCESS=17|17 completed successes|COMPLETED:SUCCESS=14|14 completed successes"
stale_active_patterns="$stale_active_patterns|Rebased the monorepo branch onto upstream Hyperlane \`2b7db706023806b36a57e446205ae443537ae9ec\`|Rebased this branch onto upstream Hyperlane \`2b7db706023806b36a57e446205ae443537ae9ec\`|Monorepo branch is now rebased onto upstream Hyperlane \`2b7db706023806b36a57e446205ae443537ae9ec\`"
stale_active_patterns="$stale_active_patterns|narrow default-branch dispatcher PR for the manual repro workflow|narrow dispatcher PR containing only|Opened dusk-network/hyperlane-dusk#3 as a narrow dispatcher|contains only.*manual-repro-check\\.yml.*actionlint\\.yaml|25767765098|25767765114|75684050585|75684050777|25818138725|25818138768|75851960627|75851960495|25797921067|75779855203"
stale_active_patterns="$stale_active_patterns|8b15eb607e83b80cc334c6402e6c752dcfc9a1ba|25817675933|25817675973|75850331585|75850331611|aheadBehind.*43 0"
stale_active_patterns="$stale_active_patterns|This refresh is aligned with Dusk \`[0-9a-f]{40}\`|Dusk PR #1 live head: \`[0-9a-f]{40}\`|Latest Dusk PR #1 review policy pass: https://github\\.com/dusk-network/hyperlane-dusk/actions/runs/[0-9]+/job/[0-9]+|Latest Dusk PR #1 production-readiness expected failure: https://github\\.com/dusk-network/hyperlane-dusk/actions/runs/[0-9]+/job/[0-9]+|Dusk PR #1: \`Dusk review policy gate\` passed on head \`[0-9a-f]{7,40}\`"
stale_active_patterns="$stale_active_patterns|Latest monorepo \`Dusk review policy gate\` passed: https://github\\.com/dusk-network/hyperlane-monorepo/actions/runs/[0-9]+/job/[0-9]+|Latest \`Dusk agent cargo check\` failed.*https://github\\.com/dusk-network/hyperlane-monorepo/actions/runs/[0-9]+/job/[0-9]+|25816561587|25816561525|25816561571|75846502733|75846502758|75846503474"
stale_active_hits="$EXPORT_DIR/stale-active-review-wording.txt"
if rg_to_file "$stale_active_hits" "stale active review wording" -n -e "$stale_active_patterns" "$active_review_text"; then
    cat "$stale_active_hits" >&2
    fail "stale current/latest wording found in active reviewer-facing text"
fi
rm -f "$stale_active_hits"

stale_review_gates_description='make review-gates` runs the lightweight non-E2E gate bundle: preservation audit, Dependabot vulnerable-range comparison, GitHub review-hygiene export/scan, and fresh live gate status'
stale_review_gates_hits="$EXPORT_DIR/stale-review-gates-description.txt"
if rg_to_file "$stale_review_gates_hits" "stale review-gates description" -n -F "$stale_review_gates_description" "$active_review_text"; then
    cat "$stale_review_gates_hits" >&2
    fail "stale make review-gates description found; archive hygiene coverage is missing"
fi
rm -f "$stale_review_gates_hits"

rg -q -F 'make dispatcher-merge-order-smoke' "$active_review_text" \
    || fail "active reviewer-facing text is missing dispatcher merge-order smoke handoff text"
rg -q -F 'monorepoCoveredPathDelta' "$active_review_text" \
    || fail "active reviewer-facing text is missing monorepo clean-layout repro delta handoff text"
rg -q -F "$LATEST_REPRO_ARCHIVE_PATH" "$active_review_text" \
    || fail "active reviewer-facing text is missing latest durable repro archive path"
rg -q -F "$LATEST_REPRO_ARCHIVE_SHA256" "$active_review_text" \
    || fail "active reviewer-facing text is missing latest durable repro archive hash"

post_rebase_e2e_comment='https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433528683'
post_rebase_e2e_archive_comment='https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433564278'
dependency_remediated_e2e_comment='https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389'
latest_repro_comment='https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449464522'
current_gate_refresh_text='head and gate refresh:'
ci_provisioning_runbook='https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4435830841'
for file in \
    "$EXPORT_DIR/dusk-pr-1-body.txt" \
    "$EXPORT_DIR/monorepo-pr-1-body.txt" \
    "$EXPORT_DIR/dusk-issue-2-body.txt"; do
    rg -q -F "$current_gate_refresh_text" "$file" \
        || fail "$file is missing current gate refresh handoff text"
    rg -q -F "$latest_repro_comment" "$file" \
        || fail "$file is missing latest clean-layout repro evidence link"
    rg -q -F '1778750702' "$file" \
        || fail "$file is missing latest clean-layout repro run id"
    rg -q -F 'ae215d061d22924dfeea64e90b4cb4803604aac4' "$file" \
        || fail "$file is missing latest clean-layout Dusk source ref"
    rg -q -F 'c0c64db4659500d077bb253ad13acba0e347d3fc' "$file" \
        || fail "$file is missing latest clean-layout clean-Rusk ref"
    if rg -n -F 'Latest checkout-v6 clean-layout' "$file" >"$EXPORT_DIR/stale-repro-body.txt"; then
        cat "$EXPORT_DIR/stale-repro-body.txt" >&2
        fail "$file contains stale latest checkout-v6 wording"
    fi
    if rg -n -e '1778607202|4433179148|836ee7d8d8e95152b3daaeebbc3fb56b0cc8e253|1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3|1778683232|9050143c1ef12f76d117ee97effa79da8df3e334' "$file" >"$EXPORT_DIR/stale-repro-body.txt"; then
        cat "$EXPORT_DIR/stale-repro-body.txt" >&2
        fail "$file contains stale clean-layout repro body evidence"
    fi
    rg -q -F "$post_rebase_e2e_comment" "$file" \
        || fail "$file is missing post-rebase E2E evidence link"
    rg -q -F "$post_rebase_e2e_archive_comment" "$file" \
        || fail "$file is missing post-rebase E2E archive link"
    rg -q -F "$dependency_remediated_e2e_comment" "$file" \
        || fail "$file is missing dependency-remediated E2E evidence link"
    rg -q -F "$REVIEWER_ROUTING_URL" "$file" \
        || fail "$file is missing advisory reviewer routing link"
    rg -q -F 'make gate-status-fresh' "$file" \
        || fail "$file is missing make gate-status-fresh handoff text"
    rg -q -F 'make dependency-alert-status' "$file" \
        || fail "$file is missing make dependency-alert-status handoff text"
    rg -q -F 'make completion-audit-status' "$file" \
        || fail "$file is missing make completion-audit-status handoff text"
    rg -q -F 'make review-gates' "$file" \
        || fail "$file is missing make review-gates handoff text"
    rg -q -F 'archive hygiene self-tests' "$file" \
        || fail "$file is missing make review-gates archive hygiene self-test handoff text"
    rg -q -F 'extracted evidence archive hygiene scans' "$file" \
        || fail "$file is missing make review-gates extracted archive hygiene handoff text"
    rg -q -F 'make production-readiness-guard' "$file" \
        || fail "$file is missing make production-readiness-guard handoff text"
    rg -q -F 'required status-check policy enabled' "$file" \
        || fail "$file is missing required status-check policy handoff text"
    rg -q -F 'latest clean-layout repro path delta' "$file" \
        || fail "$file is missing latest clean-layout repro path delta handoff text"
done
rm -f "$EXPORT_DIR/stale-repro-body.txt"

if [ -f "$EXPORT_DIR/dusk-issue-2-body.txt" ]; then
    rg -q -F 'Latest clean-layout repro run `1778750702` tested Dusk `ae215d061d22924dfeea64e90b4cb4803604aac4` and monorepo `99f718dfca395076285d3cc1226128599d67148c`' \
        "$EXPORT_DIR/dusk-issue-2-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-2-body.txt is missing current monorepo clean-layout repro handoff text"
fi

for file in \
    "$EXPORT_DIR/dusk-pr-1-body.txt" \
    "$EXPORT_DIR/dusk-issue-2-body.txt" \
    "$EXPORT_DIR/dusk-issue-8-body.txt"; do
    if [ -f "$file" ]; then
        rg -q -F 'actionlint .github/workflows/manual-repro-check.yml .github/workflows/manual-repro-dispatcher-gate.yml .github/workflows/dusk-review-policy-gate.yml' "$file" \
            || fail "$file is missing full dispatcher workflow actionlint handoff text"
        rg -q -F 'DUSK_STATUS_READ_TOKEN' "$file" \
            || fail "$file is missing DUSK_STATUS_READ_TOKEN status-visibility handoff text"
        rg -q -F 'repoStatusSecretVisible' "$file" \
            || fail "$file is missing repoStatusSecretVisible gate-status handoff text"
    fi
done

if [ -f "$EXPORT_DIR/dusk-pr-3-body.txt" ]; then
    rg -q -F "$REVIEWER_ROUTING_URL" "$EXPORT_DIR/dusk-pr-3-body.txt" \
        || fail "$EXPORT_DIR/dusk-pr-3-body.txt is missing advisory reviewer routing link"
    rg -q -F 'https://github.com/dusk-network/hyperlane-dusk/actions/runs/25818963712/job/75854858513' \
        "$EXPORT_DIR/dusk-pr-3-body.txt" \
        || fail "$EXPORT_DIR/dusk-pr-3-body.txt is missing current dispatcher gate run link"
    rg -q -F 'https://github.com/dusk-network/hyperlane-dusk/actions/runs/25818963747/job/75854858574' \
        "$EXPORT_DIR/dusk-pr-3-body.txt" \
        || fail "$EXPORT_DIR/dusk-pr-3-body.txt is missing current review policy gate run link"
    rg -q -F '.github/workflows/dusk-review-policy-gate.yml' \
        "$EXPORT_DIR/dusk-pr-3-body.txt" \
        || fail "$EXPORT_DIR/dusk-pr-3-body.txt is missing shared review-policy workflow scope text"
    rg -q -F "$ci_provisioning_runbook" "$EXPORT_DIR/dusk-pr-3-body.txt" \
        || fail "$EXPORT_DIR/dusk-pr-3-body.txt is missing CI provisioning runbook link"
fi

if [ -f "$EXPORT_DIR/dusk-pr-3-comments.txt" ]; then
    rg -q -F "$latest_repro_comment" "$EXPORT_DIR/dusk-pr-3-comments.txt" \
        || fail "$EXPORT_DIR/dusk-pr-3-comments.txt is missing latest clean-layout repro evidence link"
    check_stale_dispatcher_comments "$EXPORT_DIR/dusk-pr-3-comments.txt" "$EXPORT_DIR/stale-pr-3-comments.txt"
fi
rm -f "$EXPORT_DIR/stale-pr-3-comments.txt"

if [ -f "$EXPORT_DIR/dusk-issue-8-body.txt" ]; then
    rg -q -F "$latest_repro_comment" "$EXPORT_DIR/dusk-issue-8-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-8-body.txt is missing latest clean-layout repro evidence link"
    rg -q -F 'ae215d061d22924dfeea64e90b4cb4803604aac4' "$EXPORT_DIR/dusk-issue-8-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-8-body.txt is missing latest clean-layout Dusk source ref"
    rg -q -F 'c0c64db4659500d077bb253ad13acba0e347d3fc' "$EXPORT_DIR/dusk-issue-8-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-8-body.txt is missing latest clean-layout clean-Rusk ref"
    rg -q -F 'required status-check policy enabled' "$EXPORT_DIR/dusk-issue-8-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-8-body.txt is missing required status-check policy text"
    if rg -n -e '4433179148|836ee7d8d8e95152b3daaeebbc3fb56b0cc8e253' \
        "$EXPORT_DIR/dusk-issue-8-body.txt" >"$EXPORT_DIR/stale-issue-8-body.txt"; then
        cat "$EXPORT_DIR/stale-issue-8-body.txt" >&2
        fail "$EXPORT_DIR/dusk-issue-8-body.txt contains stale clean-layout repro body evidence"
    fi
    if rg -n -F 'That run tested Dusk source ref `ef8ee43cd99569299b9744b498ac1bbac69950bc`' \
        "$EXPORT_DIR/dusk-issue-8-body.txt" >"$EXPORT_DIR/stale-issue-8-body.txt"; then
        cat "$EXPORT_DIR/stale-issue-8-body.txt" >&2
        fail "$EXPORT_DIR/dusk-issue-8-body.txt contains stale latest-repro tested-ref text"
    fi
fi
rm -f "$EXPORT_DIR/stale-issue-8-body.txt"

if [ -f "$EXPORT_DIR/dusk-issue-7-body.txt" ]; then
    rg -q -F "$latest_repro_comment" "$EXPORT_DIR/dusk-issue-7-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-7-body.txt is missing latest clean-layout repro evidence link"
    rg -q -F "$dependency_remediated_e2e_comment" "$EXPORT_DIR/dusk-issue-7-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-7-body.txt is missing dependency-remediated E2E evidence link"
    rg -q -F 'PRODUCTION_SIGNER_POLICY.md' "$EXPORT_DIR/dusk-issue-7-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-7-body.txt is missing production signer policy link text"
    if rg -n -e '4430201984|4430343619' \
        "$EXPORT_DIR/dusk-issue-7-body.txt" >"$EXPORT_DIR/stale-issue-7-body.txt"; then
        cat "$EXPORT_DIR/stale-issue-7-body.txt" >&2
        fail "$EXPORT_DIR/dusk-issue-7-body.txt contains stale signer custody evidence links"
    fi
fi
rm -f "$EXPORT_DIR/stale-issue-7-body.txt"

for issue in 4 5 6; do
    file="$EXPORT_DIR/dusk-issue-$issue-body.txt"
    if [ -f "$file" ]; then
        rg -q -F "$latest_repro_comment" "$file" \
            || fail "$file is missing latest clean-layout repro evidence link"
        rg -q -F 'SECURITY_REVIEW.md' "$file" \
            || fail "$file is missing security review link text"
        rg -q -F 'PRODUCTION_REVIEW_DECISIONS.md' "$file" \
            || fail "$file is missing production decision record link text"
    fi
done

for issue in 4 5 6 7 8 9; do
    file="$EXPORT_DIR/dusk-issue-$issue-body.txt"
    if [ -f "$file" ]; then
        rg -q -F 'Current gate handoff:' "$file" \
            || fail "$file is missing current gate handoff section"
        rg -q -F 'currentGateRefreshHandoff' "$file" \
            || fail "$file is missing currentGateRefreshHandoff text"
        rg -q -F 'make review-gates' "$file" \
            || fail "$file is missing make review-gates handoff text"
        rg -q -F 'make production-readiness-guard' "$file" \
            || fail "$file is missing production-readiness guard handoff text"
    fi
done

if [ -f "$EXPORT_DIR/dusk-issue-9-body.txt" ]; then
    rg -q -F '1778541618' "$EXPORT_DIR/dusk-issue-9-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-9-body.txt is missing soak run id"
    rg -q -F '7282 seconds' "$EXPORT_DIR/dusk-issue-9-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-9-body.txt is missing soak duration"
    rg -q -F '280 total completed transfers' "$EXPORT_DIR/dusk-issue-9-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-9-body.txt is missing soak transfer count"
    rg -q -F 'c0c64db4659500d077bb253ad13acba0e347d3fc' "$EXPORT_DIR/dusk-issue-9-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-9-body.txt is missing clean Rusk ref"
    rg -q -F 'https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430670295' \
        "$EXPORT_DIR/dusk-issue-9-body.txt" \
        || fail "$EXPORT_DIR/dusk-issue-9-body.txt is missing soak archive evidence link"
fi

if rg -n \
    -e 'clean-Rusk E2E evidence remains recorded for pre-rebase monorepo' \
    -e 'E2E was not rerun for the docs-only post-rebase head' \
    "$active_review_text" >"$EXPORT_DIR/pre-rebase-only-e2e-wording.txt"; then
    cat "$EXPORT_DIR/pre-rebase-only-e2e-wording.txt" >&2
    fail "active reviewer-facing text still implies E2E is pre-rebase only"
fi
rm -f "$active_review_text" "$EXPORT_DIR/pre-rebase-only-e2e-wording.txt"

snapshot_markers="Current output after push|Cross-repo handoff refresh on 2026-05-12|Pushed two docs/workflow-only updates|Docs-only wording refresh pushed|still have no GitHub-enforced required status checks"
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

check_agent_placeholder_scan

bash scripts/secret-hygiene-check.sh "$EXPORT_DIR"

info "GitHub review hygiene checks passed"
echo "$EXPORT_DIR"
