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
DUSK_REPRO_COVERED_PATHS="${DUSK_REPRO_COVERED_PATHS:-contracts types data-driver dusk-tx e2e wasm-bindings demo Cargo.toml Cargo.lock}"
LATEST_REPRO_DUSK_REF="${LATEST_REPRO_DUSK_REF:-93ed3b07b26d8784610d2ba754a851490de15e21}"
MIN_STATUS_CHECKS="${MIN_STATUS_CHECKS:-1}"
MONOREPO_COMPARE_VIA_GH="${MONOREPO_COMPARE_VIA_GH:-0}"
MONOREPO_UPSTREAM_REPO="${MONOREPO_UPSTREAM_REPO:-hyperlane-xyz/hyperlane-monorepo}"
MONOREPO_COMPARE_BASE="${MONOREPO_COMPARE_BASE:-main}"
MONOREPO_COMPARE_HEAD="${MONOREPO_COMPARE_HEAD:-dusk-network:feat/dusk-support-v2}"
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

status_rollup() {
    local repo="$1"
    local number="$2"

    gh pr view "$number" --repo "$repo" \
        --json statusCheckRollup \
        --jq .statusCheckRollup
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
    local non_completed_count

    while true; do
        rollup="$(status_rollup "$repo" "$number")"
        non_completed_count="$(printf '%s\n' "$rollup" | count_non_completed_checks)"

        if [ "$non_completed_count" -eq 0 ] || [ "$elapsed" -ge "$STATUS_CHECK_WAIT_SECONDS" ]; then
            printf '%s\n' "$rollup"
            return 0
        fi

        printf '%sNonCompletedStatusChecksWaiting: %s\n' "$label" "$non_completed_count"
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

    state="$(pr_field "$repo" "$number" .state)"
    review_decision="$(pr_field "$repo" "$number" '.reviewDecision // ""')"
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
    local default_branch
    local protection_json
    local status_count
    local requires_reviews
    local err_file

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

require_cmd gh
require_cmd git
require_cmd jq

section "Internal PRs"
check_pr "dusk" "$DUSK_REPO" 1
check_pr "monorepo" "$MONOREPO_REPO" 1

if gh pr view "$WORKFLOW_PR_NUMBER" --repo "$DUSK_REPO" --json state >/dev/null 2>&1; then
    check_pr "workflowDispatcher" "$DUSK_REPO" "$WORKFLOW_PR_NUMBER"
else
    add_blocker "workflow dispatcher PR #$WORKFLOW_PR_NUMBER is missing or inaccessible"
fi

section "Branch Protection"
check_branch_protection "$DUSK_REPO" "dusk"
check_branch_protection "$MONOREPO_REPO" "monorepo"

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

section "Dependency Alerts"
dependency_alert_status=0
dependency_alert_output="$(mktemp)"
bash "$ROOT/scripts/dependency-alert-status.sh" --summary-only 2>&1 \
    | tee "$dependency_alert_output" || dependency_alert_status=$?
if [ "$dependency_alert_status" -ne 0 ]; then
    if grep -q '^dependencyAlertStatus: unavailable' "$dependency_alert_output"; then
        add_blocker "Dusk Cargo.lock dependency-alert triage is unavailable"
    else
        add_blocker "Dusk Cargo.lock dependency-alert triage has vulnerable, unparsed, or unpatchable open alerts"
    fi
fi
rm -f "$dependency_alert_output"

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
