#!/usr/bin/env bash
# Export GitHub review surfaces and scan them for stale evidence text.
#
# This is a read-only guardrail for PR/issue bodies and comments. It catches
# stale SHA/run references in reviewer-facing text and then reuses the secret
# hygiene scanner over the exported files.

set -euo pipefail

ROOT="${ROOT_OVERRIDE:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$ROOT"

DUSK_REPO="${DUSK_REPO:-dusk-network/hyperlane-dusk}"
MONOREPO_REPO="${MONOREPO_REPO:-dusk-network/hyperlane-monorepo}"
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
DUSK_PRS="${DUSK_PRS:-1 3 10}"
MONOREPO_PRS="${MONOREPO_PRS:-1}"
DUSK_ISSUES="${DUSK_ISSUES:-2 4 5 7 8 9}"
EXPORT_DIR="${EXPORT_DIR:-}"
KEEP_EXPORT="${KEEP_EXPORT:-1}"
STALE_REVIEW_PATTERNS="${STALE_REVIEW_PATTERNS:-356239661c5121e79e81930fd2b18995bf375b11|78b0cfd19b59705c49639cd8200baa2404344d2c|c0036501b26cc98fb64259807e4cbca929487aec|d192269aedee625be4355a34a6ec5672c4141ec8|10721dcb645adc0d7fc8b4952362e57355c2c67e|bb2fbd685e1bccd0c468de9bbd3f2bb49bbac1d4|0cc3732046181576cd57a536bce585c079c776c2|519478c418498d3520aee650e76292ff2aad1e64|7a00aed51e1a39ec6fd58a5fb8added3a0350f37|e2d5db3f01bb2bc0ef5a0d4eda560cc34ee21862|0aca118ce9cae9ae86c7757f207f49d76face371|06dbf75e5f880ec74d29d59e44c4059298a49685|06dbf75e2d67b0bbc5aa450066bfb5743f79bdd2|1778574482|1778576530|1778577847|28d07e01d1bbc0cf59575a811cde55e844a2abb7|2dcc3409c38107caf2b1e67c913a265fba51df7e|d25d18155dd28ffdee30793b416e6956dd4c4799|25771131928|25771131956|25771131995|25771347654|25771347630|25771347609|75694299679|75694299650|75694300152|75694938855|75694938818|75694949161|25802657899|25802657910|75796678440|75796678527|54e56fa139343df5250ac01b1589ae7052cbedeb|ahead/behind .31 0|ahead/behind .32 0|Dusk PR #1 head is now .889a00bb11d589d268ee928d0855e3724dfab0fe|Companion Dusk PR #1 head is now .889a00bb11d589d268ee928d0855e3724dfab0fe|runtime commit|Dusk runtime commit|repeatable cargo clippy|only adds evidence-doc updates|evidence-doc updates only|17 0}"
REVIEWER_ROUTING_URL="${REVIEWER_ROUTING_URL:-https://github.com/dusk-network/hyperlane-dusk/blob/feat/dusk-hardening-v2/REVIEWERS.md}"
CURRENT_BASE_CODE="${CURRENT_BASE_CODE:-876848ecc6c671995fad3ae7b22843e68a3ce8ca}"
CURRENT_STACK_CODE="${CURRENT_STACK_CODE:-b28d575527421d2a67245921ce561c88f554c099}"
CURRENT_MONOREPO_RUNTIME="${CURRENT_MONOREPO_RUNTIME:-e95d3ea282a55ead114471ffb1dece77706ffc81}"
CURRENT_MONOREPO_COVERED_PIN="${CURRENT_MONOREPO_COVERED_PIN:-833b77b4436e146a4776a3b35db68525014b3adb}"
CURRENT_MONOREPO_POLICY="${CURRENT_MONOREPO_POLICY:-c35f86405cf8cd83927860aca8b5c38b042ee198}"
CURRENT_HYPERLANE_UPSTREAM_BASE="${CURRENT_HYPERLANE_UPSTREAM_BASE:-67933966ed9c6f9e3d5ec095372e11414c82e4e7}"
CURRENT_RUSK_REF="${CURRENT_RUSK_REF:-5c6a0bab11c61fb4c81275afdeceb97fb942d85e}"
CURRENT_BASE_REPRO_SHA256="${CURRENT_BASE_REPRO_SHA256:-b4d3864dfb178adc283e8a3cc6f137c4c9580525b4bd1ffb07d7ef9a0bdbdedd}"
CURRENT_STACK_REPRO_SHA256="${CURRENT_STACK_REPRO_SHA256:-314ff8b12204be6dcf9055ce9917013d47e6c93d4adfca88b6e53c55e0434ec6}"
CURRENT_TESTMOCK_RUN="${CURRENT_TESTMOCK_RUN:-1784629402}"
CURRENT_TESTMOCK_SHA256="${CURRENT_TESTMOCK_SHA256:-c155747f8d49beb16e8cf005c3bca77eff62b3d3fe0c3e86fa4737a8ca3b0540}"
CURRENT_MULTISIG_RUN="${CURRENT_MULTISIG_RUN:-1784628130}"
CURRENT_MULTISIG_SHA256="${CURRENT_MULTISIG_SHA256:-d6d9100b3f306662000d5d865d849f492bf1810f88c245a53fda998843898df6}"
AGENT_PLACEHOLDER_PATTERN="${AGENT_PLACEHOLDER_PATTERN:-todo!|unimplemented!|panic!|expect\(}"
AGENT_PLACEHOLDER_SCAN_ONLY=0
DISPATCHER_COMMENT_SCAN_ONLY_FILE=""
STALE_DISPATCHER_REPRO_PATTERN="${STALE_DISPATCHER_REPRO_PATTERN:-https://github\.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895|https://github\.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443963744}"
EXPORT_DIR_CREATED=0

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

rg_to_file() {
    local out_file="$1"
    local label="$2"
    local rg_status
    shift 2

    : >"$out_file" || fail "cannot create $label scan output: $out_file"
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
    cat "$out_file" >&2 2>/dev/null || true
    fail "$label scan failed"
}

git_grep_to_file() {
    out_file="$1"
    label="$2"
    repo="$3"
    shift 3

    : >"$out_file" || fail "cannot create $label scan output: $out_file"
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
  - missing exact immutable validation anchors and decision links
  - missing explicit separation of pre-merge checks from production authority
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
  REVIEWER_ROUTING_URL   Advisory reviewer routing URL expected in the
                         workflow dispatcher PR body.
                         Default: $REVIEWER_ROUTING_URL
  CURRENT_*              Immutable source, runtime, Rusk, repro, and E2E
                         anchors expected on the relevant review surfaces.
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
    git -C "$MONOREPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
        || fail "missing local monorepo checkout at $MONOREPO_DIR"
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
    if rg_to_file "$hits_file" "stale dispatcher comments" \
        -n -e "$STALE_DISPATCHER_REPRO_PATTERN" "$comments_file"; then
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
    EXPORT_DIR="$(mktemp -d -t hyperlane-review-export.XXXXXX)"
    EXPORT_DIR_CREATED=1
else
    if [ -e "$EXPORT_DIR" ]; then
        fail "--export-dir must name a path that does not already exist: $EXPORT_DIR"
    fi
    mkdir -- "$EXPORT_DIR" || fail "cannot create export directory: $EXPORT_DIR"
    EXPORT_DIR_CREATED=1
fi

cleanup() {
    if [ "$KEEP_EXPORT" -eq 0 ] && [ "$EXPORT_DIR_CREATED" -eq 1 ]; then
        rm -rf -- "$EXPORT_DIR"
        EXPORT_DIR_CREATED=0
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

check_pr_head_claims \
    "Dusk withdrawal PR #10" \
    "$DUSK_REPO" \
    10 \
    "Stacked withdrawal PR #10 head: .?[0-9a-f]{40}|Withdrawal PR #10 head: .?[0-9a-f]{40}|PR #10 head: .?[0-9a-f]{40}"

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

require_literal() {
    local file="$1"
    local value="$2"
    local label="$3"

    [ -f "$file" ] || fail "missing exported review surface: $file"
    rg -q -F "$value" "$file" || fail "$file is missing $label: $value"
}

base_pr_body="$EXPORT_DIR/dusk-pr-1-body.txt"
stack_pr_body="$EXPORT_DIR/dusk-pr-10-body.txt"
monorepo_pr_body="$EXPORT_DIR/monorepo-pr-1-body.txt"
issue_2_comments="$EXPORT_DIR/dusk-issue-2-comments.txt"

for value in \
    "$CURRENT_BASE_CODE" \
    "$CURRENT_STACK_CODE" \
    "$CURRENT_MONOREPO_RUNTIME" \
    "$CURRENT_MONOREPO_COVERED_PIN" \
    "$CURRENT_MONOREPO_POLICY" \
    "$CURRENT_HYPERLANE_UPSTREAM_BASE" \
    "$CURRENT_RUSK_REF" \
    "$CURRENT_BASE_REPRO_SHA256" \
    "$CURRENT_STACK_REPRO_SHA256" \
    "$CURRENT_TESTMOCK_RUN" \
    "$CURRENT_TESTMOCK_SHA256" \
    "$CURRENT_MULTISIG_RUN" \
    "$CURRENT_MULTISIG_SHA256"; do
    require_literal "$base_pr_body" "$value" "current reassessment evidence"
done
require_literal "$base_pr_body" 'CLOSURE_REASSESSMENT_DECISIONS_2026-07-20.md' 'decision record link'

for value in \
    "$CURRENT_BASE_CODE" \
    "$CURRENT_STACK_CODE" \
    "$CURRENT_MONOREPO_RUNTIME" \
    "$CURRENT_MONOREPO_COVERED_PIN" \
    "$CURRENT_MONOREPO_POLICY" \
    "$CURRENT_RUSK_REF" \
    "$CURRENT_STACK_REPRO_SHA256" \
    "$CURRENT_TESTMOCK_RUN" \
    "$CURRENT_TESTMOCK_SHA256" \
    "$CURRENT_MULTISIG_RUN" \
    "$CURRENT_MULTISIG_SHA256"; do
    require_literal "$stack_pr_body" "$value" "current withdrawal evidence"
done
require_literal "$stack_pr_body" 'CLOSURE_REASSESSMENT_DECISIONS_2026-07-20.md' 'decision record link'

for value in \
    "$CURRENT_BASE_CODE" \
    "$CURRENT_STACK_CODE" \
    "$CURRENT_MONOREPO_RUNTIME" \
    "$CURRENT_MONOREPO_COVERED_PIN" \
    "$CURRENT_MONOREPO_POLICY" \
    "$CURRENT_HYPERLANE_UPSTREAM_BASE" \
    "$CURRENT_RUSK_REF"; do
    require_literal "$monorepo_pr_body" "$value" "current agent compatibility evidence"
done
require_literal "$monorepo_pr_body" 'docs/dusk-companion-compatibility.md' 'cross-repository compatibility manifest'

for value in \
    "$CURRENT_BASE_CODE" \
    "$CURRENT_STACK_CODE" \
    "$CURRENT_MONOREPO_RUNTIME" \
    "$CURRENT_MONOREPO_COVERED_PIN" \
    "$CURRENT_MONOREPO_POLICY" \
    "$CURRENT_HYPERLANE_UPSTREAM_BASE" \
    "$CURRENT_RUSK_REF" \
    "$CURRENT_BASE_REPRO_SHA256" \
    "$CURRENT_STACK_REPRO_SHA256" \
    "$CURRENT_TESTMOCK_RUN" \
    "$CURRENT_TESTMOCK_SHA256" \
    "$CURRENT_MULTISIG_RUN" \
    "$CURRENT_MULTISIG_SHA256"; do
    require_literal "$issue_2_comments" "$value" "current consolidated handoff evidence"
done
require_literal "$issue_2_comments" 'Current gate handoff after the 2026-07-21 escrow, dispatch-credit, and agent reassessment' 'current consolidated handoff marker'

for file in "$base_pr_body" "$stack_pr_body" "$monorepo_pr_body" "$issue_2_comments"; do
    if ! rg -qi 'not (a )?production (authorization|deployment approval)|green pre-merge result is not production authorization' "$file"; then
        fail "$file is missing the pre-merge versus production-authority disclaimer"
    fi
done

if [ -f "$EXPORT_DIR/dusk-pr-3-body.txt" ]; then
    require_literal "$EXPORT_DIR/dusk-pr-3-body.txt" "$REVIEWER_ROUTING_URL" 'advisory reviewer routing link'
    require_literal "$EXPORT_DIR/dusk-pr-3-body.txt" '.github/workflows/manual-repro-dispatcher-gate.yml' 'dispatcher self-check workflow scope'
    require_literal "$EXPORT_DIR/dusk-pr-3-body.txt" '.github/workflows/dusk-review-policy-gate.yml' 'shared review-policy workflow scope'
    require_literal "$EXPORT_DIR/dusk-pr-3-body.txt" 'Manual repro dispatcher gate' 'dispatcher gate evidence'
    require_literal "$EXPORT_DIR/dusk-pr-3-body.txt" 'Dusk review policy gate' 'review policy evidence'
fi

if [ -f "$EXPORT_DIR/dusk-pr-3-comments.txt" ]; then
    check_stale_dispatcher_comments "$EXPORT_DIR/dusk-pr-3-comments.txt" "$EXPORT_DIR/stale-pr-3-comments.txt"
fi
rm -f "$EXPORT_DIR/stale-pr-3-comments.txt"
if rg_to_file "$EXPORT_DIR/pre-rebase-only-e2e-wording.txt" "pre-rebase-only E2E wording" \
    -n \
    -e 'clean-Rusk E2E evidence remains recorded for pre-rebase monorepo' \
    -e 'E2E was not rerun for the docs-only post-rebase head' \
    "$active_review_text"; then
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
