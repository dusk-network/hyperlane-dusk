#!/usr/bin/env bash
# Fail while machine-checkable production-readiness blockers remain open.
#
# This is a negative guardrail: passing it would only mean the known
# machine-checkable blockers are closed. It does not replace Dusk reviewer
# judgment, fresh E2E evidence, CI logs, or the production sign-off issue.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MONOREPO_DIR="${MONOREPO_DIR:-$ROOT/../hyperlane-monorepo}"
DUSK_REPO="${DUSK_REPO:-dusk-network/hyperlane-dusk}"
MONOREPO_REPO="${MONOREPO_REPO:-dusk-network/hyperlane-monorepo}"
WORKFLOW_PR_NUMBER="${WORKFLOW_PR_NUMBER:-3}"
SIGNOFF_ISSUES="${SIGNOFF_ISSUES:-4 5 6 7 8 9}"
UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
DUSK_REPRO_COVERED_PATHS="${DUSK_REPRO_COVERED_PATHS:-contracts types data-driver dusk-tx e2e wasm-bindings demo tests Cargo.toml Cargo.lock}"
LATEST_REPRO_DUSK_REF="${LATEST_REPRO_DUSK_REF:-eff5e3bc181707756eead42880c71ccd1e685d34}"
MONOREPO_REPRO_COVERED_PATHS="${MONOREPO_REPRO_COVERED_PATHS:-rust/main/chains/hyperlane-dusk rust/main/Cargo.toml rust/main/Cargo.lock rust/main/hyperlane-base/Cargo.toml rust/main/hyperlane-base/src/settings/chains.rs rust/main/hyperlane-base/src/settings/parser rust/main/hyperlane-base/src/settings/signers.rs rust/main/hyperlane-base/src/contract_sync/cursors/mod.rs rust/main/hyperlane-core/src/chain.rs rust/main/agents/validator/src/reorg_reporter.rs rust/main/lander/src/adapter/chains/factory.rs .github/workflows/dusk-agent-gate.yml .github/workflows/dusk-review-policy-gate.yml .github/workflows/rust-docker.yml .github/workflows/monorepo-docker.yml .github/workflows/rust.yml .github/workflows/test.yml .github/workflows/rebalancer-e2e-test.yml}"
LATEST_REPRO_MONOREPO_REF="${LATEST_REPRO_MONOREPO_REF:-515fab074024271935bc7795604dbb4f0823a937}"
MIN_STATUS_CHECKS="${MIN_STATUS_CHECKS:-2}"
DUSK_REQUIRED_STATUS_CONTEXTS="${DUSK_REQUIRED_STATUS_CONTEXTS:-Dusk review policy gate|Production readiness guard}"
MONOREPO_REQUIRED_STATUS_CONTEXTS="${MONOREPO_REQUIRED_STATUS_CONTEXTS:-Dusk review policy gate|Dusk agent cargo check}"
MONOREPO_COMPARE_VIA_GH="${MONOREPO_COMPARE_VIA_GH:-0}"
MONOREPO_UPSTREAM_REPO="${MONOREPO_UPSTREAM_REPO:-hyperlane-xyz/hyperlane-monorepo}"
MONOREPO_COMPARE_BASE="${MONOREPO_COMPARE_BASE:-main}"
MONOREPO_COMPARE_HEAD="${MONOREPO_COMPARE_HEAD:-dusk-network:feat/dusk-support-v2}"
UPSTREAM_SUBMISSION_REPO="${UPSTREAM_SUBMISSION_REPO:-hyperlane-xyz/hyperlane-monorepo}"
UPSTREAM_SUBMISSION_HEAD="${UPSTREAM_SUBMISSION_HEAD:-dusk-network:feat/dusk-support-v2}"
UPSTREAM_SUBMISSION_GATE_ONLY="${UPSTREAM_SUBMISSION_GATE_ONLY:-0}"
CI_VISIBILITY_GATE_ONLY="${CI_VISIBILITY_GATE_ONLY:-0}"
BRANCH_PROTECTION_GATE_ONLY="${BRANCH_PROTECTION_GATE_ONLY:-0}"
DEPENDENCY_ALERT_GATE_ONLY="${DEPENDENCY_ALERT_GATE_ONLY:-0}"
DEPENDENCY_ALERT_STATUS_SCRIPT="${DEPENDENCY_ALERT_STATUS_SCRIPT:-$ROOT/scripts/dependency-alert-status.sh}"
UPSTREAM_SUBMISSION_SEARCH_JSON="${UPSTREAM_SUBMISSION_SEARCH_JSON:-}"
UPSTREAM_SUBMISSION_INTERNAL_BLOCKERS_OPEN="${UPSTREAM_SUBMISSION_INTERNAL_BLOCKERS_OPEN:-0}"
REQUIRED_SECRET_NAME="${REQUIRED_SECRET_NAME:-DUSK_ORG_READ_TOKEN}"
STATUS_SECRET_NAME="${STATUS_SECRET_NAME:-DUSK_STATUS_READ_TOKEN}"
REQUIRED_RUNNER_LABEL="${REQUIRED_RUNNER_LABEL:-dusk-hyperlane}"
STATUS_CHECK_WAIT_SECONDS="${STATUS_CHECK_WAIT_SECONDS:-60}"
STATUS_CHECK_POLL_SECONDS="${STATUS_CHECK_POLL_SECONDS:-5}"

blockers=()

add_blocker() {
    blockers+=("$1")
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "[FAIL] $1 is required" >&2
        exit 1
    }
}

section() {
    printf '\n== %s ==\n' "$1"
}

pr_field() {
    local repo="$1"
    local number="$2"
    local field="$3"

    gh pr view "$number" --repo "$repo" \
        --json state,mergeable,reviewDecision,statusCheckRollup \
        --jq "$field"
}

pr_state() {
    local repo="$1"
    local number="$2"

    gh api "repos/$repo/pulls/$number" \
        --jq 'if .merged then "MERGED" else (.state | ascii_upcase) end'
}

pr_head_sha() {
    local repo="$1"
    local number="$2"

    gh api "repos/$repo/pulls/$number" --jq .head.sha
}

pr_review_decision() {
    local repo="$1"
    local number="$2"
    local err_file
    local raw

    err_file="/tmp/hyperlane-readiness-review.$$.err"
    if raw="$(gh api "repos/$repo/pulls/$number/reviews?per_page=100" 2>"$err_file")"; then
        printf '%s\n' "$raw" | jq -r '
            if length == 0 then
                "REVIEW_REQUIRED"
            else
                [sort_by(.submitted_at)
                 | group_by(.user.login)
                 | map(.[-1].state)] as $states
                | if any($states[]; . == "CHANGES_REQUESTED") then
                    "CHANGES_REQUESTED"
                  elif any($states[]; . == "APPROVED") then
                    "APPROVED"
                  else
                    "REVIEW_REQUIRED"
                  end
            end
        '
        rm -f "$err_file"
        return 0
    fi

    sed 's/^/  gh: /' "$err_file" >&2
    rm -f "$err_file"
    printf 'UNKNOWN\n'
}

status_rollup() {
    local repo="$1"
    local number="$2"
    local head_sha
    local raw
    local err_file

    head_sha="$(pr_head_sha "$repo" "$number")"
    err_file="/tmp/hyperlane-readiness-check-runs.$$.err"
    if ! raw="$(gh api "repos/$repo/commits/$head_sha/check-runs?per_page=100" 2>"$err_file")"; then
        printf '[]\n'
        sed 's/^/  gh: /' "$err_file" >&2
        rm -f "$err_file"
        return 0
    fi
    rm -f "$err_file"

    if ! printf '%s\n' "$raw" | jq '[.check_runs[] | {
            name,
            status: (.status | ascii_upcase),
            conclusion: ((.conclusion // "") | ascii_upcase),
            detailsUrl: .html_url
        }]'; then
        printf '[]\n'
    fi
}

count_non_completed_checks() {
    local current_run_id="${GITHUB_RUN_ID:-}"

    jq --arg run_id "$current_run_id" '
        [
            .[]
            | select(.status != "COMPLETED")
            | select(
                ($run_id == "")
                or (((.detailsUrl // "") | contains("/actions/runs/" + $run_id + "/")) | not)
            )
        ]
        | length
    '
}

wait_for_status_checks() {
    local label="$1"
    local repo="$2"
    local number="$3"
    local elapsed=0
    local rollup
    local status_count
    local non_completed_count

    while true; do
        rollup="$(status_rollup "$repo" "$number")"
        status_count="$(printf '%s\n' "$rollup" | jq 'length')"
        non_completed_count="$(printf '%s\n' "$rollup" | count_non_completed_checks)"

        if { [ "$status_count" -ge "$MIN_STATUS_CHECKS" ] && [ "$non_completed_count" -eq 0 ]; } ||
            [ "$elapsed" -ge "$STATUS_CHECK_WAIT_SECONDS" ]; then
            printf '%s\n' "$rollup"
            return 0
        fi

        printf '%sStatusChecksWaiting: %s/%s, nonCompleted: %s\n' \
            "$label" "$status_count" "$MIN_STATUS_CHECKS" "$non_completed_count" >&2
        sleep "$STATUS_CHECK_POLL_SECONDS"
        elapsed=$((elapsed + STATUS_CHECK_POLL_SECONDS))
    done
}

check_pr() {
    local label="$1"
    local repo="$2"
    local number="$3"
    local state
    local review_decision
    local status_count
    local non_completed_count
    local rollup

    state="$(pr_state "$repo" "$number")"
    review_decision="$(pr_review_decision "$repo" "$number")"
    rollup="$(wait_for_status_checks "$label" "$repo" "$number")"
    status_count="$(printf '%s\n' "$rollup" | jq 'length')"
    non_completed_count="$(printf '%s\n' "$rollup" | count_non_completed_checks)"

    printf '%sState: %s\n' "$label" "$state"
    printf '%sReviewDecision: %s\n' "$label" "${review_decision:-none}"
    printf '%sStatusChecks: %s\n' "$label" "$status_count"
    printf '%sNonCompletedStatusChecks: %s\n' "$label" "$non_completed_count"

    if [ "$state" != "MERGED" ]; then
        add_blocker "$label PR #$number is $state, not MERGED"
    fi

    if [ "$review_decision" != "APPROVED" ]; then
        add_blocker "$label PR #$number reviewDecision is ${review_decision:-empty}, not APPROVED"
    fi

    if [ "$status_count" -lt "$MIN_STATUS_CHECKS" ]; then
        add_blocker "$label PR #$number has $status_count status checks; expected at least $MIN_STATUS_CHECKS"
    fi

    if [ "$non_completed_count" -gt 0 ]; then
        add_blocker "$label PR #$number has $non_completed_count non-completed status checks"
    fi
}

check_branch_protection() {
    local repo="$1"
    local label="$2"
    local required_contexts="${3:-}"
    local default_branch
    local protection_json
    local status_count
    local requires_reviews
    local err_file
    local context

    default_branch="$(gh api "repos/$repo" --jq .default_branch)"
    printf '%sDefaultBranch: %s\n' "$label" "$default_branch"

    err_file="/tmp/hyperlane-readiness-protection.$$.err"
    if protection_json="$(gh api "repos/$repo/branches/$default_branch/protection" \
        --jq '{requiredStatusChecks: (.required_status_checks.contexts // []), requiresReviews: (.required_pull_request_reviews != null)}' \
        2>"$err_file")"; then
        status_count="$(printf '%s\n' "$protection_json" | jq '.requiredStatusChecks | length')"
        requires_reviews="$(printf '%s\n' "$protection_json" | jq -r .requiresReviews)"
        printf '%sBranchProtection: enabled\n' "$label"
        printf '%sRequiredStatusChecks: %s\n' "$label" "$status_count"
        printf '%sRequiresReviews: %s\n' "$label" "$requires_reviews"

        if [ "$status_count" -lt "$MIN_STATUS_CHECKS" ]; then
            add_blocker "$label default branch has $status_count required status checks; expected at least $MIN_STATUS_CHECKS"
        fi

        if [ -n "$required_contexts" ]; then
            while IFS= read -r context; do
                [ -n "$context" ] || continue
                if ! printf '%s\n' "$protection_json" | jq -e --arg context "$context" \
                    '.requiredStatusChecks | index($context) != null' >/dev/null; then
                    add_blocker "$label default branch is missing required status check: $context"
                fi
            done <<EOF
$(printf '%s\n' "$required_contexts" | tr '|' '\n')
EOF
        fi

        if [ "$requires_reviews" != "true" ]; then
            add_blocker "$label default branch does not require pull request reviews"
        fi
    else
        if grep -q 'Branch not protected' "$err_file"; then
            printf '%sBranchProtection: none\n' "$label"
            add_blocker "$label default branch $default_branch is not protected"
        else
            printf '%sBranchProtection: unknown\n' "$label"
            sed 's/^/  /' "$err_file"
            add_blocker "$label default branch protection visibility is unknown"
        fi
    fi
    rm -f "$err_file"
}

check_upstream_submission_gate() {
    local upstream_query
    local upstream_prs_json
    local upstream_open_prs

    section "Upstream Submission Gate"
    upstream_query="repo:$UPSTREAM_SUBMISSION_REPO is:pr is:open head:$UPSTREAM_SUBMISSION_HEAD"
    printf 'upstreamSubmissionRepo: %s\n' "$UPSTREAM_SUBMISSION_REPO"
    printf 'upstreamSubmissionHead: %s\n' "$UPSTREAM_SUBMISSION_HEAD"

    if [ -n "$UPSTREAM_SUBMISSION_SEARCH_JSON" ]; then
        upstream_prs_json="$UPSTREAM_SUBMISSION_SEARCH_JSON"
    elif ! upstream_prs_json="$(gh api -X GET search/issues -f "q=$upstream_query" 2>/tmp/hyperlane-readiness-upstream-prs.$$.err)"; then
        echo "upstreamOpenPrsFromDuskHead: unknown"
        sed 's/^/  /' /tmp/hyperlane-readiness-upstream-prs.$$.err
        rm -f /tmp/hyperlane-readiness-upstream-prs.$$.err
        add_blocker "upstream Hyperlane PR visibility for $UPSTREAM_SUBMISSION_HEAD is unknown"
        return 0
    fi
    rm -f /tmp/hyperlane-readiness-upstream-prs.$$.err

    if ! upstream_open_prs="$(printf '%s\n' "$upstream_prs_json" | jq -r .total_count)"; then
        echo "upstreamOpenPrsFromDuskHead: unknown"
        add_blocker "upstream Hyperlane PR search response for $UPSTREAM_SUBMISSION_HEAD is invalid"
        return 0
    fi

    printf 'upstreamOpenPrsFromDuskHead: %s\n' "$upstream_open_prs"
    if [ "$upstream_open_prs" -gt 0 ]; then
        printf '%s\n' "$upstream_prs_json" \
            | jq -r '.items[] | "  #\(.number) \(.html_url) \(.title)"'
        if [ "${#blockers[@]}" -gt 0 ] || [ "$UPSTREAM_SUBMISSION_INTERNAL_BLOCKERS_OPEN" = "1" ]; then
            add_blocker "upstream Hyperlane PRs are open from $UPSTREAM_SUBMISSION_HEAD before internal blockers are closed"
        fi
    fi
}

check_ci_visibility() {
    local workflow_output
    local repo_runners_json
    local repo_runners_count
    local repo_runner_has_label
    local org_name
    local org_runners_json
    local org_runners_count
    local org_runner_has_label
    local repo_secrets_json
    local repo_secrets_count
    local required_secret_visible
    local status_secret_visible

    section "Workflow And CI Visibility"
    workflow_output="$(gh workflow list --repo "$DUSK_REPO" --all || true)"
    if [ -z "$workflow_output" ]; then
        echo "workflowVisibility: none"
        add_blocker "no GitHub Actions workflows are visible on the default branch"
    else
        echo "workflowVisibility: present"
    fi

    runner_has_label() {
        jq --arg label "$REQUIRED_RUNNER_LABEL" \
            '[.runners[]? | select(any(.labels[]?; .name == $label))] | length > 0'
    }

    repo_runner_has_label="unknown"
    org_runner_has_label="unknown"

    if repo_runners_json="$(gh api "repos/$DUSK_REPO/actions/runners" 2>/tmp/hyperlane-readiness-runners.$$.err)"; then
        repo_runners_count="$(printf '%s\n' "$repo_runners_json" | jq .total_count)"
        repo_runner_has_label="$(printf '%s\n' "$repo_runners_json" | runner_has_label)"
        printf 'repoSelfHostedRunnersVisible: %s\n' "$repo_runners_count"
        printf 'repoRunnerWithRequiredLabelVisible: %s\n' "$repo_runner_has_label"
    else
        echo "repoSelfHostedRunnersVisible: unknown"
        echo "repoRunnerWithRequiredLabelVisible: unknown"
        sed 's/^/  /' /tmp/hyperlane-readiness-runners.$$.err
    fi
    rm -f /tmp/hyperlane-readiness-runners.$$.err

    org_name="${DUSK_REPO%%/*}"
    if org_runners_json="$(gh api "orgs/$org_name/actions/runners" 2>/tmp/hyperlane-readiness-org-runners.$$.err)"; then
        org_runners_count="$(printf '%s\n' "$org_runners_json" | jq .total_count)"
        org_runner_has_label="$(printf '%s\n' "$org_runners_json" | runner_has_label)"
        printf 'orgSelfHostedRunnersVisible: %s\n' "$org_runners_count"
        printf 'orgRunnerWithRequiredLabelVisible: %s\n' "$org_runner_has_label"
    else
        echo "orgSelfHostedRunnersVisible: unknown"
        echo "orgRunnerWithRequiredLabelVisible: unknown"
        sed 's/^/  /' /tmp/hyperlane-readiness-org-runners.$$.err
    fi
    rm -f /tmp/hyperlane-readiness-org-runners.$$.err

    case "$repo_runner_has_label:$org_runner_has_label" in
        true:*|*:true)
            ;;
        false:false)
            add_blocker "no repo-level or org-level self-hosted runner with label $REQUIRED_RUNNER_LABEL is visible"
            ;;
        false:unknown)
            add_blocker "no repo-level self-hosted runner with label $REQUIRED_RUNNER_LABEL is visible and org runner visibility is unknown"
            ;;
        unknown:false)
            add_blocker "repo-level runner visibility is unknown and no org-level self-hosted runner with label $REQUIRED_RUNNER_LABEL is visible"
            ;;
        *)
            add_blocker "self-hosted runner visibility for label $REQUIRED_RUNNER_LABEL is unknown"
            ;;
    esac

    check_required_secret() {
        local repo="$1"
        local label="$2"
        local err_file="$3"
        local secrets_json
        local secrets_count
        local required_secret_visible

        if secrets_json="$(gh api "repos/$repo/actions/secrets" 2>"$err_file")"; then
            secrets_count="$(printf '%s\n' "$secrets_json" | jq .total_count)"
            required_secret_visible="$(printf '%s\n' "$secrets_json" | jq --arg name "$REQUIRED_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
            printf '%sSecretsVisible: %s\n' "$label" "$secrets_count"
            printf '%sRequiredSecretVisible: %s\n' "$label" "$required_secret_visible"
            if [ "$required_secret_visible" != "true" ]; then
                add_blocker "$label Actions secret $REQUIRED_SECRET_NAME is not visible"
            fi
        else
            printf '%sSecretsVisible: unknown\n' "$label"
            printf '%sRequiredSecretVisible: unknown\n' "$label"
            sed 's/^/  /' "$err_file"
            add_blocker "$label Actions secret visibility is unknown"
        fi
        rm -f "$err_file"
    }

    if repo_secrets_json="$(gh api "repos/$DUSK_REPO/actions/secrets" 2>/tmp/hyperlane-readiness-secrets.$$.err)"; then
        repo_secrets_count="$(printf '%s\n' "$repo_secrets_json" | jq .total_count)"
        required_secret_visible="$(printf '%s\n' "$repo_secrets_json" | jq --arg name "$REQUIRED_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
        status_secret_visible="$(printf '%s\n' "$repo_secrets_json" | jq --arg name "$STATUS_SECRET_NAME" '[.secrets[]?.name] | index($name) != null')"
        printf 'repoSecretsVisible: %s\n' "$repo_secrets_count"
        printf 'repoRequiredSecretVisible: %s\n' "$required_secret_visible"
        printf 'repoStatusSecretVisible: %s\n' "$status_secret_visible"
        if [ "$required_secret_visible" != "true" ]; then
            add_blocker "repo-level Actions secret $REQUIRED_SECRET_NAME is not visible"
        fi
    else
        echo "repoSecretsVisible: unknown"
        echo "repoRequiredSecretVisible: unknown"
        echo "repoStatusSecretVisible: unknown"
        sed 's/^/  /' /tmp/hyperlane-readiness-secrets.$$.err
        add_blocker "repo-level Actions secret visibility is unknown"
    fi
    rm -f /tmp/hyperlane-readiness-secrets.$$.err

    check_required_secret "$MONOREPO_REPO" "monorepoRepo" "/tmp/hyperlane-readiness-monorepo-secrets.$$.err"
}

check_dependency_alerts() {
    local dependency_alert_status=0
    local dependency_alert_output

    section "Dependency Alerts"
    dependency_alert_output="$(mktemp)"
    bash "$DEPENDENCY_ALERT_STATUS_SCRIPT" --summary-only 2>&1 \
        | tee "$dependency_alert_output" || dependency_alert_status=$?
    if [ "$dependency_alert_status" -ne 0 ]; then
        if grep -q '^dependencyAlertStatus: unavailable' "$dependency_alert_output"; then
            add_blocker "Dusk Cargo.lock dependency-alert triage is unavailable"
        else
            add_blocker "Dusk Cargo.lock dependency-alert triage has vulnerable, unparsed, or unpatchable open alerts"
        fi
    fi
    rm -f "$dependency_alert_output"
}

print_summary_and_exit() {
    section "Summary"
    if [ "${#blockers[@]}" -eq 0 ]; then
        echo "productionReadinessGuard: passed"
        echo "Known machine-checkable blockers are closed, but Dusk reviewer judgment and fresh release evidence still apply."
        exit 0
    fi

    echo "productionReadinessGuard: blocked"
    for blocker in "${blockers[@]}"; do
        printf -- '- %s\n' "$blocker"
    done
    exit 1
}

require_cmd gh
require_cmd git
require_cmd jq

if [ "$UPSTREAM_SUBMISSION_GATE_ONLY" = "1" ]; then
    check_upstream_submission_gate
    print_summary_and_exit
fi

if [ "$CI_VISIBILITY_GATE_ONLY" = "1" ]; then
    check_ci_visibility
    print_summary_and_exit
fi

if [ "$BRANCH_PROTECTION_GATE_ONLY" = "1" ]; then
    section "Branch Protection"
    check_branch_protection "$DUSK_REPO" "dusk" "$DUSK_REQUIRED_STATUS_CONTEXTS"
    check_branch_protection "$MONOREPO_REPO" "monorepo" "$MONOREPO_REQUIRED_STATUS_CONTEXTS"
    print_summary_and_exit
fi

if [ "$DEPENDENCY_ALERT_GATE_ONLY" = "1" ]; then
    check_dependency_alerts
    print_summary_and_exit
fi

section "Internal PRs"
check_pr "dusk" "$DUSK_REPO" 1
check_pr "monorepo" "$MONOREPO_REPO" 1

if gh pr view "$WORKFLOW_PR_NUMBER" --repo "$DUSK_REPO" --json state >/dev/null 2>&1; then
    check_pr "workflowDispatcher" "$DUSK_REPO" "$WORKFLOW_PR_NUMBER"
else
    add_blocker "workflow dispatcher PR #$WORKFLOW_PR_NUMBER is missing or inaccessible"
fi

section "Branch Protection"
check_branch_protection "$DUSK_REPO" "dusk" "$DUSK_REQUIRED_STATUS_CONTEXTS"
check_branch_protection "$MONOREPO_REPO" "monorepo" "$MONOREPO_REQUIRED_STATUS_CONTEXTS"

section "Production Sign-Off"
issue_body="$(gh issue view 2 --repo "$DUSK_REPO" --json body --jq .body)"
unchecked_count="$(printf '%s\n' "$issue_body" | grep -c '^- \[ \]' || true)"
checked_count="$(printf '%s\n' "$issue_body" | grep -c '^- \[x\]' || true)"
printf 'uncheckedItems: %s\n' "$unchecked_count"
printf 'checkedItems: %s\n' "$checked_count"
if [ "$unchecked_count" -gt 0 ]; then
    add_blocker "production sign-off issue #2 has $unchecked_count unchecked checklist items"
fi

section "Split Decision Issues"
open_split_issues=0
for issue in $SIGNOFF_ISSUES; do
    state="$(gh issue view "$issue" --repo "$DUSK_REPO" --json state --jq .state)"
    printf '#%s: %s\n' "$issue" "$state"
    if [ "$state" = "OPEN" ]; then
        open_split_issues=$((open_split_issues + 1))
    fi
done
printf 'openSplitIssues: %s\n' "$open_split_issues"
if [ "$open_split_issues" -gt 0 ]; then
    add_blocker "$open_split_issues split production decision issues remain open"
fi

check_dependency_alerts

check_ci_visibility

section "Freshness"
if [ "$MONOREPO_COMPARE_VIA_GH" = "1" ]; then
    if compare_json="$(gh api "repos/$MONOREPO_UPSTREAM_REPO/compare/$MONOREPO_COMPARE_BASE...$MONOREPO_COMPARE_HEAD" 2>/tmp/hyperlane-readiness-compare.$$.err)"; then
        upstream_head="$(printf '%s\n' "$compare_json" | jq -r .base_commit.sha)"
        merge_base="$(printf '%s\n' "$compare_json" | jq -r .merge_base_commit.sha)"
        ahead_count="$(printf '%s\n' "$compare_json" | jq -r .ahead_by)"
        behind_count="$(printf '%s\n' "$compare_json" | jq -r .behind_by)"
        ahead_behind="$ahead_count	$behind_count"
        printf 'upstreamHead: %s\n' "$upstream_head"
        printf 'mergeBase: %s\n' "$merge_base"
        printf 'aheadBehind: %s\n' "$ahead_behind"
        if [ "$behind_count" != "0" ]; then
            add_blocker "monorepo branch is behind $MONOREPO_UPSTREAM_REPO/$MONOREPO_COMPARE_BASE by $behind_count commits"
        fi
    else
        echo "compareApi: unavailable"
        sed 's/^/  /' /tmp/hyperlane-readiness-compare.$$.err
        add_blocker "monorepo upstream freshness compare is unavailable"
    fi
    rm -f /tmp/hyperlane-readiness-compare.$$.err
elif [ -d "$MONOREPO_DIR" ] && git -C "$MONOREPO_DIR" remote get-url "$UPSTREAM_REMOTE" >/dev/null 2>&1; then
    git -C "$MONOREPO_DIR" fetch "$UPSTREAM_REMOTE" main
    upstream_ref="$UPSTREAM_REMOTE/main"
    merge_base="$(git -C "$MONOREPO_DIR" merge-base HEAD "$upstream_ref")"
    upstream_head="$(git -C "$MONOREPO_DIR" rev-parse "$upstream_ref")"
    ahead_behind="$(git -C "$MONOREPO_DIR" rev-list --left-right --count HEAD..."$upstream_ref")"
    behind_count="$(printf '%s\n' "$ahead_behind" | awk '{print $2}')"
    printf 'upstreamHead: %s\n' "$upstream_head"
    printf 'mergeBase: %s\n' "$merge_base"
    printf 'aheadBehind: %s\n' "$ahead_behind"
    if [ "$behind_count" != "0" ]; then
        add_blocker "monorepo branch is behind $UPSTREAM_REMOTE/main by $behind_count commits"
    fi
else
    add_blocker "monorepo checkout or $UPSTREAM_REMOTE remote is unavailable"
fi

if git -C "$ROOT" rev-parse --verify "$LATEST_REPRO_DUSK_REF^{commit}" >/dev/null 2>&1; then
    covered_delta="$(git -C "$ROOT" diff --name-only "$LATEST_REPRO_DUSK_REF"..HEAD -- $DUSK_REPRO_COVERED_PATHS)"
    if [ -n "$covered_delta" ]; then
        echo "coveredPathDelta: present"
        printf '%s\n' "$covered_delta" | sed 's/^/  /'
        add_blocker "runtime/test covered paths changed since latest clean-layout repro"
    else
        echo "coveredPathDelta: none"
    fi
else
    add_blocker "latest clean-layout repro ref $LATEST_REPRO_DUSK_REF is unavailable"
fi

check_monorepo_repro_delta() {
    local all_delta="$1"
    local covered_delta=""
    local changed_path
    local covered_path

    while IFS= read -r changed_path; do
        [ -n "$changed_path" ] || continue
        for covered_path in $MONOREPO_REPRO_COVERED_PATHS; do
            if [ "$changed_path" = "$covered_path" ] || [[ "$changed_path" == "$covered_path/"* ]]; then
                covered_delta="${covered_delta}${changed_path}"$'\n'
                break
            fi
        done
    done <<<"$all_delta"

    if [ -n "$covered_delta" ]; then
        echo "monorepoCoveredPathDelta: present"
        printf '%s' "$covered_delta" | sed 's/^/  /'
        add_blocker "monorepo runtime/agent/CI covered paths changed since latest clean-layout repro"
    else
        echo "monorepoCoveredPathDelta: none"
    fi
}

github_commit_tree_sha() {
    local repo="$1"
    local ref="$2"

    gh api "repos/$repo/commits/$ref" --jq .commit.tree.sha
}

github_monorepo_covered_manifest() {
    local repo="$1"
    local ref="$2"
    local output_path="$3"
    local err_file="$4"
    local tree_sha
    local tree_json
    local truncated

    tree_sha="$(github_commit_tree_sha "$repo" "$ref" 2>"$err_file")" || return 1
    tree_json="$(gh api "repos/$repo/git/trees/$tree_sha?recursive=1" 2>"$err_file")" || return 1
    truncated="$(printf '%s\n' "$tree_json" | jq -r '.truncated')"
    if [ "$truncated" = "true" ]; then
        printf '  GitHub tree API response for %s was truncated\n' "$ref" >"$err_file"
        return 1
    fi

    printf '%s\n' "$tree_json" \
        | jq -r '.tree[] | select(.type == "blob") | [.path, .sha] | @tsv' \
        | while IFS=$'\t' read -r path sha; do
            for covered_path in $MONOREPO_REPRO_COVERED_PATHS; do
                if [ "$path" = "$covered_path" ] || [[ "$path" == "$covered_path/"* ]]; then
                    printf '%s\t%s\n' "$path" "$sha"
                    break
                fi
            done
        done \
        | LC_ALL=C sort >"$output_path"
}

github_monorepo_covered_delta() {
    local base_ref="$1"
    local head_ref="$2"
    local err_file="$3"
    local base_manifest
    local head_manifest
    local delta

    base_manifest="$(mktemp)"
    head_manifest="$(mktemp)"
    if ! github_monorepo_covered_manifest "$MONOREPO_REPO" "$base_ref" "$base_manifest" "$err_file"; then
        rm -f "$base_manifest" "$head_manifest"
        return 1
    fi
    if ! github_monorepo_covered_manifest "$MONOREPO_REPO" "$head_ref" "$head_manifest" "$err_file"; then
        rm -f "$base_manifest" "$head_manifest"
        return 1
    fi

    delta="$(comm -3 "$base_manifest" "$head_manifest" | sed $'s/^\t//' | cut -f1 | LC_ALL=C sort -u)"
    rm -f "$base_manifest" "$head_manifest"
    printf '%s\n' "$delta"
}

if [ "$MONOREPO_COMPARE_VIA_GH" = "1" ]; then
    monorepo_delta_head="${MONOREPO_COMPARE_HEAD#*:}"
    if monorepo_delta="$(github_monorepo_covered_delta "$LATEST_REPRO_MONOREPO_REF" "$monorepo_delta_head" /tmp/hyperlane-readiness-monorepo-delta.$$.err)"; then
        check_monorepo_repro_delta "$monorepo_delta"
    else
        sed 's/^/  /' /tmp/hyperlane-readiness-monorepo-delta.$$.err
        add_blocker "monorepo latest clean-layout repro covered-tree compare is unavailable"
    fi
    rm -f /tmp/hyperlane-readiness-monorepo-delta.$$.err
elif [ -d "$MONOREPO_DIR" ] && git -C "$MONOREPO_DIR" rev-parse --verify "$LATEST_REPRO_MONOREPO_REF^{commit}" >/dev/null 2>&1; then
    monorepo_covered_delta="$(git -C "$MONOREPO_DIR" diff --name-only "$LATEST_REPRO_MONOREPO_REF"..HEAD -- $MONOREPO_REPRO_COVERED_PATHS)"
    if [ -n "$monorepo_covered_delta" ]; then
        echo "monorepoCoveredPathDelta: present"
        printf '%s\n' "$monorepo_covered_delta" | sed 's/^/  /'
        add_blocker "monorepo runtime/agent/CI covered paths changed since latest clean-layout repro"
    else
        echo "monorepoCoveredPathDelta: none"
    fi
else
    add_blocker "latest clean-layout monorepo repro ref $LATEST_REPRO_MONOREPO_REF is unavailable"
fi

check_upstream_submission_gate
print_summary_and_exit
