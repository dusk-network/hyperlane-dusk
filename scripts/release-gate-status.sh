#!/usr/bin/env bash
# Print the current machine-checkable release gate status for Dusk Hyperlane.
#
# This script is intentionally read-only except when --fetch-upstream is used,
# which refreshes the local Hyperlane upstream/main ref before reporting drift.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
DUSK_REPO="${DUSK_REPO:-dusk-network/hyperlane-dusk}"
MONOREPO_REPO="${MONOREPO_REPO:-dusk-network/hyperlane-monorepo}"
WORKFLOW_PR_NUMBER="${WORKFLOW_PR_NUMBER:-3}"
SIGNOFF_ISSUES="${SIGNOFF_ISSUES:-4 5 6 7 8 9}"
UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
DUSK_PLACEHOLDER_PATHS="${DUSK_PLACEHOLDER_PATHS:-contracts types data-driver dusk-tx e2e wasm-bindings demo}"
DUSK_PLACEHOLDER_PATTERN="${DUSK_PLACEHOLDER_PATTERN:-todo!|unimplemented!|panic!}"
DUSK_CONTRACT_UNWRAP_PATTERN="${DUSK_CONTRACT_UNWRAP_PATTERN:-\\.unwrap\\(\\)}"
AGENT_PLACEHOLDER_PATTERN="${AGENT_PLACEHOLDER_PATTERN:-todo!|unimplemented!|panic!|expect\(}"
DUSK_REPRO_COVERED_PATHS="${DUSK_REPRO_COVERED_PATHS:-contracts types data-driver dusk-tx e2e wasm-bindings demo tests Cargo.toml Cargo.lock Makefile rust-toolchain.toml scripts/local-repro-check.sh .github/workflows/manual-repro-check.yml}"
LATEST_REPRO_DUSK_REF="${LATEST_REPRO_DUSK_REF:-e8d6596f93c7cb90e87a76ee76126a23608339b5}"
MONOREPO_REPRO_COVERED_PATHS="${MONOREPO_REPRO_COVERED_PATHS:-rust/main/chains/hyperlane-dusk rust/main/Cargo.toml rust/main/Cargo.lock rust/main/hyperlane-base/Cargo.toml rust/main/hyperlane-base/src/settings/chains.rs rust/main/hyperlane-base/src/settings/parser rust/main/hyperlane-base/src/settings/signers.rs rust/main/hyperlane-base/src/contract_sync/cursors/mod.rs rust/main/hyperlane-core/src/chain.rs rust/main/agents/validator/src/reorg_reporter.rs rust/main/lander/src/adapter/chains/factory.rs .github/workflows/dusk-agent-gate.yml .github/workflows/dusk-review-policy-gate.yml .github/workflows/rust-docker.yml .github/workflows/monorepo-docker.yml .github/workflows/rust.yml .github/workflows/test.yml .github/workflows/rebalancer-e2e-test.yml}"
LATEST_REPRO_MONOREPO_REF="${LATEST_REPRO_MONOREPO_REF:-a931f75b3d23d2e15e75f2e064470a1a01289abb}"
UPSTREAM_SUBMISSION_REPO="${UPSTREAM_SUBMISSION_REPO:-hyperlane-xyz/hyperlane-monorepo}"
UPSTREAM_SUBMISSION_HEAD="${UPSTREAM_SUBMISSION_HEAD:-dusk-network:feat/dusk-support-v2}"
POST_REBASE_E2E_URL="${POST_REBASE_E2E_URL:-${CURRENT_E2E_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433528683}}"
POST_REBASE_E2E_ARCHIVE_URL="${POST_REBASE_E2E_ARCHIVE_URL:-${CURRENT_E2E_ARCHIVE_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433564278}}"
DEPENDENCY_REMEDIATED_E2E_URL="${DEPENDENCY_REMEDIATED_E2E_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389}"
LATEST_REPRO_URL="${LATEST_REPRO_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043}"
CURRENT_GATE_REFRESH_TEXT="${CURRENT_GATE_REFRESH_TEXT:-head and gate refresh:}"
REVIEWER_ROUTING_URL="${REVIEWER_ROUTING_URL:-https://github.com/dusk-network/hyperlane-dusk/blob/feat/dusk-hardening-v2/REVIEWERS.md}"
GATE_STATUS_FRESH_TEXT="${GATE_STATUS_FRESH_TEXT:-make gate-status-fresh}"
DEPENDENCY_ALERT_STATUS_TEXT="${DEPENDENCY_ALERT_STATUS_TEXT:-make dependency-alert-status}"
COMPLETION_AUDIT_STATUS_TEXT="${COMPLETION_AUDIT_STATUS_TEXT:-make completion-audit-status}"
REVIEW_GATES_TEXT="${REVIEW_GATES_TEXT:-make review-gates}"
REVIEW_GATES_ARCHIVE_SELF_TEST_TEXT="${REVIEW_GATES_ARCHIVE_SELF_TEST_TEXT:-archive hygiene self-tests}"
REVIEW_GATES_ARCHIVE_SCAN_TEXT="${REVIEW_GATES_ARCHIVE_SCAN_TEXT:-extracted evidence archive hygiene scans}"
PRODUCTION_READINESS_GUARD_TEXT="${PRODUCTION_READINESS_GUARD_TEXT:-make production-readiness-guard}"
BRANCH_PROTECTION_STATUS_TEXT="${BRANCH_PROTECTION_STATUS_TEXT:-required status-check policy enabled}"
REPRO_PATH_DELTA_TEXT="${REPRO_PATH_DELTA_TEXT:-latest clean-layout repro path delta}"
MONOREPO_REPRO_DELTA_TEXT="${MONOREPO_REPRO_DELTA_TEXT:-monorepoCoveredPathDelta}"
CI_PROVISIONING_RUNBOOK_URL="${CI_PROVISIONING_RUNBOOK_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4435830841}"
REQUIRED_SECRET_NAME="${REQUIRED_SECRET_NAME:-DUSK_ORG_READ_TOKEN}"
STATUS_SECRET_NAME="${STATUS_SECRET_NAME:-DUSK_STATUS_READ_TOKEN}"
REQUIRED_RUNNER_LABEL="${REQUIRED_RUNNER_LABEL:-dusk-hyperlane}"
DUSK_REQUIRED_STATUS_CONTEXTS="${DUSK_REQUIRED_STATUS_CONTEXTS:-Dusk review policy gate|Production readiness guard}"
MONOREPO_REQUIRED_STATUS_CONTEXTS="${MONOREPO_REQUIRED_STATUS_CONTEXTS:-Dusk review policy gate|Dusk agent validation}"
FETCH_UPSTREAM=0
PLACEHOLDER_SCAN_ONLY=0

fail() {
    echo "[FAIL] $*" >&2
    exit 1
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
Usage: bash scripts/release-gate-status.sh [options]

Prints the current machine-checkable review status:
  - local Dusk and Hyperlane worktree state
  - untracked source status for both active repositories
  - Dusk and monorepo PR state, labels, mergeability, reviews, and status checks
  - default-branch protection and merge method settings for the internal repos
  - production sign-off checklist counts from dusk-network/hyperlane-dusk#2
  - split production decision issue states from dusk-network/hyperlane-dusk#4-#9
  - workflow visibility for dusk-network/hyperlane-dusk
  - repo-level Actions source/status secret and self-hosted runner visibility
    for CI gate #8
  - reviewer-facing evidence, routing, fresh-gate, dependency-alert,
    completion-audit, review-gates, and production-readiness handoff visibility
  - Dusk Dependabot open-alert visibility and local Cargo.lock vulnerable-range
    comparison
  - Hyperlane upstream/main drift for the local monorepo checkout
  - Dusk path delta since the latest clean-layout repro source ref
  - Dusk agent panic/placeholder scan in rust/main/chains/hyperlane-dusk/src

Options:
  --fetch-upstream     Run git fetch <upstream-remote> main before drift check.
  --monorepo-dir DIR   Hyperlane monorepo checkout.
                       Default: $MONOREPO_DIR
  --placeholder-scan-only
                       Run only the local runtime placeholder scans.
  -h, --help           Show this help.

Environment:
  DUSK_REPO            GitHub repo for Dusk contract/tooling PRs.
                       Default: $DUSK_REPO
  MONOREPO_REPO        GitHub repo for Hyperlane integration PRs.
                       Default: $MONOREPO_REPO
  WORKFLOW_PR_NUMBER   Dusk repo PR number for the manual workflow dispatcher.
                       Default: $WORKFLOW_PR_NUMBER
  SIGNOFF_ISSUES       Space-separated Dusk repo issue numbers for split
                       production decisions.
                       Default: $SIGNOFF_ISSUES
  UPSTREAM_REMOTE      Hyperlane upstream remote name.
                       Default: $UPSTREAM_REMOTE
  DUSK_PLACEHOLDER_PATHS
                       Space-separated tracked Dusk repo paths scanned for
                       runtime placeholder macros.
                       Default: $DUSK_PLACEHOLDER_PATHS
  DUSK_PLACEHOLDER_PATTERN
                       Extended regex used for tracked Dusk repo runtime
                       placeholder scans.
                       Default: $DUSK_PLACEHOLDER_PATTERN
  DUSK_CONTRACT_UNWRAP_PATTERN
                       Extended regex used to reject direct unwrap() calls in
                       production contract source.
                       Default: $DUSK_CONTRACT_UNWRAP_PATTERN
  AGENT_PLACEHOLDER_PATTERN
                       Extended regex used for Dusk agent runtime
                       panic/placeholder scans.
                       Default: $AGENT_PLACEHOLDER_PATTERN
  DUSK_REPRO_COVERED_PATHS
                       Space-separated Dusk repo paths whose changes would make
                       the latest clean-layout repro stale for runtime/test
                       source coverage.
                       Default: $DUSK_REPRO_COVERED_PATHS
  LATEST_REPRO_DUSK_REF
                       Dusk source ref covered by the latest clean-layout repro.
                       Default: $LATEST_REPRO_DUSK_REF
  MONOREPO_REPRO_COVERED_PATHS
                       Space-separated Hyperlane monorepo paths whose changes
                       would make the latest clean-layout repro stale for
                       runtime, agent, or CI workflow coverage.
                       Default: $MONOREPO_REPRO_COVERED_PATHS
  LATEST_REPRO_MONOREPO_REF
                       Hyperlane monorepo ref covered by the latest
                       clean-layout repro.
                       Default: $LATEST_REPRO_MONOREPO_REF
  UPSTREAM_SUBMISSION_REPO
                       Upstream Hyperlane repository checked for premature
                       Dusk-head PRs.
                       Default: $UPSTREAM_SUBMISSION_REPO
  UPSTREAM_SUBMISSION_HEAD
                       Head owner/branch searched in the upstream Hyperlane
                       repository.
                       Default: $UPSTREAM_SUBMISSION_HEAD
  POST_REBASE_E2E_URL  Post-rebase E2E evidence URL expected in active
                       reviewer-facing bodies.
                       Default: $POST_REBASE_E2E_URL
  POST_REBASE_E2E_ARCHIVE_URL
                       Post-rebase E2E archive URL expected in active
                       reviewer-facing bodies.
                       Default: $POST_REBASE_E2E_ARCHIVE_URL
  DEPENDENCY_REMEDIATED_E2E_URL
                       Dependency-remediated E2E evidence URL expected in
                       active reviewer-facing bodies.
                       Default: $DEPENDENCY_REMEDIATED_E2E_URL
  LATEST_REPRO_URL     Latest clean-layout repro evidence URL expected in
                       active reviewer-facing bodies.
                       Default: $LATEST_REPRO_URL
  CURRENT_GATE_REFRESH_TEXT
                       Current head/gate refresh handoff text expected in active
                       reviewer-facing bodies.
                       Default: $CURRENT_GATE_REFRESH_TEXT
  REVIEWER_ROUTING_URL Advisory reviewer routing URL expected in active
                       reviewer-facing bodies.
                       Default: $REVIEWER_ROUTING_URL
  GATE_STATUS_FRESH_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the fresh upstream gate command.
                       Default: $GATE_STATUS_FRESH_TEXT
  DEPENDENCY_ALERT_STATUS_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the dependency alert comparison.
                       Default: $DEPENDENCY_ALERT_STATUS_TEXT
  COMPLETION_AUDIT_STATUS_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the preservation audit check.
                       Default: $COMPLETION_AUDIT_STATUS_TEXT
  REVIEW_GATES_TEXT    Text expected in active implementation PR and sign-off
                       bodies to expose the lightweight gate bundle.
                       Default: $REVIEW_GATES_TEXT
  REVIEW_GATES_ARCHIVE_SELF_TEST_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose review-gates archive self-test coverage.
                       Default: $REVIEW_GATES_ARCHIVE_SELF_TEST_TEXT
  REVIEW_GATES_ARCHIVE_SCAN_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose review-gates extracted archive scans.
                       Default: $REVIEW_GATES_ARCHIVE_SCAN_TEXT
  PRODUCTION_READINESS_GUARD_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the blocking production readiness guard.
                       Default: $PRODUCTION_READINESS_GUARD_TEXT
  BRANCH_PROTECTION_STATUS_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the required status-check gate.
                       Default: $BRANCH_PROTECTION_STATUS_TEXT
  REPRO_PATH_DELTA_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose latest clean-layout repro path delta.
                       Default: $REPRO_PATH_DELTA_TEXT
  MONOREPO_REPRO_DELTA_TEXT
                       Text expected in active reviewer-facing bodies to expose
                       the monorepo clean-layout repro path delta.
                       Default: $MONOREPO_REPRO_DELTA_TEXT
  CI_PROVISIONING_RUNBOOK_URL
                       Admin-side CI provisioning runbook URL expected in the
                       manual workflow dispatcher PR body.
                       Default: $CI_PROVISIONING_RUNBOOK_URL
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --fetch-upstream)
            FETCH_UPSTREAM=1
            shift
            ;;
        --placeholder-scan-only)
            PLACEHOLDER_SCAN_ONLY=1
            shift
            ;;
        --monorepo-dir)
            MONOREPO_DIR="${2:-}"
            [ -n "$MONOREPO_DIR" ] || {
                echo "[FAIL] --monorepo-dir requires a path" >&2
                exit 1
            }
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "[FAIL] unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

command -v git >/dev/null 2>&1 || {
    echo "[FAIL] git is required" >&2
    exit 1
}

if [ -d "$MONOREPO_DIR" ]; then
    MONOREPO_DIR="$(cd "$MONOREPO_DIR" && pwd -P)"
fi

section() {
    printf '\n== %s ==\n' "$1"
}

print_runtime_placeholder_scan() {
    section "Runtime Placeholder Scan"
    echo "duskRepoPaths: $DUSK_PLACEHOLDER_PATHS"
    if git_grep_to_file /tmp/hyperlane-dusk-repo-placeholder-scan.$$ "Dusk repo runtime placeholder" "$ROOT" -n -E "$DUSK_PLACEHOLDER_PATTERN" -- $DUSK_PLACEHOLDER_PATHS; then
        cat /tmp/hyperlane-dusk-repo-placeholder-scan.$$
        rm -f /tmp/hyperlane-dusk-repo-placeholder-scan.$$
    else
        rm -f /tmp/hyperlane-dusk-repo-placeholder-scan.$$
        echo "no matches in Dusk repo scoped runtime paths"
    fi

    if git_grep_to_file /tmp/hyperlane-dusk-contract-unwrap-scan.$$ "Dusk contract direct unwrap" "$ROOT" -n -E "$DUSK_CONTRACT_UNWRAP_PATTERN" -- contracts; then
        cat /tmp/hyperlane-dusk-contract-unwrap-scan.$$
        rm -f /tmp/hyperlane-dusk-contract-unwrap-scan.$$
    else
        rm -f /tmp/hyperlane-dusk-contract-unwrap-scan.$$
        echo "no direct unwrap() matches in Dusk contract source"
    fi

    if [ -d "$MONOREPO_DIR/rust/main/chains/hyperlane-dusk" ]; then
        if git_grep_to_file /tmp/hyperlane-dusk-placeholder-scan.$$ "Dusk agent runtime panic/placeholder" "$MONOREPO_DIR" -n -E "$AGENT_PLACEHOLDER_PATTERN" -- rust/main/chains/hyperlane-dusk/src; then
            cat /tmp/hyperlane-dusk-placeholder-scan.$$
            rm -f /tmp/hyperlane-dusk-placeholder-scan.$$
        else
            rm -f /tmp/hyperlane-dusk-placeholder-scan.$$
            echo "no panic/placeholder matches in rust/main/chains/hyperlane-dusk/src"
        fi
    else
        echo "missing Dusk chain crate in monorepo checkout"
    fi
}

if [ "$PLACEHOLDER_SCAN_ONLY" -eq 1 ]; then
    print_runtime_placeholder_scan
    exit 0
fi

command -v gh >/dev/null 2>&1 || {
    echo "[FAIL] gh is required" >&2
    exit 1
}

print_pr() {
    local repo="$1"
    local number="$2"
    local label="$3"

    section "$label PR"
    gh pr view "$number" --repo "$repo" \
        --json state,labels,mergeable,headRefOid,reviewDecision,reviewRequests,statusCheckRollup,url \
        --jq '
            "url: \(.url)\n" +
            "state: \(.state)\n" +
            "labels: \([.labels[].name] | join(", "))\n" +
            "mergeable: \(.mergeable)\n" +
            "head: \(.headRefOid)\n" +
            "reviewDecision: \(.reviewDecision // "")\n" +
            "reviewRequests: \([.reviewRequests[].login] | join(", "))\n" +
            "statusChecks: \(.statusCheckRollup | length)\n" +
            "statusCheckSummary: \(
                [.statusCheckRollup[] | ((.status // "") + ":" + (.conclusion // ""))]
                | sort
                | group_by(.)
                | map("\(.[0])=\(length)")
                | join(", ")
            )"
        '
}

print_issue() {
    local repo="$1"
    local number="$2"

    gh issue view "$number" --repo "$repo" \
        --json state,title,labels,assignees,url \
        --jq '
            "#\(.url | split("/")[-1]) \(.title)\n" +
            "  url: \(.url)\n" +
            "  state: \(.state)\n" +
            "  labels: \([.labels[].name] | join(", "))\n" +
            "  assignees: \([.assignees[].login] | join(", "))"
        '
}

print_repo_merge_policy() {
    local repo="$1"
    local required_contexts="${2:-}"
    local default_branch
    local protection_json
    local missing_contexts

    gh api "repos/$repo" \
        --jq '
            "repo: \(.full_name)\n" +
            "defaultBranch: \(.default_branch)\n" +
            "allowMergeCommit: \(.allow_merge_commit)\n" +
            "allowSquashMerge: \(.allow_squash_merge)\n" +
            "allowRebaseMerge: \(.allow_rebase_merge)"
        '

    default_branch="$(gh api "repos/$repo" --jq .default_branch)"
    if protection_json="$(gh api "repos/$repo/branches/$default_branch/protection" --jq '{requiredStatusChecks: (.required_status_checks.contexts // []), strictStatusChecks: .required_status_checks.strict, requiresReviews: (.required_pull_request_reviews != null)}' 2>/tmp/hyperlane-dusk-protection.$$.err)"; then
        echo "branchProtection: enabled"
        printf '%s\n' "$protection_json" | sed 's/^/  /'
        if ! printf '%s\n' "$protection_json" | jq -e '.strictStatusChecks == true' >/dev/null; then
            echo "  strictStatusChecksMismatch: required true"
        fi
        if [ -n "$required_contexts" ]; then
            echo "  expectedRequiredStatusChecks: $(printf '%s\n' "$required_contexts" | tr '|' ',')"
            missing_contexts="$(
                printf '%s\n' "$required_contexts" | tr '|' '\n' | while IFS= read -r context; do
                    [ -n "$context" ] || continue
                    if ! printf '%s\n' "$protection_json" | jq -e --arg context "$context" \
                        '.requiredStatusChecks | index($context) != null' >/dev/null; then
                        printf '%s\n' "$context"
                    fi
                done
            )"
            if [ -n "$missing_contexts" ]; then
                echo "  missingRequiredStatusChecks: $(printf '%s\n' "$missing_contexts" | paste -sd ',' -)"
            else
                echo "  missingRequiredStatusChecks: none"
            fi
        fi
    else
        if printf '%s\n' "$protection_json" | grep -q '"message":"Branch not protected"'; then
            echo "branchProtection: none"
        else
            echo "branchProtection: unknown"
        fi
        if [ -n "$protection_json" ] && ! printf '%s\n' "$protection_json" | grep -q '"message":"Branch not protected"'; then
            printf '%s\n' "$protection_json" | sed 's/^/  /'
        fi
        if ! grep -q 'Branch not protected' /tmp/hyperlane-dusk-protection.$$.err; then
            sed 's/^/  /' /tmp/hyperlane-dusk-protection.$$.err
        fi
    fi
    rm -f /tmp/hyperlane-dusk-protection.$$.err
}

unchecked_count() {
    grep -c '^- \[ \]' || true
}

checked_count() {
    grep -c '^- \[x\]' || true
}

print_link_presence() {
    local label="$1"
    local body="$2"

    echo "$label:"
    if printf '%s\n' "$body" | grep -Fq "$LATEST_REPRO_URL"; then
        echo "  latestCleanLayoutRepro: present"
    else
        echo "  latestCleanLayoutRepro: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$CURRENT_GATE_REFRESH_TEXT"; then
        echo "  currentGateRefreshHandoff: present"
    else
        echo "  currentGateRefreshHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$POST_REBASE_E2E_URL"; then
        echo "  postRebaseE2E: present"
    else
        echo "  postRebaseE2E: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$POST_REBASE_E2E_ARCHIVE_URL"; then
        echo "  postRebaseE2EArchive: present"
    else
        echo "  postRebaseE2EArchive: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$DEPENDENCY_REMEDIATED_E2E_URL"; then
        echo "  dependencyRemediatedE2E: present"
    else
        echo "  dependencyRemediatedE2E: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$REVIEWER_ROUTING_URL"; then
        echo "  reviewerRouting: present"
    else
        echo "  reviewerRouting: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$GATE_STATUS_FRESH_TEXT"; then
        echo "  freshGateHandoff: present"
    else
        echo "  freshGateHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$DEPENDENCY_ALERT_STATUS_TEXT"; then
        echo "  dependencyAlertHandoff: present"
    else
        echo "  dependencyAlertHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$COMPLETION_AUDIT_STATUS_TEXT"; then
        echo "  completionAuditHandoff: present"
    else
        echo "  completionAuditHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$REVIEW_GATES_TEXT"; then
        echo "  reviewGatesHandoff: present"
    else
        echo "  reviewGatesHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$REVIEW_GATES_ARCHIVE_SELF_TEST_TEXT"; then
        echo "  reviewGatesArchiveSelfTestHandoff: present"
    else
        echo "  reviewGatesArchiveSelfTestHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$REVIEW_GATES_ARCHIVE_SCAN_TEXT"; then
        echo "  reviewGatesArchiveScanHandoff: present"
    else
        echo "  reviewGatesArchiveScanHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$PRODUCTION_READINESS_GUARD_TEXT"; then
        echo "  productionReadinessGuardHandoff: present"
    else
        echo "  productionReadinessGuardHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$BRANCH_PROTECTION_STATUS_TEXT"; then
        echo "  branchProtectionStatusHandoff: present"
    else
        echo "  branchProtectionStatusHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$REPRO_PATH_DELTA_TEXT"; then
        echo "  reproPathDeltaHandoff: present"
    else
        echo "  reproPathDeltaHandoff: missing"
    fi

    if printf '%s\n' "$body" | grep -Fq "$MONOREPO_REPRO_DELTA_TEXT"; then
        echo "  monorepoReproDeltaHandoff: present"
    else
        echo "  monorepoReproDeltaHandoff: missing"
    fi
}

print_reviewer_routing_presence() {
    local label="$1"
    local body="$2"

    echo "$label:"
    if printf '%s\n' "$body" | grep -Fq "$REVIEWER_ROUTING_URL"; then
        echo "  reviewerRouting: present"
    else
        echo "  reviewerRouting: missing"
    fi
}

print_workflow_pr_handoff_presence() {
    local label="$1"
    local body="$2"

    print_reviewer_routing_presence "$label" "$body"
    if printf '%s\n' "$body" | grep -Fq "$CI_PROVISIONING_RUNBOOK_URL"; then
        echo "  ciProvisioningRunbook: present"
    else
        echo "  ciProvisioningRunbook: missing"
    fi
}

section "Local Worktrees"
echo "dusk: $(git -C "$ROOT" rev-parse HEAD)"
git -C "$ROOT" status --short --branch
if [ -d "$MONOREPO_DIR/.git" ] || git -C "$MONOREPO_DIR" rev-parse --git-dir >/dev/null 2>&1; then
    echo "monorepo: $(git -C "$MONOREPO_DIR" rev-parse HEAD)"
    git -C "$MONOREPO_DIR" status --short --branch
else
    echo "monorepo: missing checkout at $MONOREPO_DIR"
fi

section "Branch Protection And Merge Settings"
print_repo_merge_policy "$DUSK_REPO" "$DUSK_REQUIRED_STATUS_CONTEXTS"
print_repo_merge_policy "$MONOREPO_REPO" "$MONOREPO_REQUIRED_STATUS_CONTEXTS"

section "Untracked Source Check"
dusk_untracked="$(git -C "$ROOT" ls-files --others --exclude-standard)"
if [ -n "$dusk_untracked" ]; then
    echo "duskUntrackedSource: present"
    printf '%s\n' "$dusk_untracked" | sed 's/^/  /'
else
    echo "duskUntrackedSource: none"
fi

if [ -d "$MONOREPO_DIR" ]; then
    monorepo_untracked="$(git -C "$MONOREPO_DIR" ls-files --others --exclude-standard)"
    if [ -n "$monorepo_untracked" ]; then
        echo "monorepoUntrackedSource: present"
        printf '%s\n' "$monorepo_untracked" | sed 's/^/  /'
    else
        echo "monorepoUntrackedSource: none"
    fi
else
    echo "monorepoUntrackedSource: unknown"
fi

print_pr "$DUSK_REPO" 1 "Dusk"
print_pr "$MONOREPO_REPO" 1 "Hyperlane monorepo"
if gh pr view "$WORKFLOW_PR_NUMBER" --repo "$DUSK_REPO" --json number >/dev/null 2>&1; then
    print_pr "$DUSK_REPO" "$WORKFLOW_PR_NUMBER" "Manual workflow dispatcher"
fi

section "Production Sign-Off Issue"
issue_body="$(gh issue view 2 --repo "$DUSK_REPO" --json body --jq .body)"
echo "url: https://github.com/$DUSK_REPO/issues/2"
echo "uncheckedItems: $(printf '%s\n' "$issue_body" | unchecked_count)"
echo "checkedItems: $(printf '%s\n' "$issue_body" | checked_count)"

section "Review Handoff Link Visibility"
dusk_pr_body="$(gh pr view 1 --repo "$DUSK_REPO" --json body --jq .body)"
monorepo_pr_body="$(gh pr view 1 --repo "$MONOREPO_REPO" --json body --jq .body)"
print_link_presence "duskPR1" "$dusk_pr_body"
print_link_presence "monorepoPR1" "$monorepo_pr_body"
print_link_presence "signoffIssue2" "$issue_body"
if gh pr view "$WORKFLOW_PR_NUMBER" --repo "$DUSK_REPO" --json number >/dev/null 2>&1; then
    workflow_pr_body="$(gh pr view "$WORKFLOW_PR_NUMBER" --repo "$DUSK_REPO" --json body --jq .body)"
    print_workflow_pr_handoff_presence "workflowPR$WORKFLOW_PR_NUMBER" "$workflow_pr_body"
fi

section "Split Production Decision Issues"
open_split_issues=0
closed_split_issues=0
for issue in $SIGNOFF_ISSUES; do
    if issue_json="$(gh issue view "$issue" --repo "$DUSK_REPO" --json state --jq .state 2>/dev/null)"; then
        print_issue "$DUSK_REPO" "$issue"
        if [ "$issue_json" = "OPEN" ]; then
            open_split_issues=$((open_split_issues + 1))
        else
            closed_split_issues=$((closed_split_issues + 1))
        fi
    else
        echo "#$issue missing or inaccessible"
    fi
done
echo "openSplitIssues: $open_split_issues"
echo "closedSplitIssues: $closed_split_issues"

section "Workflow Visibility"
workflow_output="$(gh workflow list --repo "$DUSK_REPO" --all || true)"
if [ -n "$workflow_output" ]; then
    printf '%s\n' "$workflow_output"
else
    echo "no workflows visible through GitHub API"
fi

section "CI Provisioning Visibility"
if repo_secrets_json="$(gh api "repos/$DUSK_REPO/actions/secrets" 2>/tmp/hyperlane-dusk-secrets.$$.err)"; then
    repo_secrets_count="$(printf '%s\n' "$repo_secrets_json" | jq .total_count)"
    repo_required_secret_visible="$(printf '%s\n' "$repo_secrets_json" | jq --arg name "$REQUIRED_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
    repo_status_secret_visible="$(printf '%s\n' "$repo_secrets_json" | jq --arg name "$STATUS_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
    echo "repoSecretsVisible: $repo_secrets_count"
    echo "repoRequiredSecretVisible: $repo_required_secret_visible"
    echo "repoStatusSecretVisible: $repo_status_secret_visible"
else
    echo "repoSecretsVisible: unknown"
    echo "repoRequiredSecretVisible: unknown"
    echo "repoStatusSecretVisible: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-secrets.$$.err
fi
rm -f /tmp/hyperlane-dusk-secrets.$$.err

if monorepo_secrets_json="$(gh api "repos/$MONOREPO_REPO/actions/secrets" 2>/tmp/hyperlane-dusk-monorepo-secrets.$$.err)"; then
    monorepo_secrets_count="$(printf '%s\n' "$monorepo_secrets_json" | jq .total_count)"
    monorepo_required_secret_visible="$(printf '%s\n' "$monorepo_secrets_json" | jq --arg name "$REQUIRED_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
    monorepo_status_secret_visible="$(printf '%s\n' "$monorepo_secrets_json" | jq --arg name "$STATUS_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
    echo "monorepoRepoSecretsVisible: $monorepo_secrets_count"
    echo "monorepoRepoRequiredSecretVisible: $monorepo_required_secret_visible"
    echo "monorepoRepoStatusSecretVisible: $monorepo_status_secret_visible"
else
    echo "monorepoRepoSecretsVisible: unknown"
    echo "monorepoRepoRequiredSecretVisible: unknown"
    echo "monorepoRepoStatusSecretVisible: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-monorepo-secrets.$$.err
fi
rm -f /tmp/hyperlane-dusk-monorepo-secrets.$$.err

runner_has_label() {
    jq --arg label "$REQUIRED_RUNNER_LABEL" \
        '[.runners[]? | select(any(.labels[]?; .name == $label))] | length > 0'
}

if repo_runners_json="$(gh api "repos/$DUSK_REPO/actions/runners" 2>/tmp/hyperlane-dusk-runners.$$.err)"; then
    repo_runners_count="$(printf '%s\n' "$repo_runners_json" | jq .total_count)"
    repo_runner_with_label="$(printf '%s\n' "$repo_runners_json" | runner_has_label)"
    echo "repoSelfHostedRunnersVisible: $repo_runners_count"
    echo "repoRunnerWithRequiredLabelVisible: $repo_runner_with_label"
else
    echo "repoSelfHostedRunnersVisible: unknown"
    echo "repoRunnerWithRequiredLabelVisible: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-runners.$$.err
fi
rm -f /tmp/hyperlane-dusk-runners.$$.err

org_name="${DUSK_REPO%%/*}"
if org_runners_json="$(gh api "orgs/$org_name/actions/runners" 2>/tmp/hyperlane-dusk-org-runners.$$.err)"; then
    org_runners_count="$(printf '%s\n' "$org_runners_json" | jq .total_count)"
    org_runner_with_label="$(printf '%s\n' "$org_runners_json" | runner_has_label)"
    echo "orgSelfHostedRunnersVisible: $org_runners_count"
    echo "orgRunnerWithRequiredLabelVisible: $org_runner_with_label"
else
    echo "orgSelfHostedRunnersVisible: unknown"
    echo "orgRunnerWithRequiredLabelVisible: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-org-runners.$$.err
fi
rm -f /tmp/hyperlane-dusk-org-runners.$$.err

section "Dusk Dependency Alert Visibility"
dependabot_alerts="/tmp/hyperlane-dusk-dependabot-alerts.$$"
if gh api "repos/$DUSK_REPO/dependabot/alerts?state=open&per_page=100" --paginate \
    --jq '.[] | "\(.dependency.package.name)\t\(.dependency.manifest_path)\t\(.security_advisory.severity)\t\(.security_advisory.ghsa_id)"' \
    >"$dependabot_alerts" 2>/tmp/hyperlane-dusk-dependabot.$$.err; then
    echo "openAlerts: $(wc -l <"$dependabot_alerts" | tr -d ' ')"
    if [ -s "$dependabot_alerts" ]; then
        awk -F '\t' '
            {
                key = $1 " (" $2 ")"
                count[key]++
                if (sev[key] == "") {
                    sev[key] = $3
                } else if (sev[key] !~ "(^|, )" $3 "(,|$)") {
                    sev[key] = sev[key] ", " $3
                }
            }

            END {
                for (key in count) {
                    print "  " key ": " count[key] " open; severities: " sev[key]
                }
            }
        ' "$dependabot_alerts" | sort
    fi
else
    echo "openAlerts: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-dependabot.$$.err
fi
rm -f "$dependabot_alerts" /tmp/hyperlane-dusk-dependabot.$$.err

if dependency_status="$(bash "$ROOT/scripts/dependency-alert-status.sh" --summary-only 2>&1)"; then
    printf '%s\n' "$dependency_status" | sed 's/^/  /'
else
    echo "dependencyAlertPatchFloorStatus: failed"
    printf '%s\n' "$dependency_status" | sed 's/^/  /'
fi

section "Hyperlane Upstream Drift"
if [ -d "$MONOREPO_DIR" ]; then
    if git -C "$MONOREPO_DIR" remote get-url "$UPSTREAM_REMOTE" >/dev/null 2>&1; then
        if [ "$FETCH_UPSTREAM" -eq 1 ]; then
            git -C "$MONOREPO_DIR" fetch "$UPSTREAM_REMOTE" main
        fi
        upstream_ref="$UPSTREAM_REMOTE/main"
        if git -C "$MONOREPO_DIR" rev-parse --verify "$upstream_ref" >/dev/null 2>&1; then
            echo "upstreamHead: $(git -C "$MONOREPO_DIR" rev-parse "$upstream_ref")"
            echo "mergeBase: $(git -C "$MONOREPO_DIR" merge-base HEAD "$upstream_ref")"
            echo "aheadBehind: $(git -C "$MONOREPO_DIR" rev-list --left-right --count HEAD..."$upstream_ref")"
        else
            echo "missing local ref: $upstream_ref"
        fi
    else
        echo "missing remote: $UPSTREAM_REMOTE"
    fi
else
    echo "missing monorepo checkout: $MONOREPO_DIR"
fi

print_runtime_placeholder_scan

section "Latest Clean Repro Delta"
echo "latestReproDuskRef: $LATEST_REPRO_DUSK_REF"
echo "reproCoveredPaths: $DUSK_REPRO_COVERED_PATHS"
if git -C "$ROOT" rev-parse --verify "$LATEST_REPRO_DUSK_REF^{commit}" >/dev/null 2>&1; then
    all_delta="/tmp/hyperlane-dusk-latest-repro-all-delta.$$"
    covered_delta="/tmp/hyperlane-dusk-latest-repro-covered-delta.$$"
    git -C "$ROOT" diff --name-only "$LATEST_REPRO_DUSK_REF"..HEAD >"$all_delta"
    git -C "$ROOT" diff --name-only "$LATEST_REPRO_DUSK_REF"..HEAD -- $DUSK_REPRO_COVERED_PATHS >"$covered_delta"
    echo "changedPathsSinceLatestRepro: $(wc -l <"$all_delta" | tr -d ' ')"
    if [ -s "$covered_delta" ]; then
        echo "coveredPathDelta: present"
        sed 's/^/  /' "$covered_delta"
    else
        echo "coveredPathDelta: none"
    fi
    if [ -s "$all_delta" ]; then
        echo "allPathDelta:"
        sed 's/^/  /' "$all_delta"
    else
        echo "allPathDelta: none"
    fi
    rm -f "$all_delta" "$covered_delta"
else
    echo "latestReproDuskRefStatus: missing"
fi

echo "latestReproMonorepoRef: $LATEST_REPRO_MONOREPO_REF"
echo "monorepoReproCoveredPaths: $MONOREPO_REPRO_COVERED_PATHS"
if [ -d "$MONOREPO_DIR" ] && git -C "$MONOREPO_DIR" rev-parse --verify "$LATEST_REPRO_MONOREPO_REF^{commit}" >/dev/null 2>&1; then
    monorepo_all_delta="/tmp/hyperlane-monorepo-latest-repro-all-delta.$$"
    monorepo_covered_delta="/tmp/hyperlane-monorepo-latest-repro-covered-delta.$$"
    git -C "$MONOREPO_DIR" diff --name-only "$LATEST_REPRO_MONOREPO_REF"..HEAD >"$monorepo_all_delta"
    git -C "$MONOREPO_DIR" diff --name-only "$LATEST_REPRO_MONOREPO_REF"..HEAD -- $MONOREPO_REPRO_COVERED_PATHS >"$monorepo_covered_delta"
    echo "monorepoChangedPathsSinceLatestRepro: $(wc -l <"$monorepo_all_delta" | tr -d ' ')"
    if [ -s "$monorepo_covered_delta" ]; then
        echo "monorepoCoveredPathDelta: present"
        sed 's/^/  /' "$monorepo_covered_delta"
    else
        echo "monorepoCoveredPathDelta: none"
    fi
    if [ -s "$monorepo_all_delta" ]; then
        echo "monorepoAllPathDelta:"
        sed 's/^/  /' "$monorepo_all_delta"
    else
        echo "monorepoAllPathDelta: none"
    fi
    rm -f "$monorepo_all_delta" "$monorepo_covered_delta"
else
    echo "latestReproMonorepoRefStatus: missing"
fi

section "Upstream Submission Gate"
upstream_query="repo:$UPSTREAM_SUBMISSION_REPO is:pr is:open head:$UPSTREAM_SUBMISSION_HEAD"
printf 'upstreamSubmissionRepo: %s\n' "$UPSTREAM_SUBMISSION_REPO"
printf 'upstreamSubmissionHead: %s\n' "$UPSTREAM_SUBMISSION_HEAD"
if upstream_prs_json="$(gh api -X GET search/issues -f "q=$upstream_query" 2>/tmp/hyperlane-gate-upstream-prs.$$.err)"; then
    upstream_open_prs="$(printf '%s\n' "$upstream_prs_json" | jq -r .total_count)"
    printf 'upstreamOpenPrsFromDuskHead: %s\n' "$upstream_open_prs"
    if [ "$upstream_open_prs" -gt 0 ]; then
        printf '%s\n' "$upstream_prs_json" \
            | jq -r '.items[] | "  #\(.number) \(.html_url) \(.title)"'
    fi
else
    echo "upstreamOpenPrsFromDuskHead: unknown"
    sed 's/^/  /' /tmp/hyperlane-gate-upstream-prs.$$.err
fi
rm -f /tmp/hyperlane-gate-upstream-prs.$$.err

section "Summary"
echo "This script reports machine-checkable gates only."
echo "The objective remains blocked until the unchecked sign-off items, reviews,"
echo "CI/default-branch workflow runner and secret provisioning,"
echo "internal PR merge, and upstream-prep gate are resolved by Dusk reviewers."
