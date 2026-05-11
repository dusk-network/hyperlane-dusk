#!/usr/bin/env bash
# =============================================================================
# E2E: Corrupt MessageIdMultisig Checkpoint Metadata
# =============================================================================
#
# Creates a valid validator checkpoint for an EVM -> Dusk message, corrupts the
# local checkpoint signature before the relayer can consume it, verifies delivery
# remains blocked, restores the valid checkpoint, and verifies delivery resumes.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

TIMEOUT_SECS="${TIMEOUT_SECS:-420}"
CORRUPT_METADATA_SECS="${CORRUPT_METADATA_SECS:-35}"
AMOUNT_TO_DUSK="${AMOUNT_TO_DUSK:-1}"

CURRENT_RELAYER_PID=""
CURRENT_VALIDATOR_PID=""

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
    kill_pid "$CURRENT_VALIDATOR_PID"
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
}

trap cleanup EXIT

tail_logs_on_fail() {
    local relayer_log="$1"
    local validator_log="${2:-}"
    echo "" >&2
    echo "=== relayer log (tail) ===" >&2
    tail -n 160 "$relayer_log" 2>/dev/null || true
    if [ -n "$validator_log" ]; then
        echo "" >&2
        echo "=== validator log (tail) ===" >&2
        tail -n 160 "$validator_log" 2>/dev/null || true
    fi
}

start_validator() {
    local cfg="$1"
    local log="$2"

    (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
      exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$cfg" ./target/debug/validator \
      >"$log" 2>&1) &
    CURRENT_VALIDATOR_PID="$!"
    sleep 2
    kill -0 "$CURRENT_VALIDATOR_PID" 2>/dev/null || {
        tail_logs_on_fail /dev/null "$log"
        fail "validator failed to start"
    }
}

start_relayer() {
    local cfg="$1"
    local log="$2"
    local validator_log="$3"

    (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
      exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$cfg" ./target/debug/relayer \
      >"$log" 2>&1) &
    CURRENT_RELAYER_PID="$!"
    sleep 2
    kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
        tail_logs_on_fail "$log" "$validator_log"
        fail "relayer failed to start"
    }
}

query_dusk_supply() {
    local dusk_warp="$1"
    "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
        --contract "$dusk_warp" --method total_supply --return-type u64 \
        2>/dev/null | jq -r '.value // 0'
}

wait_for_checkpoint() {
    local checkpoint_file="$1"
    local validator_log="$2"
    local start_ts now

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail /dev/null "$validator_log"
            fail "timeout waiting for checkpoint file $checkpoint_file"
        fi
        kill -0 "$CURRENT_VALIDATOR_PID" 2>/dev/null || {
            tail_logs_on_fail /dev/null "$validator_log"
            fail "validator exited before checkpoint was written"
        }
        if [ -s "$checkpoint_file" ] && jq -e '.signature.r and .signature.s and .signature.v' "$checkpoint_file" >/dev/null; then
            return 0
        fi
        sleep 2
    done
}

corrupt_checkpoint_signature() {
    local checkpoint_file="$1"
    local tmp_file="${checkpoint_file}.tmp"

    jq '.signature.r = "0x1" |
        .signature.s = "0x1" |
        .signature.v = 27 |
        .serialized_signature = "0x000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000011b"' \
        "$checkpoint_file" > "$tmp_file"
    mv "$tmp_file" "$checkpoint_file"
}

assert_not_delivered_while_corrupt() {
    local dusk_warp="$1"
    local expected="$2"
    local relayer_log="$3"
    local validator_log="$4"
    local start_ts now supply

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -ge "$CORRUPT_METADATA_SECS" ]; then
            return 0
        fi
        kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
            tail_logs_on_fail "$relayer_log" "$validator_log"
            fail "relayer exited while checkpoint metadata was corrupt"
        }
        supply="$(query_dusk_supply "$dusk_warp")"
        if [ "$supply" = "$expected" ]; then
            fail "message delivered with corrupt checkpoint metadata"
        fi
        sleep 5
    done
}

wait_for_dusk_supply() {
    local dusk_warp="$1"
    local expected="$2"
    local relayer_log="$3"
    local validator_log="$4"
    local start_ts now supply

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "$validator_log"
            fail "timeout waiting for Dusk supply $expected"
        fi
        kill -0 "$CURRENT_RELAYER_PID" 2>/dev/null || {
            tail_logs_on_fail "$relayer_log" "$validator_log"
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
start_env_log="/tmp/hyperlane-corrupt-metadata-start-messageIdMultisig-${run_id}.log"
deploy_log="/tmp/hyperlane-corrupt-metadata-deploy-messageIdMultisig-${run_id}.log"
relayer_log="/tmp/hyperlane-corrupt-metadata-relayer-messageIdMultisig-${run_id}.log"
validator_log="/tmp/hyperlane-corrupt-metadata-validator-messageIdMultisig-${run_id}.log"

info "Starting fresh local environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true bash "$SCRIPT_DIR/start-env.sh" \
    >"$start_env_log" 2>&1 || {
        tail -n 200 "$start_env_log" >&2 || true
        fail "start-env.sh failed (log: $start_env_log)"
    }

info "Deploying MessageIdMultisig environment..."
bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism messageIdMultisig \
    --multisig-validators "$ANVIL_DEPLOYER" --multisig-threshold 1 \
    >"$deploy_log" 2>&1 || {
        tail -n 200 "$deploy_log" >&2 || true
        fail "deploy.sh failed (log: $deploy_log)"
    }

cfg_json="$(bash "$SCRIPT_DIR/gen-agent-configs.sh" --ism messageIdMultisig --run-id "$run_id")"
relayer_cfg="$(echo "$cfg_json" | jq -r '.relayer')"
validator_cfg="$(echo "$cfg_json" | jq -r '.validator')"
checkpoint_dir="$(jq -r '.checkpointSyncer.path' "$validator_cfg")"
checkpoint_file="${checkpoint_dir}/0_with_id.json"
checkpoint_backup="${checkpoint_file}.valid"

info "Building agent binaries..."
(cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && cargo build -p relayer -p validator >/dev/null)

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

info "Starting validator to create a valid checkpoint..."
start_validator "$validator_cfg" "$validator_log"

info "Dispatching EVM -> Dusk ($AMOUNT_TO_DUSK wDUSK) before relayer starts..."
cast send "$evm_token" \
    "transferRemote(uint32,bytes32,uint256)" \
    "$dusk_domain" "0x${account_h256}" "$amount_wei" \
    --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null

info "Waiting for validator checkpoint..."
wait_for_checkpoint "$checkpoint_file" "$validator_log"

info "Stopping validator and corrupting checkpoint signature..."
kill_pid "$CURRENT_VALIDATOR_PID"
CURRENT_VALIDATOR_PID=""
cp "$checkpoint_file" "$checkpoint_backup"
corrupt_checkpoint_signature "$checkpoint_file"

info "Starting relayer with corrupt checkpoint metadata..."
start_relayer "$relayer_cfg" "$relayer_log" "$validator_log"

info "Verifying delivery is blocked for ${CORRUPT_METADATA_SECS}s while metadata is corrupt..."
assert_not_delivered_while_corrupt "$dusk_warp" "$expected_dusk_supply" "$relayer_log" "$validator_log"

info "Restoring valid checkpoint metadata..."
cp "$checkpoint_backup" "$checkpoint_file"

info "Waiting for delivery after metadata restore..."
wait_for_dusk_supply "$dusk_warp" "$expected_dusk_supply" "$relayer_log" "$validator_log"

info "Stopping relayer..."
kill_pid "$CURRENT_RELAYER_PID"
CURRENT_RELAYER_PID=""

info "Stopping environment..."
bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true

info "Corrupt metadata flow recovered after restore."
info "Logs:"
info "  start:      $start_env_log"
info "  deploy:     $deploy_log"
info "  relayer:    $relayer_log"
info "  validator:  $validator_log"
info "  checkpoint: $checkpoint_file"
