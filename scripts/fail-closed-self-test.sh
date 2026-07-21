#!/usr/bin/env bash
# Regression tests for local guard paths that must fail closed on scan errors.

set -euo pipefail

ROOT="${ROOT_OVERRIDE:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$ROOT"

fail() {
    echo "[FAIL] $*" >&2
    exit 1
}

info() {
    echo "[INFO] $*" >&2
}

command -v tar >/dev/null 2>&1 || fail "tar is required"
command -v rg >/dev/null 2>&1 || fail "rg is required"
command -v jq >/dev/null 2>&1 || fail "jq is required"

saved_version_checks=(
    'validate_dusk_state_version "$dusk_mailbox" "Dusk Mailbox" 2'
    'validate_dusk_state_version "$dusk_test_mock" "Dusk TestMock"'
    'validate_dusk_state_version "$dusk_ism_multisig" "Dusk multisig ISM"'
    'validate_dusk_state_version "$dusk_merkle" "Dusk MerkleTreeHook"'
    'validate_dusk_state_version "$dusk_warp" "Dusk synthetic warp route" 3'
    'validate_dusk_state_version "$dusk_warp_native" "Dusk native warp route"'
    'validate_dusk_state_version "$dusk_warp_collateral" "Dusk collateral warp route" 2'
    'validate_dusk_state_version "$dusk_validator_announce" "Dusk ValidatorAnnounce"'
    'validate_dusk_state_version "$dusk_igp" "Dusk IGP" 2'
    'validate_dusk_state_version "$dusk_protocol_fee" "Dusk ProtocolFee"'
    'validate_dusk_state_version "$dusk_aggregation_hook" "Dusk AggregationHook"'
    'validate_dusk_state_version "$dusk_test_recipient" "Dusk test recipient"'
)
for check in "${saved_version_checks[@]}"; do
    rg -q -F "$check" demo/deploy.sh \
        || fail "saved-deployment validation omits required contract state version: $check"
done

rg -q -F 'live_igp_config="$(query_dusk_domain_gas_config' demo/deploy.sh \
    || fail "saved-deployment validation omits live IGP pricing"
saved_hook_type_checks=(
    'query_dusk_u8 "$dusk_merkle" hook_type "Dusk MerkleTreeHook hook type")" = "3"'
    'query_dusk_u8 "$dusk_aggregation_hook" hook_type "Dusk AggregationHook hook type")" = "2"'
    'query_dusk_u8 "$dusk_igp" hook_type "Dusk IGP hook type")" = "4"'
    'query_dusk_u8 "$dusk_protocol_fee" hook_type "Dusk ProtocolFee hook type")" = "6"'
)
for check in "${saved_hook_type_checks[@]}"; do
    rg -q -F "$check" demo/deploy.sh \
        || fail "saved-deployment validation has a missing or incorrect hook type check: $check"
done
rg -q -F 'per-chain artifacts are not a trusted reuse boundary' demo/deploy.sh \
    || fail "--skip-deploy can fall back to per-chain artifacts"
rg -q -F 'bash "$SCRIPT_DIR/deploy.sh" --skip-deploy' demo/demo.sh \
    || fail "standalone demo does not delegate warm validation"
rg -q -F 'bash "$SCRIPT_DIR/deploy.sh" --skip-deploy' demo/gen-agent-configs.sh \
    || fail "agent config generation does not delegate live validation"
rg -q -F '"key": "${ANVIL_RELAYER_PRIVATE_KEY}"' demo/gen-agent-configs.sh \
    || fail "relayer config does not use its isolated EVM signer"
[ "$(rg -c -F '"key": "${ANVIL_VALIDATOR_PRIVATE_KEY}"' demo/gen-agent-configs.sh)" -eq 3 ] \
    || fail "validator configs do not consistently use the isolated validator identity"
rg -q -F -- '--multisig-validators "$E2E_ANVIL_VALIDATOR"' demo/e2e-agents.sh \
    || fail "live multisig E2E does not deploy against the isolated validator identity"
rg -q -F -- '--constructor-args "[$MULTISIG_VALIDATORS]" "$MULTISIG_THRESHOLD"' demo/deploy.sh \
    || fail "EVM and Dusk multisig deployments do not consume the same validator policy"
rg -q -F 'ANVIL_RELAYER_PRIVATE_KEY="$E2E_ANVIL_RELAYER_PRIVATE_KEY"' demo/e2e-agents.sh \
    || fail "live E2E does not pass the isolated relayer signer to config generation"
rg -q -F 'ANVIL_VALIDATOR_PRIVATE_KEY="$E2E_ANVIL_VALIDATOR_PRIVATE_KEY"' demo/e2e-agents.sh \
    || fail "live E2E does not pass the isolated validator signer to config generation"
rg -q -F 'require_merged=0' scripts/production-readiness-guard.sh \
    || fail "pre-merge readiness still requires an already merged PR"
rg -q -F 'require_approved=0' scripts/production-readiness-guard.sh \
    || fail "pre-merge readiness still requires prior reviewer approval"
rg -q -F 'require_merged=1' scripts/production-readiness-guard.sh \
    || fail "production readiness no longer requires merged PRs"
rg -q -F 'require_approved=1' scripts/production-readiness-guard.sh \
    || fail "production readiness no longer requires approved PRs"
rg -q -F 'WORKFLOW_REQUIRED_STATUS_CONTEXTS="${WORKFLOW_REQUIRED_STATUS_CONTEXTS:-Dusk review policy gate|Manual repro dispatcher gate}"' scripts/production-readiness-guard.sh \
    || fail "workflow dispatcher readiness uses the wrong required status contexts"

validator_line="$(rg -n -F 'bash "$SCRIPT_DIR/deploy.sh" --skip-deploy' demo/gen-agent-configs.sh | cut -d: -f1 | head -1)"
secret_write_line="$(rg -n -F "printf '0x%s\\n'" demo/gen-agent-configs.sh | cut -d: -f1 | head -1)"
[ -n "$validator_line" ] && [ -n "$secret_write_line" ] && [ "$validator_line" -lt "$secret_write_line" ] \
    || fail "agent config live validation must precede signer material writes"

workdir="$(mktemp -d -t hyperlane-fail-closed-test.XXXXXX)"
untracked_probe=""
cleanup() {
    chmod -R u+rwX "$workdir" 2>/dev/null || true
    rm -rf "$workdir"
    if [ -n "$untracked_probe" ]; then
        rm -f -- "$untracked_probe"
    fi
}
trap cleanup EXIT

expect_fail() {
    local label="$1"
    local pattern="$2"
    shift 2
    local log="$workdir/$label.log"

    if "$@" >"$log" 2>&1; then
        cat "$log" >&2
        fail "$label: expected command to fail"
    fi

    if ! rg -q "$pattern" "$log"; then
        cat "$log" >&2
        fail "$label: failure did not include expected pattern: $pattern"
    fi

    info "$label: failed closed as expected"
}

expect_pass() {
    local label="$1"
    shift
    local log="$workdir/$label.log"

    if ! "$@" >"$log" 2>&1; then
        cat "$log" >&2
        fail "$label: expected command to pass"
    fi

    info "$label: passed as expected"
}

mkdir -p "$workdir/archive/src" "$workdir/archive/archives"
printf 'safe log\n' >"$workdir/archive/src/log.txt"
tar -czf "$workdir/archive/archives/safe.tgz" -C "$workdir/archive/src" .

expect_fail \
    archive-invalid-pattern \
    'archive member path scan failed' \
    env ARCHIVE_SHA256_MANIFEST='' ARCHIVE_EXPECTED_SHA256S='' ARCHIVE_UNSAFE_MEMBER_PATTERN='[invalid' \
    bash scripts/archive-hygiene-check.sh "$workdir/archive/archives"

expect_fail \
    gate-status-invalid-placeholder-pattern \
    'Dusk repo runtime placeholder scan failed' \
    env DUSK_PLACEHOLDER_PATTERN='[invalid' \
    bash scripts/release-gate-status.sh --placeholder-scan-only

expect_fail \
    gate-status-invalid-contract-unwrap-pattern \
    'Dusk contract direct unwrap scan failed' \
    env DUSK_CONTRACT_UNWRAP_PATTERN='[invalid' \
    bash scripts/release-gate-status.sh --placeholder-scan-only

expect_fail \
    production-readiness-premature-upstream-pr \
    'upstream Hyperlane PRs are open' \
    env UPSTREAM_SUBMISSION_GATE_ONLY=1 \
        UPSTREAM_SUBMISSION_INTERNAL_BLOCKERS_OPEN=1 \
        UPSTREAM_SUBMISSION_SEARCH_JSON='{"total_count":1,"items":[{"number":1,"html_url":"https://github.com/hyperlane-xyz/hyperlane-monorepo/pull/1","title":"Premature Dusk upstream PR"}]}' \
    bash scripts/production-readiness-guard.sh

mkdir -p "$workdir/ci-visibility-mock-bin"
cat >"$workdir/ci-visibility-mock-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "$*" in
    "workflow list --repo dusk-network/hyperlane-dusk --all")
        if [ "${GH_MOCK_NO_WORKFLOWS:-0}" = "1" ]; then
            exit 0
        fi
        printf 'Dusk Review Policy Gate active 1\n'
        ;;
    "api repos/dusk-network/hyperlane-dusk/actions/runners")
        printf '{"total_count":0,"runners":[]}\n'
        ;;
    "api orgs/dusk-network/actions/runners")
        printf '{"total_count":0,"runners":[]}\n'
        ;;
    "api repos/dusk-network/hyperlane-dusk/actions/secrets")
        printf '{"total_count":0,"secrets":[]}\n'
        ;;
    "api repos/dusk-network/hyperlane-monorepo/actions/secrets")
        printf '{"total_count":0,"secrets":[]}\n'
        ;;
    *)
        echo "unexpected gh invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$workdir/ci-visibility-mock-bin/gh"

expect_fail \
    production-readiness-missing-ci-runner \
    'no repo-level or org-level self-hosted runner with label dusk-hyperlane is visible' \
    env PATH="$workdir/ci-visibility-mock-bin:$PATH" CI_VISIBILITY_GATE_ONLY=1 \
    bash scripts/production-readiness-guard.sh

expect_fail \
    production-readiness-missing-workflow-visibility \
    'no GitHub Actions workflows are visible on the default branch' \
    env PATH="$workdir/ci-visibility-mock-bin:$PATH" CI_VISIBILITY_GATE_ONLY=1 GH_MOCK_NO_WORKFLOWS=1 \
    bash scripts/production-readiness-guard.sh

expect_fail \
    production-readiness-missing-ci-secret \
    'repo-level Actions secret DUSK_ORG_READ_TOKEN is not visible' \
    env PATH="$workdir/ci-visibility-mock-bin:$PATH" CI_VISIBILITY_GATE_ONLY=1 \
    bash scripts/production-readiness-guard.sh

mkdir -p "$workdir/branch-protection-mock-bin"
cat >"$workdir/branch-protection-mock-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "$*" in
    "api repos/dusk-network/hyperlane-dusk --jq .default_branch" | \
    "api repos/dusk-network/hyperlane-monorepo --jq .default_branch")
        printf 'main\n'
        exit 0
        ;;
    "api repos/dusk-network/hyperlane-dusk/branches/main/protection --jq "* | \
    "api repos/dusk-network/hyperlane-monorepo/branches/main/protection --jq "*)
        echo "Branch not protected" >&2
        exit 1
        ;;
    *)
        echo "unexpected gh invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$workdir/branch-protection-mock-bin/gh"

expect_fail \
    production-readiness-missing-branch-protection \
    'dusk default branch main is not protected' \
    env PATH="$workdir/branch-protection-mock-bin:$PATH" BRANCH_PROTECTION_GATE_ONLY=1 \
    bash scripts/production-readiness-guard.sh

mkdir -p "$workdir/branch-protection-one-check-mock-bin"
cat >"$workdir/branch-protection-one-check-mock-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "$*" in
    "api repos/dusk-network/hyperlane-dusk --jq .default_branch" | \
    "api repos/dusk-network/hyperlane-monorepo --jq .default_branch")
        printf 'main\n'
        ;;
    "api repos/dusk-network/hyperlane-dusk/branches/main/protection --jq "* | \
    "api repos/dusk-network/hyperlane-monorepo/branches/main/protection --jq "*)
        printf '{"requiredStatusChecks":["Dusk review policy gate"],"strictStatusChecks":true,"requiresReviews":true}\n'
        ;;
    *)
        echo "unexpected gh invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$workdir/branch-protection-one-check-mock-bin/gh"

expect_fail \
    production-readiness-insufficient-required-checks \
    'dusk default branch has 1 required status checks; expected at least 2' \
    env PATH="$workdir/branch-protection-one-check-mock-bin:$PATH" BRANCH_PROTECTION_GATE_ONLY=1 \
    bash scripts/production-readiness-guard.sh

mkdir -p "$workdir/branch-protection-wrong-check-mock-bin"
cat >"$workdir/branch-protection-wrong-check-mock-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "$*" in
    "api repos/dusk-network/hyperlane-dusk --jq .default_branch" | \
    "api repos/dusk-network/hyperlane-monorepo --jq .default_branch")
        printf 'main\n'
        ;;
    "api repos/dusk-network/hyperlane-dusk/branches/main/protection --jq "* | \
    "api repos/dusk-network/hyperlane-monorepo/branches/main/protection --jq "*)
        printf '{"requiredStatusChecks":["Dusk review policy gate","Unrelated CI"],"strictStatusChecks":true,"requiresReviews":true}\n'
        ;;
    *)
        echo "unexpected gh invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$workdir/branch-protection-wrong-check-mock-bin/gh"

expect_fail \
    production-readiness-missing-required-check-context \
    'dusk default branch is missing required status check: Production readiness guard' \
    env PATH="$workdir/branch-protection-wrong-check-mock-bin:$PATH" BRANCH_PROTECTION_GATE_ONLY=1 \
    bash scripts/production-readiness-guard.sh

mkdir -p "$workdir/branch-protection-strict-mock-bin"
cat >"$workdir/branch-protection-strict-mock-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "$*" in
    "api repos/dusk-network/hyperlane-dusk --jq .default_branch" | \
    "api repos/dusk-network/hyperlane-monorepo --jq .default_branch")
        printf 'main\n'
        exit 0
        ;;
    "api repos/dusk-network/hyperlane-dusk/branches/main/protection --jq "*)
        contexts='["Dusk review policy gate","Production readiness guard"]'
        ;;
    "api repos/dusk-network/hyperlane-monorepo/branches/main/protection --jq "*)
        contexts='["Dusk review policy gate","Dusk agent validation"]'
        ;;
    *)
        echo "unexpected gh invocation: $*" >&2
        exit 1
        ;;
esac

case "${GH_MOCK_STRICT_MODE:-false}" in
    false)
        printf '{"requiredStatusChecks":%s,"strictStatusChecks":false,"requiresReviews":true}\n' "$contexts"
        ;;
    missing)
        printf '{"requiredStatusChecks":%s,"requiresReviews":true}\n' "$contexts"
        ;;
    *)
        echo "unexpected GH_MOCK_STRICT_MODE" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$workdir/branch-protection-strict-mock-bin/gh"

expect_fail \
    production-readiness-nonstrict-status-checks \
    'dusk default branch does not require branches to be up to date before merging' \
    env PATH="$workdir/branch-protection-strict-mock-bin:$PATH" BRANCH_PROTECTION_GATE_ONLY=1 \
        GH_MOCK_STRICT_MODE=false \
    bash scripts/production-readiness-guard.sh

expect_fail \
    production-readiness-missing-strict-status-checks \
    'dusk default branch does not require branches to be up to date before merging' \
    env PATH="$workdir/branch-protection-strict-mock-bin:$PATH" BRANCH_PROTECTION_GATE_ONLY=1 \
        GH_MOCK_STRICT_MODE=missing \
    bash scripts/production-readiness-guard.sh

mkdir -p "$workdir/status-check-mock-bin"
cat >"$workdir/status-check-mock-bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "$*" in
    "api repos/dusk-network/hyperlane-dusk/pulls/1 --jq .head.sha")
        printf '1111111111111111111111111111111111111111\n'
        ;;
    "api repos/dusk-network/hyperlane-dusk/pulls/1 --jq if "*)
        printf 'OPEN\n'
        ;;
    "api repos/dusk-network/hyperlane-dusk/pulls/1/reviews?per_page=100")
        printf '[]\n'
        ;;
    "api --paginate repos/dusk-network/hyperlane-dusk/commits/1111111111111111111111111111111111111111/check-runs?per_page=100 --jq "*)
        printf '%s\n' '{"name":"Dusk proposal validation","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://example.test/first"}'
        if [ "${GH_MOCK_MISSING_EXACT:-0}" = "1" ]; then
            printf '%s\n' '{"name":"Production readiness guard / lookalike","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://example.test/lookalike"}'
        else
            # This record represents a required context only present on the
            # second paginated response.
            printf '%s\n' '{"name":"Production readiness guard","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://example.test/second-page"}'
        fi
        ;;
    *)
        echo "unexpected gh invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$workdir/status-check-mock-bin/gh"

expect_pass \
    production-readiness-paginates-required-checks \
    env PATH="$workdir/status-check-mock-bin:$PATH" STATUS_CHECK_GATE_ONLY=1 \
        READINESS_MODE=premerge STATUS_CHECK_WAIT_SECONDS=0 \
    bash scripts/production-readiness-guard.sh

expect_fail \
    production-readiness-rejects-lookalike-check-name \
    'missing required status checks: Production readiness guard' \
    env PATH="$workdir/status-check-mock-bin:$PATH" STATUS_CHECK_GATE_ONLY=1 \
        READINESS_MODE=premerge STATUS_CHECK_WAIT_SECONDS=0 GH_MOCK_MISSING_EXACT=1 \
    bash scripts/production-readiness-guard.sh

expect_fail \
    production-readiness-production-rejects-proposal-only \
    'missing required status checks: Dusk review policy gate' \
    env PATH="$workdir/status-check-mock-bin:$PATH" STATUS_CHECK_GATE_ONLY=1 \
        READINESS_MODE=production STATUS_CHECK_WAIT_SECONDS=0 \
    bash scripts/production-readiness-guard.sh

agent_state="$workdir/agent-state.json"
agent_id_a="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
agent_id_b="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
jq -n \
    --arg a "$agent_id_a" \
    '{
        evm: {mailbox:$a, token:$a, igp:$a, validator_announce:$a, merkle_tree_hook:$a},
        dusk: {
            mailbox:$a, igp:$a, validator_announce:$a, merkle_tree_hook:$a,
            test_mock:$a, ism_multisig:"", default_ism:$a
        },
        evm_domain:31338, dusk_domain:4242, dusk_chain_id:"00",
        dusk_default_ism:"testMock"
    }' >"$agent_state"

cp demo/gen-agent-configs.sh "$workdir/gen-agent-configs.sh"
chmod +x "$workdir/gen-agent-configs.sh"
env AGENT_CONFIG_VALIDATE_ONLY=1 BRIDGE_STATE_FILE="$agent_state" \
    bash "$workdir/gen-agent-configs.sh" --ism testMock --run-id validation >/dev/null \
    || fail "matching saved agent policy should validate"

expect_fail \
    agent-config-requested-ism-mismatch \
    'does not match deployed Dusk Mailbox policy' \
    env AGENT_CONFIG_VALIDATE_ONLY=1 BRIDGE_STATE_FILE="$agent_state" \
    bash demo/gen-agent-configs.sh --ism messageIdMultisig --run-id validation

jq --arg b "$agent_id_b" '.dusk.default_ism = $b' "$agent_state" >"$workdir/agent-state-wrong-id.json"
expect_fail \
    agent-config-default-ism-id-mismatch \
    'does not match TestMock' \
    env AGENT_CONFIG_VALIDATE_ONLY=1 BRIDGE_STATE_FILE="$workdir/agent-state-wrong-id.json" \
    bash demo/gen-agent-configs.sh --ism testMock --run-id validation

jq '.dusk_chain_id = "0000"' "$agent_state" >"$workdir/agent-state-wrong-chain.json"
expect_fail \
    agent-config-chain-id-mismatch \
    'invalid Dusk chain ID' \
    env AGENT_CONFIG_VALIDATE_ONLY=1 BRIDGE_STATE_FILE="$workdir/agent-state-wrong-chain.json" \
    bash demo/gen-agent-configs.sh --ism testMock --run-id validation

dependency_alert_unavailable="$workdir/dependency-alert-unavailable.sh"
cat >"$dependency_alert_unavailable" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "dependencyAlertStatus: unavailable"
exit 1
EOF
chmod +x "$dependency_alert_unavailable"

expect_fail \
    production-readiness-dependency-alert-unavailable \
    'Dusk Cargo.lock dependency-alert triage is unavailable' \
    env DEPENDENCY_ALERT_GATE_ONLY=1 DEPENDENCY_ALERT_STATUS_SCRIPT="$dependency_alert_unavailable" \
    bash scripts/production-readiness-guard.sh

expect_fail \
    production-readiness-covered-test-delta \
    'runtime/test covered paths changed since latest clean-layout repro' \
    env REPRO_DELTA_GATE_ONLY=1 \
        LATEST_REPRO_DUSK_REF=77fdeae8b6813fdbfb26d03593125a57c0bb458c \
        DUSK_REPRO_COVERED_PATHS=tests/tests/integration.rs \
    bash scripts/production-readiness-guard.sh

agent_scan_repo="$workdir/agent-scan-repo"
mkdir -p "$agent_scan_repo/rust/main/chains/hyperlane-dusk/src"
git -C "$agent_scan_repo" init -q
printf 'pub fn scan_fixture() {}\n' \
    >"$agent_scan_repo/rust/main/chains/hyperlane-dusk/src/lib.rs"
git -C "$agent_scan_repo" add rust/main/chains/hyperlane-dusk/src/lib.rs

expect_fail \
    review-hygiene-invalid-agent-pattern \
    'Dusk agent runtime panic/placeholder scan failed' \
    env MONOREPO_DIR="$agent_scan_repo" AGENT_PLACEHOLDER_PATTERN='[invalid' \
    bash scripts/github-review-hygiene.sh --agent-placeholder-scan-only

stale_dispatcher_comments="$workdir/stale-dispatcher-comments.txt"
cat >"$stale_dispatcher_comments" <<'EOF'
COMMENT_ID=1
Latest clean-layout repro evidence: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443963744
---END---
EOF
expect_fail \
    review-hygiene-stale-dispatcher-evidence \
    'stale dispatcher clean-layout repro evidence' \
    bash scripts/github-review-hygiene.sh --dispatcher-comment-scan-only "$stale_dispatcher_comments"

expect_fail \
    review-hygiene-invalid-dispatcher-pattern \
    'stale dispatcher comments scan failed' \
    env STALE_DISPATCHER_REPRO_PATTERN='[invalid' \
    bash scripts/github-review-hygiene.sh --dispatcher-comment-scan-only "$stale_dispatcher_comments"

existing_export="$workdir/existing-review-export"
mkdir "$existing_export"
printf 'preserve me\n' >"$existing_export/sentinel.txt"
expect_fail \
    review-hygiene-existing-export-directory \
    'must name a path that does not already exist' \
    bash scripts/github-review-hygiene.sh --export-dir "$existing_export" --no-keep
[ "$(cat "$existing_export/sentinel.txt")" = 'preserve me' ] \
    || fail "review-hygiene-existing-export-directory: caller data was modified"

expect_fail \
    report-hygiene-invalid-pattern \
    'report hygiene scan failed' \
    env STALE_REPORT_PATTERNS='[invalid' \
    bash scripts/report-hygiene-check.sh

expect_fail \
    report-hygiene-invalid-archive-verification-mode \
    'VERIFY_LATEST_REPRO_ARCHIVE must be 0 or 1' \
    env VERIFY_LATEST_REPRO_ARCHIVE=invalid \
    bash scripts/report-hygiene-check.sh

stale_report="$workdir/stale-report.md"
cat >"$stale_report" <<'EOF'
Stale dispatcher smoke evidence:
- implementation head ae937e79e60af2d1c6f02e303be8956bddf6a133
- log path /tmp/hyperlane-merge-order-logs.eGzQ7Q
EOF
expect_fail \
    report-hygiene-stale-dispatcher-evidence \
    'stale local report evidence found' \
    env REPORT_FILES="$stale_report" \
    bash scripts/report-hygiene-check.sh

stale_goal_audit="$workdir/stale-goal-audit.md"
cat >"$stale_goal_audit" <<'EOF'
Dispatcher smoke evidence:
- head under test `f656de36ea7b7099ab3a1eabcb89b672d9eb9c4d`
- Latest command logs from the refreshed review-gates bundle were written under `/tmp/hyperlane-merge-order-logs.QUAn6t`.
EOF
expect_fail \
    report-hygiene-stale-goal-audit-moving-evidence \
    'stale moving goal-audit evidence found' \
    env REPORT_FILES="$stale_goal_audit" GOAL_AUDIT_FILE="$stale_goal_audit" \
    bash scripts/report-hygiene-check.sh

stale_decision_record="$workdir/stale-production-decisions.md"
cat >"$stale_decision_record" <<'EOF'
Latest clean-layout repro evidence at
https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443963744.
EOF
expect_fail \
    report-hygiene-stale-production-decision-evidence \
    'stale production decision evidence found' \
    env REPORT_FILES="$stale_decision_record" \
        GOAL_AUDIT_FILE="$stale_decision_record" \
        DECISION_RECORD_FILE="$stale_decision_record" \
    bash scripts/report-hygiene-check.sh

tmp_only_repro_report="$workdir/tmp-only-repro-report.md"
report_archive="$workdir/report-archive.tgz"
printf 'hermetic report archive fixture\n' >"$report_archive"
report_archive_hash="$(sha256sum "$report_archive" | awk '{print $1}')"
cat >"$tmp_only_repro_report" <<'EOF'
Latest clean-layout repro evidence:
- log: /tmp/hyperlane-clean-repro-current-head-1778751867.log
- SHA256: df88d712f4bfada0b1958a9b4d7c1b96b0b2ec4754482c524dca91b0c83e738d
EOF
expect_fail \
    report-hygiene-missing-latest-repro-archive \
    'latest repro durable archive missing' \
    env REPORT_FILES="$tmp_only_repro_report" \
        LATEST_REPRO_ARCHIVE_PATH="$report_archive" \
        LATEST_REPRO_ARCHIVE_SHA256="$report_archive_hash" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$tmp_only_repro_report" \
        GOAL_AUDIT_FILE="$tmp_only_repro_report" \
    bash scripts/report-hygiene-check.sh

path_only_repro_report="$workdir/path-only-repro-report.md"
cat >"$path_only_repro_report" <<EOF
Latest clean-layout repro evidence:
- archive: $report_archive
- log SHA256: df88d712f4bfada0b1958a9b4d7c1b96b0b2ec4754482c524dca91b0c83e738d
EOF
expect_fail \
    report-hygiene-missing-latest-repro-archive-hash \
    'latest repro durable archive hash missing' \
    env REPORT_FILES="$path_only_repro_report" \
        LATEST_REPRO_ARCHIVE_PATH="$report_archive" \
        LATEST_REPRO_ARCHIVE_SHA256="$report_archive_hash" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$path_only_repro_report" \
        GOAL_AUDIT_FILE="$path_only_repro_report" \
    bash scripts/report-hygiene-check.sh

missing_archive_report="$workdir/missing-archive-report.md"
missing_archive_path="$workdir/missing-archive.tgz"
missing_archive_hash="0000000000000000000000000000000000000000000000000000000000000000"
cat >"$missing_archive_report" <<EOF
Latest clean-layout repro evidence:
- archive: $missing_archive_path
- archive SHA256: $missing_archive_hash
EOF
expect_fail \
    report-hygiene-missing-latest-repro-archive-file \
    'latest repro durable archive file not found' \
    env REPORT_FILES="$missing_archive_report" \
        LATEST_REPRO_ARCHIVE_PATH="$missing_archive_path" \
        LATEST_REPRO_ARCHIVE_SHA256="$missing_archive_hash" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$missing_archive_report" \
        GOAL_AUDIT_FILE="$missing_archive_report" \
    bash scripts/report-hygiene-check.sh

hash_mismatch_report="$workdir/hash-mismatch-report.md"
hash_mismatch_archive="$workdir/hash-mismatch.tgz"
printf 'not the expected archive\n' >"$hash_mismatch_archive"
hash_mismatch_expected="1111111111111111111111111111111111111111111111111111111111111111"
cat >"$hash_mismatch_report" <<EOF
Latest clean-layout repro evidence:
- archive: $hash_mismatch_archive
- archive SHA256: $hash_mismatch_expected
EOF
expect_fail \
    report-hygiene-latest-repro-archive-hash-mismatch \
    'latest repro durable archive hash mismatch' \
    env REPORT_FILES="$hash_mismatch_report" \
        LATEST_REPRO_ARCHIVE_PATH="$hash_mismatch_archive" \
        LATEST_REPRO_ARCHIVE_SHA256="$hash_mismatch_expected" \
        LATEST_REPRO_ARCHIVE_REQUIRED_FILES="$hash_mismatch_report" \
        GOAL_AUDIT_FILE="$hash_mismatch_report" \
    bash scripts/report-hygiene-check.sh

mkdir -p "$workdir/env-artifacts"
printf 'local env placeholder\n' >"$workdir/env-artifacts/.env.bridge"
expect_fail \
    secret-hygiene-env-artifact \
    'runtime artifact path includes secret-like file names' \
    bash scripts/secret-hygiene-check.sh "$workdir/env-artifacts"

mkdir -p "$workdir/private-key-artifacts"
cat >"$workdir/private-key-artifacts/config.json" <<'EOF'
{"signer":{"privateKey":"0x1111111111111111111111111111111111111111111111111111111111111111"}}
EOF
expect_fail \
    secret-hygiene-private-key-artifact \
    'runtime artifact scan found' \
    bash scripts/secret-hygiene-check.sh "$workdir/private-key-artifacts"

mkdir -p "$workdir/toml-key-artifacts"
cat >"$workdir/toml-key-artifacts/config.toml" <<'EOF'
[signer]
private_key = "0x2222222222222222222222222222222222222222222222222222222222222222"
EOF
expect_fail \
    secret-hygiene-toml-private-key-artifact \
    'runtime artifact scan found' \
    bash scripts/secret-hygiene-check.sh "$workdir/toml-key-artifacts"

mkdir -p "$workdir/single-quote-key-artifacts"
cat >"$workdir/single-quote-key-artifacts/config.yaml" <<'EOF'
signer:
  privateKey: '0x3333333333333333333333333333333333333333333333333333333333333333'
EOF
expect_fail \
    secret-hygiene-single-quote-private-key-artifact \
    'runtime artifact scan found' \
    bash scripts/secret-hygiene-check.sh "$workdir/single-quote-key-artifacts"

mkdir -p "$workdir/env-key-artifacts"
cat >"$workdir/env-key-artifacts/runner.log" <<'EOF'
DUSK_SIGNER_KEY=0x4444444444444444444444444444444444444444444444444444444444444444
EOF
expect_fail \
    secret-hygiene-env-private-key-artifact \
    'runtime artifact scan found' \
    bash scripts/secret-hygiene-check.sh "$workdir/env-key-artifacts"

mkdir -p "$workdir/secret-artifacts"
printf 'safe log\n' >"$workdir/secret-artifacts/unreadable.log"
chmod 000 "$workdir/secret-artifacts/unreadable.log"
expect_fail \
    secret-hygiene-unreadable-artifact \
    'runtime artifact secret text scan failed' \
    bash scripts/secret-hygiene-check.sh "$workdir/secret-artifacts"

untracked_probe="$(mktemp "$ROOT/.completion-audit-untracked-probe.XXXXXX")"
printf 'temporary completion audit probe\n' >"$untracked_probe"
expect_fail \
    completion-audit-untracked-source \
    'dusk has untracked source paths' \
    env MONOREPO_DIR="$agent_scan_repo" UNTRACKED_SOURCE_GATE_ONLY=1 \
    bash scripts/completion-audit-status.sh
rm -f -- "$untracked_probe"
untracked_probe=""

info "Fail-closed self-test passed"
