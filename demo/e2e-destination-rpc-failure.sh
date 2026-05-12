#!/usr/bin/env bash
# =============================================================================
# E2E: Destination RPC Failure Recovery
# =============================================================================
#
# Runs an EVM -> Dusk delivery with the relayer configured to an unreachable
# Dusk RUES URL, verifies the message does not deliver while destination reads
# and submissions are broken, then restarts the relayer with the healthy RUES
# URL and verifies delivery.

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

TIMEOUT_SECS="${TIMEOUT_SECS:-300}"
RPC_FAILURE_SECS="${RPC_FAILURE_SECS:-35}"
AMOUNT_TO_DUSK="${AMOUNT_TO_DUSK:-1}"
BAD_DUSK_RPC="${BAD_DUSK_RPC:-http://127.0.0.1:18080}"

CURRENT_RELAYER_PID=""
GENERATED_DUSK_SIGNER_KEY_FILES=()

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
    kill_pid "$CURRENT_RELAYER_PID"
    if [ "${#GENERATED_DUSK_SIGNER_KEY_FILES[@]}" -gt 0 ]; then
        rm -f "${GENERATED_DUSK_SIGNER_KEY_FILES[@]}" 2>/dev/null || true
    fi
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
}

trap cleanup EXIT

tail_logs_on_fail() {
    local relayer_log="$1"
    echo "" >&2
    echo "=== relayer log (tail) ===" >&2
    tail -n 160 "$relayer_log" 2>/dev/null || true
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

    (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
      exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$cfg" ./target/debug/relayer \
      >"$log" 2>&1) &
    CURRENT_RELAYER_PID="$!"
    sleep 2
    kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
        tail_logs_on_fail "$log"
        fail "relayer failed to start"
    }
}

assert_not_delivered_with_bad_rpc() {
    local dusk_warp="$1"
    local expected="$2"
    local relayer_log="$3"
    local start_ts now supply

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -ge "$RPC_FAILURE_SECS" ]; then
            return 0
        fi
        kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
            tail_logs_on_fail "$relayer_log"
            fail "relayer exited while destination RPC was unreachable"
        }
        supply="$(query_dusk_supply "$dusk_warp")"
        if [ "$supply" = "$expected" ]; then
            fail "message delivered while destination RPC was unreachable"
        fi
        sleep 5
    done
}

wait_for_dusk_supply() {
    local dusk_warp="$1"
    local expected="$2"
    local relayer_log="$3"
    local start_ts now supply

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log"
            fail "timeout waiting for Dusk supply $expected"
        fi
        kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
            tail_logs_on_fail "$relayer_log"
            fail "relayer exited unexpectedly"
        }
        supply="$(query_dusk_supply "$dusk_warp")"
        if [ "$supply" = "$expected" ]; then
            return 0
        fi
        sleep 5
    done
}

require_tools

run_id="$(date +%s)"
start_env_log="/tmp/hyperlane-destination-rpc-failure-start-testMock-${run_id}.log"
deploy_log="/tmp/hyperlane-destination-rpc-failure-deploy-testMock-${run_id}.log"
bad_rpc_relayer_log="/tmp/hyperlane-destination-rpc-failure-relayer-bad-rpc-testMock-${run_id}.log"
healthy_relayer_log="/tmp/hyperlane-destination-rpc-failure-relayer-healthy-testMock-${run_id}.log"

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
generated_dusk_signer_key_file="$(echo "$cfg_json" | jq -r '.duskSignerKeyFile // empty')"
if [ -n "$generated_dusk_signer_key_file" ]; then
    GENERATED_DUSK_SIGNER_KEY_FILES+=("$generated_dusk_signer_key_file")
fi
bad_rpc_relayer_cfg="/tmp/hyperlane-relayer-bad-destination-rpc-testMock-${run_id}.json"
healthy_relayer_cfg="/tmp/hyperlane-relayer-healthy-destination-rpc-testMock-${run_id}.json"

jq \
  --arg rpc "$BAD_DUSK_RPC" \
  --arg db "/tmp/hyperlane-db-relayer-bad-destination-rpc-testMock-${run_id}" \
  '.db = $db | .chains.dusk.rpcUrls[0].http = $rpc' \
  "$relayer_cfg" > "$bad_rpc_relayer_cfg"

jq \
  --arg db "/tmp/hyperlane-db-relayer-healthy-destination-rpc-testMock-${run_id}" \
  '.db = $db' \
  "$relayer_cfg" > "$healthy_relayer_cfg"

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

info "Starting relayer with unreachable destination RPC ($BAD_DUSK_RPC)..."
start_relayer "$bad_rpc_relayer_cfg" "$bad_rpc_relayer_log"

info "Dispatching EVM -> Dusk ($AMOUNT_TO_DUSK wDUSK)..."
cast send "$evm_token" \
    "transferRemote(uint32,bytes32,uint256)" \
    "$dusk_domain" "0x${account_h256}" "$amount_wei" \
    --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null

info "Verifying delivery is blocked for ${RPC_FAILURE_SECS}s with bad destination RPC..."
assert_not_delivered_with_bad_rpc "$dusk_warp" "$expected_dusk_supply" "$bad_rpc_relayer_log"

info "Restarting relayer with healthy destination RPC..."
kill_pid "$CURRENT_RELAYER_PID"
CURRENT_RELAYER_PID=""
start_relayer "$healthy_relayer_cfg" "$healthy_relayer_log"

info "Waiting for delivery after destination RPC recovery..."
wait_for_dusk_supply "$dusk_warp" "$expected_dusk_supply" "$healthy_relayer_log"

info "Stopping relayer..."
kill_pid "$CURRENT_RELAYER_PID"
CURRENT_RELAYER_PID=""

info "Stopping environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true

info "Destination RPC failure flow recovered after healthy relayer restart."
info "Logs:"
info "  start:           $start_env_log"
info "  deploy:          $deploy_log"
info "  bad rpc relayer: $bad_rpc_relayer_log"
info "  healthy relayer: $healthy_relayer_log"
