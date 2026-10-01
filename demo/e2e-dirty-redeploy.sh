#!/usr/bin/env bash
# =============================================================================
# E2E: Dirty Redeploy Guard
# =============================================================================
#
# Proves that deterministic Dusk contract IDs are not silently reused when
# deploy-hyperlane is run against a non-reset local chain.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

ISM="${ISM:-testMock}"
TIMEOUT_SECS="${TIMEOUT_SECS:-240}"

if [ "$ISM" != "testMock" ] && [ "$ISM" != "messageIdMultisig" ]; then
    fail "Invalid ISM '$ISM' (expected: testMock or messageIdMultisig)"
fi

cleanup() {
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
}

trap cleanup EXIT

run_id="$(date +%s)"
start_log="/tmp/hyperlane-dirty-redeploy-start-${ISM}-${run_id}.log"
deploy_first_log="/tmp/hyperlane-dirty-redeploy-first-${ISM}-${run_id}.log"
deploy_second_log="/tmp/hyperlane-dirty-redeploy-second-${ISM}-${run_id}.log"

info "Starting fresh local environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true TIMEOUT_SECS="$TIMEOUT_SECS" \
    bash "$SCRIPT_DIR/start-env.sh" >"$start_log" 2>&1 || {
        tail -n 200 "$start_log" >&2 || true
        fail "start-env.sh failed (log: $start_log)"
    }

info "Running initial deployment..."
if [ "$ISM" = "messageIdMultisig" ]; then
    bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism messageIdMultisig \
        --multisig-validators "$ANVIL_DEPLOYER" --multisig-threshold 1 \
        >"$deploy_first_log" 2>&1 || {
            tail -n 200 "$deploy_first_log" >&2 || true
            fail "initial deploy failed (log: $deploy_first_log)"
        }
else
    bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism testMock \
        >"$deploy_first_log" 2>&1 || {
            tail -n 200 "$deploy_first_log" >&2 || true
            fail "initial deploy failed (log: $deploy_first_log)"
        }
fi

info "Attempting dirty redeploy without resetting Dusk state..."
set +e
if [ "$ISM" = "messageIdMultisig" ]; then
    bash "$SCRIPT_DIR/deploy.sh" --dusk-ism messageIdMultisig \
        --multisig-validators "$ANVIL_DEPLOYER" --multisig-threshold 1 \
        >"$deploy_second_log" 2>&1
else
    bash "$SCRIPT_DIR/deploy.sh" --dusk-ism testMock >"$deploy_second_log" 2>&1
fi
status=$?
set -e

if [ "$status" -eq 0 ]; then
    tail -n 200 "$deploy_second_log" >&2 || true
    fail "dirty redeploy unexpectedly succeeded (log: $deploy_second_log)"
fi

if ! grep -q "Refusing to deploy: contract IDs already exist on-chain" "$deploy_second_log"; then
    tail -n 200 "$deploy_second_log" >&2 || true
    fail "dirty redeploy failed for the wrong reason (log: $deploy_second_log)"
fi

if ! grep -q "Restart rusk with a fresh state" "$deploy_second_log"; then
    tail -n 200 "$deploy_second_log" >&2 || true
    fail "dirty redeploy refusal did not include recovery guidance (log: $deploy_second_log)"
fi

info "Dirty redeploy correctly refused."
info "Logs:"
info "  start:        $start_log"
info "  first deploy: $deploy_first_log"
info "  second deploy: $deploy_second_log"
