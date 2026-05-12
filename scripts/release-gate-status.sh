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
DUSK_REPRO_COVERED_PATHS="${DUSK_REPRO_COVERED_PATHS:-contracts types data-driver dusk-tx e2e wasm-bindings demo Cargo.toml Cargo.lock}"
LATEST_REPRO_DUSK_REF="${LATEST_REPRO_DUSK_REF:-016eaa89e1afce0ef9a7534fe285d9aa16e26183}"
POST_REBASE_E2E_URL="${POST_REBASE_E2E_URL:-${CURRENT_E2E_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433528683}}"
POST_REBASE_E2E_ARCHIVE_URL="${POST_REBASE_E2E_ARCHIVE_URL:-${CURRENT_E2E_ARCHIVE_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4433564278}}"
DEPENDENCY_REMEDIATED_E2E_URL="${DEPENDENCY_REMEDIATED_E2E_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389}"
LATEST_REPRO_URL="${LATEST_REPRO_URL:-https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434277572}"
CURRENT_GATE_REFRESH_TEXT="${CURRENT_GATE_REFRESH_TEXT:-head and gate refresh:}"
REVIEWER_ROUTING_URL="${REVIEWER_ROUTING_URL:-https://github.com/dusk-network/hyperlane-dusk/blob/feat/dusk-hardening-v2/REVIEWERS.md}"
GATE_STATUS_FRESH_TEXT="${GATE_STATUS_FRESH_TEXT:-make gate-status-fresh}"
DEPENDENCY_ALERT_STATUS_TEXT="${DEPENDENCY_ALERT_STATUS_TEXT:-make dependency-alert-status}"
COMPLETION_AUDIT_STATUS_TEXT="${COMPLETION_AUDIT_STATUS_TEXT:-make completion-audit-status}"
REVIEW_GATES_TEXT="${REVIEW_GATES_TEXT:-make review-gates}"
PRODUCTION_READINESS_GUARD_TEXT="${PRODUCTION_READINESS_GUARD_TEXT:-make production-readiness-guard}"
BRANCH_PROTECTION_STATUS_TEXT="${BRANCH_PROTECTION_STATUS_TEXT:-branch protection/status-check policy}"
REPRO_PATH_DELTA_TEXT="${REPRO_PATH_DELTA_TEXT:-latest clean-layout repro path delta}"
FETCH_UPSTREAM=0

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
  - repo-level Actions secret and self-hosted runner visibility for CI gate #8
  - reviewer-facing evidence, routing, fresh-gate, dependency-alert,
    completion-audit, review-gates, and production-readiness handoff visibility
  - Dusk Dependabot open-alert visibility and local Cargo.lock vulnerable-range
    comparison
  - Hyperlane upstream/main drift for the local monorepo checkout
  - Dusk path delta since the latest clean-layout repro source ref
  - Dusk agent placeholder scan in rust/main/chains/hyperlane-dusk

Options:
  --fetch-upstream     Run git fetch <upstream-remote> main before drift check.
  --monorepo-dir DIR   Hyperlane monorepo checkout.
                       Default: $MONOREPO_DIR
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
  DUSK_REPRO_COVERED_PATHS
                       Space-separated Dusk repo paths whose changes would make
                       the latest clean-layout repro stale for runtime/test
                       source coverage.
                       Default: $DUSK_REPRO_COVERED_PATHS
  LATEST_REPRO_DUSK_REF
                       Dusk source ref covered by the latest clean-layout repro.
                       Default: $LATEST_REPRO_DUSK_REF
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
  PRODUCTION_READINESS_GUARD_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the blocking production readiness guard.
                       Default: $PRODUCTION_READINESS_GUARD_TEXT
  BRANCH_PROTECTION_STATUS_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose the branch protection/status-check gate.
                       Default: $BRANCH_PROTECTION_STATUS_TEXT
  REPRO_PATH_DELTA_TEXT
                       Text expected in active implementation PR and sign-off
                       bodies to expose latest clean-layout repro path delta.
                       Default: $REPRO_PATH_DELTA_TEXT
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --fetch-upstream)
            FETCH_UPSTREAM=1
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
command -v gh >/dev/null 2>&1 || {
    echo "[FAIL] gh is required" >&2
    exit 1
}

if [ -d "$MONOREPO_DIR" ]; then
    MONOREPO_DIR="$(cd "$MONOREPO_DIR" && pwd -P)"
fi

section() {
    printf '\n== %s ==\n' "$1"
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
            "statusChecks: \(.statusCheckRollup | length)"
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
    local default_branch
    local protection_json

    gh api "repos/$repo" \
        --jq '
            "repo: \(.full_name)\n" +
            "defaultBranch: \(.default_branch)\n" +
            "allowMergeCommit: \(.allow_merge_commit)\n" +
            "allowSquashMerge: \(.allow_squash_merge)\n" +
            "allowRebaseMerge: \(.allow_rebase_merge)"
        '

    default_branch="$(gh api "repos/$repo" --jq .default_branch)"
    if protection_json="$(gh api "repos/$repo/branches/$default_branch/protection" --jq '{requiredStatusChecks: (.required_status_checks.contexts // []), requiresReviews: (.required_pull_request_reviews != null)}' 2>/tmp/hyperlane-dusk-protection.$$.err)"; then
        echo "branchProtection: enabled"
        printf '%s\n' "$protection_json" | sed 's/^/  /'
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
print_repo_merge_policy "$DUSK_REPO"
print_repo_merge_policy "$MONOREPO_REPO"

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
    print_reviewer_routing_presence "workflowPR$WORKFLOW_PR_NUMBER" "$workflow_pr_body"
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
if repo_secrets_count="$(gh api "repos/$DUSK_REPO/actions/secrets" --jq .total_count 2>/tmp/hyperlane-dusk-secrets.$$.err)"; then
    echo "repoSecretsVisible: $repo_secrets_count"
else
    echo "repoSecretsVisible: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-secrets.$$.err
fi
rm -f /tmp/hyperlane-dusk-secrets.$$.err

if repo_runners_count="$(gh api "repos/$DUSK_REPO/actions/runners" --jq .total_count 2>/tmp/hyperlane-dusk-runners.$$.err)"; then
    echo "repoSelfHostedRunnersVisible: $repo_runners_count"
else
    echo "repoSelfHostedRunnersVisible: unknown"
    sed 's/^/  /' /tmp/hyperlane-dusk-runners.$$.err
fi
rm -f /tmp/hyperlane-dusk-runners.$$.err

org_name="${DUSK_REPO%%/*}"
if org_runners_count="$(gh api "orgs/$org_name/actions/runners" --jq .total_count 2>/tmp/hyperlane-dusk-org-runners.$$.err)"; then
    echo "orgSelfHostedRunnersVisible: $org_runners_count"
else
    echo "orgSelfHostedRunnersVisible: unknown"
    if [ -n "$org_runners_count" ]; then
        printf '%s\n' "$org_runners_count" | sed 's/^/  /'
    fi
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

section "Runtime Placeholder Scan"
echo "duskRepoPaths: $DUSK_PLACEHOLDER_PATHS"
if git -C "$ROOT" grep -n -E 'todo!|unimplemented!|panic!' -- $DUSK_PLACEHOLDER_PATHS >/tmp/hyperlane-dusk-repo-placeholder-scan.$$; then
    cat /tmp/hyperlane-dusk-repo-placeholder-scan.$$
    rm -f /tmp/hyperlane-dusk-repo-placeholder-scan.$$
else
    rm -f /tmp/hyperlane-dusk-repo-placeholder-scan.$$
    echo "no matches in Dusk repo scoped runtime paths"
fi

if [ -d "$MONOREPO_DIR/rust/main/chains/hyperlane-dusk" ]; then
    if git -C "$MONOREPO_DIR" grep -n -E 'todo!|unimplemented!|panic!' -- rust/main/chains/hyperlane-dusk >/tmp/hyperlane-dusk-placeholder-scan.$$; then
        cat /tmp/hyperlane-dusk-placeholder-scan.$$
        rm -f /tmp/hyperlane-dusk-placeholder-scan.$$
    else
        rm -f /tmp/hyperlane-dusk-placeholder-scan.$$
        echo "no matches in rust/main/chains/hyperlane-dusk"
    fi
else
    echo "missing Dusk chain crate in monorepo checkout"
fi

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

section "Summary"
echo "This script reports machine-checkable gates only."
echo "The objective remains blocked until the unchecked sign-off items, reviews,"
echo "branch protection/status-check policy, CI/default-branch workflow decision,"
echo "internal PR merge, and upstream-prep gate are resolved by Dusk reviewers."
