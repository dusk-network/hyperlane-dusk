#!/usr/bin/env bash
# =============================================================================
# E2E: Duplicate Relayer Attempt
# =============================================================================
#
# Runs two independent relayer instances against the same EVM -> Dusk message,
# verifies the message is delivered once, and then verifies Dusk token supply
# remains stable rather than double-minting.

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

TIMEOUT_SECS="${TIMEOUT_SECS:-300}"
STABILITY_SECS="${STABILITY_SECS:-30}"
AMOUNT_TO_DUSK="${AMOUNT_TO_DUSK:-1}"

RELAYER_A_PID=""
RELAYER_B_PID=""

require_tools() {
    command -v jq >/dev/null 2>&1 || fail "jq not found"
    command -v cast >/dev/null 2>&1 || fail "cast not found (foundry)"
    command -v forge >/dev/null 2>&1 || fail "forge not found (foundry)"
    command -v python3 >/dev/null 2>&1 || fail "python3 not found"
}

to_wei() {
    local amount="$1"
    if ! [[ "$amount" =~ ^[1-9][0-9]*$ ]]; then
        fail "Invalid amount '$amount' (expected whole positive integer)"
    fi
    echo "${amount}000000000000000000"
}

kill_pid() {
    local pid="$1"
    if [ -n "${pid:-}" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        for _ in 1 2 3 4 5; do
            kill -0 "$pid" 2>/dev/null || return 0
            sleep 1
        done
        kill -9 "$pid" 2>/dev/null || true
    fi
}

cleanup() {
    kill_pid "$RELAYER_A_PID"
    kill_pid "$RELAYER_B_PID"
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
}

trap cleanup EXIT

tail_logs_on_fail() {
    local relayer_a_log="$1"
    local relayer_b_log="$2"
    echo "" >&2
    echo "=== relayer A log (tail) ===" >&2
    tail -n 160 "$relayer_a_log" 2>/dev/null || true
    echo "" >&2
    echo "=== relayer B log (tail) ===" >&2
    tail -n 160 "$relayer_b_log" 2>/dev/null || true
}

query_dusk_supply() {
    local dusk_warp="$1"
    "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
        --contract "$dusk_warp" --method total_supply --return-type u64 \
        2>/dev/null | jq -r '.value // 0'
}

start_relayer() {
    local cfg="$1"
    local log="$2"
    local pid_var="$3"

    (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
      exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$cfg" ./target/debug/relayer \
      >"$log" 2>&1) &
    printf -v "$pid_var" "%s" "$!"
    sleep 2
    local pid="${!pid_var}"
    kill -0 "$pid" 2>/dev/null || {
        tail -n 160 "$log" >&2 || true
        fail "relayer failed to start: $log"
    }
}

assert_relayers_alive() {
    local relayer_a_log="$1"
    local relayer_b_log="$2"
    kill -0 "$RELAYER_A_PID" 2>/dev/null || {
        tail_logs_on_fail "$relayer_a_log" "$relayer_b_log"
        fail "relayer A exited unexpectedly"
    }
    kill -0 "$RELAYER_B_PID" 2>/dev/null || {
        tail_logs_on_fail "$relayer_a_log" "$relayer_b_log"
        fail "relayer B exited unexpectedly"
    }
}

wait_for_dusk_supply() {
    local dusk_warp="$1"
    local expected="$2"
    local relayer_a_log="$3"
    local relayer_b_log="$4"
    local start_ts now supply

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_a_log" "$relayer_b_log"
            fail "timeout waiting for Dusk supply $expected"
        fi
        assert_relayers_alive "$relayer_a_log" "$relayer_b_log"
        supply="$(query_dusk_supply "$dusk_warp")"
        if [ "$supply" = "$expected" ]; then
            return 0
        fi
        sleep 5
    done
}

assert_supply_stable() {
    local dusk_warp="$1"
    local expected="$2"
    local relayer_a_log="$3"
    local relayer_b_log="$4"
    local start_ts now supply

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -ge "$STABILITY_SECS" ]; then
            return 0
        fi
        assert_relayers_alive "$relayer_a_log" "$relayer_b_log"
        supply="$(query_dusk_supply "$dusk_warp")"
        if [ "$supply" != "$expected" ]; then
            tail_logs_on_fail "$relayer_a_log" "$relayer_b_log"
            fail "Dusk supply changed after first delivery: expected $expected, got $supply"
        fi
        sleep 5
    done
}

require_tools

run_id="$(date +%s)"
start_env_log="/tmp/hyperlane-duplicate-relayer-start-testMock-${run_id}.log"
deploy_log="/tmp/hyperlane-duplicate-relayer-deploy-testMock-${run_id}.log"
relayer_a_log="/tmp/hyperlane-duplicate-relayer-a-testMock-${run_id}.log"
relayer_b_log="/tmp/hyperlane-duplicate-relayer-b-testMock-${run_id}.log"

info "Starting fresh local environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true bash "$SCRIPT_DIR/start-env.sh" \
    >"$start_env_log" 2>&1 || {
        tail -n 200 "$start_env_log" >&2 || true
        fail "start-env.sh failed (log: $start_env_log)"
    }

info "Deploying TestMock environment..."
bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism testMock >"$deploy_log" 2>&1 || {
    tail -n 200 "$deploy_log" >&2 || true
    fail "deploy.sh failed (log: $deploy_log)"
}

cfg_json="$(bash "$SCRIPT_DIR/gen-agent-configs.sh" --ism testMock --run-id "$run_id")"
relayer_cfg="$(echo "$cfg_json" | jq -r '.relayer')"
relayer_a_cfg="/tmp/hyperlane-relayer-duplicate-a-testMock-${run_id}.json"
relayer_b_cfg="/tmp/hyperlane-relayer-duplicate-b-testMock-${run_id}.json"

jq \
  --arg db "/tmp/hyperlane-db-relayer-duplicate-a-testMock-${run_id}" \
  '.db = $db | .metricsPort = 19092' \
  "$relayer_cfg" > "$relayer_a_cfg"

jq \
  --arg db "/tmp/hyperlane-db-relayer-duplicate-b-testMock-${run_id}" \
  '.db = $db | .metricsPort = 19094' \
  "$relayer_cfg" > "$relayer_b_cfg"

info "Building relayer binary..."
(cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && cargo build -p relayer >/dev/null)

state="$BRIDGE_STATE_FILE"
evm_token="$(jq -r '.evm.token' "$state")"
dusk_warp="$(jq -r '.dusk.warp_drc20' "$state")"
account_h256="$(jq -r '.account_h256' "$state")"
dusk_domain="$(jq -r '.dusk_domain' "$state")"

amount_wei="$(to_wei "$AMOUNT_TO_DUSK")"
dusk_supply_before="$(query_dusk_supply "$dusk_warp")"
expected_dusk_supply="$(python3 - <<PY
print(int(${dusk_supply_before}) + int(${amount_wei}))
PY
)"

info "Starting relayer A..."
start_relayer "$relayer_a_cfg" "$relayer_a_log" RELAYER_A_PID

info "Starting relayer B..."
start_relayer "$relayer_b_cfg" "$relayer_b_log" RELAYER_B_PID

info "Dispatching EVM -> Dusk ($AMOUNT_TO_DUSK wDUSK)..."
cast send "$evm_token" \
    "transferRemote(uint32,bytes32,uint256)" \
    "$dusk_domain" "0x${account_h256}" "$amount_wei" \
    --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null

info "Waiting for first delivery..."
wait_for_dusk_supply "$dusk_warp" "$expected_dusk_supply" "$relayer_a_log" "$relayer_b_log"

info "Verifying supply remains stable for ${STABILITY_SECS}s with both relayers running..."
assert_supply_stable "$dusk_warp" "$expected_dusk_supply" "$relayer_a_log" "$relayer_b_log"

info "Stopping relayers..."
kill_pid "$RELAYER_A_PID"
kill_pid "$RELAYER_B_PID"
RELAYER_A_PID=""
RELAYER_B_PID=""

info "Stopping environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true

info "Duplicate relayer attempt did not double-deliver."
info "Logs:"
info "  start:     $start_env_log"
info "  deploy:    $deploy_log"
info "  relayer A: $relayer_a_log"
info "  relayer B: $relayer_b_log"
