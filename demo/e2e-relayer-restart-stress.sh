#!/usr/bin/env bash
# =============================================================================
# E2E: Relayer Restart and Backlog Stress
# =============================================================================
#
# Exercises repeated relayer-driven transfers and verifies that queued Dusk
# messages are delivered after a relayer restart.
#
# Environment:
# - TRANSFERS: number of transfers in each direction (default: 5).
# - TRANSFER_AMOUNT_WEI: per-transfer amount in wei. Defaults to 1 DUSK.
#   Lower this for high-count runs when the local EVM test balance is limited.
# - TIMEOUT_SECS: wait timeout for each balance/nonce checkpoint.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

TRANSFERS="${TRANSFERS:-5}"
TIMEOUT_SECS="${TIMEOUT_SECS:-300}"
TRANSFER_AMOUNT_WEI="${TRANSFER_AMOUNT_WEI:-}"

CURRENT_RELAYER_PID=""

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

pad_evm_address() {
    local addr="${1#0x}"
    addr="$(echo "$addr" | tr '[:upper:]' '[:lower:]')"
    echo "000000000000000000000000${addr}"
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
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
}

trap cleanup EXIT

tail_relayer_log() {
    local log="$1"
    echo "" >&2
    echo "=== relayer log (tail: $log) ===" >&2
    tail -n 160 "$log" 2>/dev/null || true
}

start_relayer() {
    local cfg="$1"
    local log="$2"
    local attempt

    for attempt in 1 2 3 4 5; do
        (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
          exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$cfg" ./target/debug/relayer \
          >"$log" 2>&1) &
        CURRENT_RELAYER_PID="$!"
        sleep 2
        if kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null; then
            return 0
        fi

        wait "$CURRENT_RELAYER_PID" 2>/dev/null || true
        CURRENT_RELAYER_PID=""

        if grep -q "Resource temporarily unavailable" "$log"; then
            info "Relayer DB still locked; retrying startup ($attempt/5)..."
            sleep 3
            continue
        fi

        tail_relayer_log "$log"
        fail "relayer failed to start"
    done

    tail_relayer_log "$log"
    fail "relayer failed to start after retries"
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
            tail_relayer_log "$relayer_log"
            fail "timeout waiting for Dusk supply $expected"
        fi
        kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
            tail_relayer_log "$relayer_log"
            fail "relayer exited unexpectedly"
        }
        supply="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
            --contract "$dusk_warp" --method total_supply --return-type u64 \
            2>/dev/null | jq -r '.value // 0')"
        if [ "$supply" = "$expected" ]; then
            return 0
        fi
        sleep 5
    done
}

wait_for_evm_balance() {
    local evm_token="$1"
    local expected="$2"
    local relayer_log="$3"
    local start_ts now balance

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_relayer_log "$relayer_log"
            fail "timeout waiting for EVM balance $expected"
        fi
        kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
            tail_relayer_log "$relayer_log"
            fail "relayer exited unexpectedly"
        }
        balance="$(cast call "$evm_token" "balanceOf(address)(uint256)" \
            "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
        if [ "$balance" = "$expected" ]; then
            return 0
        fi
        sleep 2
    done
}

query_dusk_mailbox_nonce() {
    local dusk_mailbox="$1"
    "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
        --contract "$dusk_mailbox" --method nonce --return-type u32 \
        2>/dev/null | jq -r '.value // 0'
}

wait_for_dusk_mailbox_nonce() {
    local dusk_mailbox="$1"
    local expected="$2"
    local start_ts now nonce

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            fail "timeout waiting for Dusk mailbox nonce $expected"
        fi
        nonce="$(query_dusk_mailbox_nonce "$dusk_mailbox")"
        if [ "$nonce" = "$expected" ]; then
            return 0
        fi
        sleep 5
    done
}

require_tools

if ! [[ "$TRANSFERS" =~ ^[1-9][0-9]*$ ]]; then
    fail "TRANSFERS must be a positive integer"
fi

if [ -n "$TRANSFER_AMOUNT_WEI" ]; then
    if ! [[ "$TRANSFER_AMOUNT_WEI" =~ ^[1-9][0-9]*$ ]]; then
        fail "TRANSFER_AMOUNT_WEI must be a positive integer"
    fi
    amount_wei="$TRANSFER_AMOUNT_WEI"
else
    amount_wei="$(to_wei 1)"
fi
total_wei="$(python3 - <<PY
print(int(${TRANSFERS}) * int(${amount_wei}))
PY
)"

run_id="$(date +%s)"
start_log="/tmp/hyperlane-restart-stress-start-testMock-${run_id}.log"
deploy_log="/tmp/hyperlane-restart-stress-deploy-testMock-${run_id}.log"
relayer_log_1="/tmp/hyperlane-restart-stress-relayer-a-testMock-${run_id}.log"
relayer_log_2="/tmp/hyperlane-restart-stress-relayer-b-testMock-${run_id}.log"
dusk_transfer_log_dir="/tmp/hyperlane-restart-stress-dusk-transfers-testMock-${run_id}"

info "Starting fresh local environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true bash "$SCRIPT_DIR/start-env.sh" \
    >"$start_log" 2>&1 || {
        tail -n 200 "$start_log" >&2 || true
        fail "start-env.sh failed (log: $start_log)"
    }

info "Deploying testMock environment..."
bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism testMock >"$deploy_log" 2>&1 || {
    tail -n 200 "$deploy_log" >&2 || true
    fail "deploy.sh failed (log: $deploy_log)"
}

cfg_json="$(bash "$SCRIPT_DIR/gen-agent-configs.sh" --ism testMock --run-id "$run_id")"
relayer_cfg="$(echo "$cfg_json" | jq -r '.relayer')"

info "Building relayer binary..."
(cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && cargo build -p relayer >/dev/null)

state="$BRIDGE_STATE_FILE"
evm_token="$(jq -r '.evm.token' "$state")"
dusk_warp="$(jq -r '.dusk.warp_drc20' "$state")"
dusk_mailbox="$(jq -r '.dusk.mailbox' "$state")"
account_h256="$(jq -r '.account_h256' "$state")"
evm_domain="$(jq -r '.evm_domain' "$state")"
dusk_domain="$(jq -r '.dusk_domain' "$state")"
evm_recipient_pad32="$(pad_evm_address "$ANVIL_DEPLOYER")"

dusk_supply_before="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
    --contract "$dusk_warp" --method total_supply --return-type u64 \
    2>/dev/null | jq -r '.value // 0')"
evm_balance_before="$(cast call "$evm_token" "balanceOf(address)(uint256)" \
    "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"

info "Starting relayer for EVM -> Dusk burst..."
start_relayer "$relayer_cfg" "$relayer_log_1"

info "Dispatching ${TRANSFERS} EVM -> Dusk transfers (${amount_wei} wei each)..."
for i in $(seq 1 "$TRANSFERS"); do
    cast send "$evm_token" \
      "transferRemote(uint32,bytes32,uint256)" \
      "$dusk_domain" "0x${account_h256}" "$amount_wei" \
      --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null
    info "  EVM -> Dusk transfer $i/$TRANSFERS submitted"
done

expected_dusk_supply="$(python3 - <<PY
print(int(${dusk_supply_before}) + int(${total_wei}))
PY
)"

info "Waiting for Dusk supply after EVM burst: $expected_dusk_supply"
wait_for_dusk_supply "$dusk_warp" "$expected_dusk_supply" "$relayer_log_1"

info "Stopping relayer before Dusk -> EVM burst..."
kill_pid "$CURRENT_RELAYER_PID"
CURRENT_RELAYER_PID=""

evm_balance_mid="$(cast call "$evm_token" "balanceOf(address)(uint256)" \
    "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"

info "Dispatching ${TRANSFERS} Dusk -> EVM transfers while relayer is down (${amount_wei} wei each)..."
mkdir -p "$dusk_transfer_log_dir"
dusk_mailbox_nonce="$(query_dusk_mailbox_nonce "$dusk_mailbox")"
for i in $(seq 1 "$TRANSFERS"); do
    dusk_transfer_log="${dusk_transfer_log_dir}/transfer-${i}.log"
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote \
      --rues-url "$DUSK_RUES_URL" \
      --keys "$CONSENSUS_KEYS" \
      --warp-contract "$dusk_warp" \
      --destination "$evm_domain" \
      --recipient "$evm_recipient_pad32" \
      --amount "$amount_wei" >"$dusk_transfer_log" 2>&1 || {
          cat "$dusk_transfer_log" >&2 || true
          fail "Dusk -> EVM transfer $i/$TRANSFERS failed (log: $dusk_transfer_log)"
      }
    info "  Dusk -> EVM transfer $i/$TRANSFERS submitted"
    dusk_mailbox_nonce="$(python3 - <<PY
print(int(${dusk_mailbox_nonce}) + 1)
PY
)"
    wait_for_dusk_mailbox_nonce "$dusk_mailbox" "$dusk_mailbox_nonce"
done

expected_evm_balance="$(python3 - <<PY
print(int(${evm_balance_mid}) + int(${total_wei}))
PY
)"

info "Restarting relayer with same config/db..."
start_relayer "$relayer_cfg" "$relayer_log_2"

info "Waiting for EVM balance after relayer restart: $expected_evm_balance"
wait_for_evm_balance "$evm_token" "$expected_evm_balance" "$relayer_log_2"

evm_balance_final="$(cast call "$evm_token" "balanceOf(address)(uint256)" \
    "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
dusk_supply_final="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
    --contract "$dusk_warp" --method total_supply --return-type u64 \
    2>/dev/null | jq -r '.value // 0')"

info "Stress result:"
info "  EVM balance before: $evm_balance_before"
info "  EVM balance final:  $evm_balance_final"
info "  Dusk supply before: $dusk_supply_before"
info "  Dusk supply final:  $dusk_supply_final"
info "Logs:"
info "  start:     $start_log"
info "  deploy:    $deploy_log"
info "  relayer A: $relayer_log_1"
info "  relayer B: $relayer_log_2"
info "  dusk txs:  $dusk_transfer_log_dir"
info "CASE OK: relayer restart stress"
