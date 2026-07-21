#!/usr/bin/env bash
# =============================================================================
# E2E: Bridge wDUSK Between Dusk <-> EVM Using Hyperlane Agents
# =============================================================================
#
# Runs a full local end-to-end flow:
# - starts rusk + anvil (optionally without explorers)
# - deploys Hyperlane + warp route contracts
# - starts relayer (and validator for MessageIdMultisig)
# - proves token bridging EVM->Dusk and Dusk->EVM without manual processing
#
# By default it runs both cases:
#   1) Dusk Mailbox default ISM = testMock
#   2) Dusk Mailbox default ISM = messageIdMultisig
#
# Usage:
#   bash demo/e2e-agents.sh
#   bash demo/e2e-agents.sh --only testMock
#   bash demo/e2e-agents.sh --only messageIdMultisig
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

ONLY=""
AMOUNT_TO_DUSK="${AMOUNT_TO_DUSK:-3}" # whole tokens
AMOUNT_TO_EVM="${AMOUNT_TO_EVM:-1}"   # whole tokens
TIMEOUT_SECS="${TIMEOUT_SECS:-240}"

# PIDs of the currently running agents (used by the EXIT trap).
CURRENT_RELAYER_PID=""
CURRENT_VALIDATOR_PID=""
GENERATED_DUSK_SIGNER_KEY_FILES=()
GENERATED_AGENT_CONFIG_FILES=()
GENERATED_AGENT_RUN_DIRS=()

while [ "$#" -gt 0 ]; do
    case "$1" in
        --only)
            ONLY="${2:-}"; shift 2 ;;
        --amount-to-dusk)
            AMOUNT_TO_DUSK="${2:-}"; shift 2 ;;
        --amount-to-evm)
            AMOUNT_TO_EVM="${2:-}"; shift 2 ;;
        --timeout)
            TIMEOUT_SECS="${2:-}"; shift 2 ;;
        *)
            fail "Unknown argument: $1" ;;
    esac
done

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

require_tools() {
    command -v jq >/dev/null 2>&1 || fail "jq not found"
    command -v cast >/dev/null 2>&1 || fail "cast not found (foundry)"
    command -v forge >/dev/null 2>&1 || fail "forge not found (foundry)"
    command -v python3 >/dev/null 2>&1 || fail "python3 not found"
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
    # Best-effort cleanup on failures/timeouts.
    kill_pid "$CURRENT_RELAYER_PID"
    kill_pid "$CURRENT_VALIDATOR_PID"
    if [ "${#GENERATED_DUSK_SIGNER_KEY_FILES[@]}" -gt 0 ]; then
        rm -f "${GENERATED_DUSK_SIGNER_KEY_FILES[@]}" 2>/dev/null || true
    fi
    if [ "${#GENERATED_AGENT_CONFIG_FILES[@]}" -gt 0 ]; then
        rm -f "${GENERATED_AGENT_CONFIG_FILES[@]}" 2>/dev/null || true
    fi
    if [ "${#GENERATED_AGENT_RUN_DIRS[@]}" -gt 0 ]; then
        rm -rf -- "${GENERATED_AGENT_RUN_DIRS[@]}" 2>/dev/null || true
    fi
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true
}

trap cleanup EXIT

tail_logs_on_fail() {
    local relayer_log="$1"
    local validator_log="${2:-}"
    echo "" >&2
    echo "=== relayer log (tail) ===" >&2
    tail -n 120 "$relayer_log" 2>/dev/null || true
    if [ -n "$validator_log" ]; then
        echo "" >&2
        echo "=== validator log (tail) ===" >&2
        tail -n 120 "$validator_log" 2>/dev/null || true
    fi
}

assert_agents_alive() {
    local relayer_pid="$1"
    local validator_pid="$2"
    local relayer_log="$3"
    local validator_log="$4"

    if ! kill -0 "$relayer_pid" 2>/dev/null; then
        tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
        fail "relayer exited unexpectedly"
    fi
    if [ -n "$validator_pid" ] && ! kill -0 "$validator_pid" 2>/dev/null; then
        tail_logs_on_fail "$relayer_log" "$validator_log"
        fail "validator exited unexpectedly"
    fi
}

run_case() {
    local ism="$1"

    if [ "$ism" != "testMock" ] && [ "$ism" != "messageIdMultisig" ]; then
        fail "Invalid case '$ism' (expected: testMock or messageIdMultisig)"
    fi

    local run_id
    run_id="$(date +%s)"

    info "=== CASE: dusk default ISM = $ism ==="

    # Hard reset environment (fresh rusk state is required; Dusk deployments are deterministic).
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true

    # Start only what we need for headless E2E.
    local start_env_log="/tmp/hyperlane-start-env-${ism}-${run_id}.log"
    SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true bash "$SCRIPT_DIR/start-env.sh" >"$start_env_log" 2>&1 || {
        tail -n 200 "$start_env_log" >&2 || true
        fail "start-env.sh failed (log: $start_env_log)"
    }

    # Deploy contracts.
    local deploy_log="/tmp/hyperlane-deploy-${ism}-${run_id}.log"
    if [ "$ism" = "messageIdMultisig" ]; then
        bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism messageIdMultisig \
          --multisig-validators "$ANVIL_DEPLOYER" --multisig-threshold 1 >"$deploy_log" 2>&1 || {
            tail -n 200 "$deploy_log" >&2 || true
            fail "deploy.sh failed (log: $deploy_log)"
          }
    else
        bash "$SCRIPT_DIR/deploy.sh" --reset --dusk-ism testMock >"$deploy_log" 2>&1 || {
            tail -n 200 "$deploy_log" >&2 || true
            fail "deploy.sh failed (log: $deploy_log)"
        }
    fi

    # Exercise the complete saved-topology validation against the live fresh
    # deployment before any agent config is generated from that state.
    local warm_validate_log="/tmp/hyperlane-warm-validate-${ism}-${run_id}.log"
    bash "$SCRIPT_DIR/deploy.sh" --skip-deploy >"$warm_validate_log" 2>&1 || {
        tail -n 200 "$warm_validate_log" >&2 || true
        fail "saved deployment validation failed (log: $warm_validate_log)"
    }

    # Register the generator's deterministic paths before invoking it. If the
    # generator succeeds but JSON parsing fails, the parent EXIT trap still
    # owns every emitted config and signer file.
    local cfg_json relayer_cfg validator_cfg generated_signer_key_file
    local expected_relayer_cfg expected_validator_cfg expected_run_dir
    expected_run_dir="/tmp/hyperlane-agent-${ism}-${run_id}"
    expected_relayer_cfg="$expected_run_dir/relayer.json"
    expected_validator_cfg="$expected_run_dir/validator-anvil.json"
    dusk_signer_key_file="$expected_run_dir/dusk-signer.key"
    GENERATED_AGENT_RUN_DIRS+=("$expected_run_dir")
    GENERATED_AGENT_CONFIG_FILES+=("$expected_relayer_cfg")
    if [ "$ism" = "messageIdMultisig" ]; then
        GENERATED_AGENT_CONFIG_FILES+=("$expected_validator_cfg")
    fi
    GENERATED_DUSK_SIGNER_KEY_FILES+=("$dusk_signer_key_file")

    # Generate agent configs.
    cfg_json="$(bash "$SCRIPT_DIR/gen-agent-configs.sh" --ism "$ism" --run-id "$run_id")"
    relayer_cfg="$(echo "$cfg_json" | jq -r '.relayer')"
    validator_cfg="$(echo "$cfg_json" | jq -r '.validator // empty')"
    generated_signer_key_file="$(echo "$cfg_json" | jq -r '.duskSignerKeyFile // empty')"
    [ "$relayer_cfg" = "$expected_relayer_cfg" ] || fail "generator returned an unexpected relayer config path"
    [ "$generated_signer_key_file" = "$dusk_signer_key_file" ] || fail "generator returned an unexpected Dusk signer path"
    if [ "$ism" = "messageIdMultisig" ]; then
        [ "$validator_cfg" = "$expected_validator_cfg" ] || fail "generator returned an unexpected validator config path"
    else
        [ -z "$validator_cfg" ] || fail "TestMock generator unexpectedly returned a validator config"
    fi

    # Build agent binaries (incremental).
    (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && cargo build -p relayer -p validator >/dev/null)

    local relayer_log="/tmp/hyperlane-relayer-${ism}-${run_id}.log"
    local validator_log="/tmp/hyperlane-validator-${ism}-${run_id}.log"

    local relayer_pid="" validator_pid=""

    # Start validator first (needed for messageIdMultisig metadata).
    if [ "$ism" = "messageIdMultisig" ]; then
        info "Starting validator..."
        (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
          exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$validator_cfg" ./target/debug/validator \
          >"$validator_log" 2>&1) &
        validator_pid="$!"
        CURRENT_VALIDATOR_PID="$validator_pid"
        sleep 2
        kill -0 "$validator_pid" 2>/dev/null || { tail_logs_on_fail "$relayer_log" "$validator_log"; fail "validator failed to start"; }
    fi

    info "Starting relayer..."
    (cd "$SCRIPT_DIR/../../hyperlane-monorepo/rust/main" && \
      exec env DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$relayer_cfg" ./target/debug/relayer \
      >"$relayer_log" 2>&1) &
    relayer_pid="$!"
    CURRENT_RELAYER_PID="$relayer_pid"
    sleep 2
    kill -0 "$relayer_pid" 2>/dev/null || { tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"; fail "relayer failed to start"; }

    # Load state.
    local state="$BRIDGE_STATE_FILE"
    local evm_token evm_native_token evm_collateral_token
    local dusk_warp dusk_warp_native dusk_warp_collateral dusk_protocol_fee
    local account_h256 evm_domain dusk_domain
    evm_token="$(jq -r '.evm.token' "$state")"
    evm_native_token="$(jq -r '.evm.native_token' "$state")"
    evm_collateral_token="$(jq -r '.evm.collateral_token' "$state")"
    dusk_warp="$(jq -r '.dusk.warp_drc20' "$state")"
    dusk_warp_native="$(jq -r '.dusk.warp_native' "$state")"
    dusk_warp_collateral="$(jq -r '.dusk.warp_drc20_collateral' "$state")"
    dusk_protocol_fee="$(jq -r '.dusk.protocol_fee' "$state")"
    account_h256="$(jq -r '.account_h256' "$state")"
    evm_domain="$(jq -r '.evm_domain' "$state")"
    dusk_domain="$(jq -r '.dusk_domain' "$state")"

    # ----------------------------
    # EVM -> Dusk (via relayer)
    # ----------------------------
    local amount_to_dusk_wei
    amount_to_dusk_wei="$(to_wei "$AMOUNT_TO_DUSK")"

    info "Dispatching EVM -> Dusk ($AMOUNT_TO_DUSK wDUSK)..."
    local evm_balance_before dusk_supply_before_json dusk_supply_before
    evm_balance_before="$(cast call "$evm_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    dusk_supply_before_json="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_warp" --method total_supply --return-type u64 2>/dev/null)"
    dusk_supply_before="$(echo "$dusk_supply_before_json" | jq -r '.value // 0')"

    cast send "$evm_token" \
      "transferRemote(uint32,bytes32,uint256)" \
      "$dusk_domain" "0x${account_h256}" "$amount_to_dusk_wei" \
      --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null

    local expected_dusk_supply
    expected_dusk_supply="$(
python3 - <<PY
print(int(${dusk_supply_before}) + int(${amount_to_dusk_wei}))
PY
)"

    info "Waiting for relayer to mint on Dusk (target supply=${expected_dusk_supply})..."
    local start_ts now supply
    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
            fail "timeout waiting for EVM->Dusk delivery"
        fi
        assert_agents_alive "$relayer_pid" "$validator_pid" "$relayer_log" "$validator_log"
        supply="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_warp" --method total_supply --return-type u64 2>/dev/null | jq -r '.value // 0')"
        if [ "$supply" = "$expected_dusk_supply" ]; then
            break
        fi
        sleep 5
    done
    info "EVM->Dusk delivered."

    # ----------------------------
    # Dusk -> EVM (via relayer)
    # ----------------------------
    local amount_to_evm_wei
    amount_to_evm_wei="$(to_wei "$AMOUNT_TO_EVM")"
    local evm_balance_mid
    evm_balance_mid="$(cast call "$evm_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    local protocol_collected_before protocol_fee_per_dispatch
    protocol_collected_before="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_protocol_fee" --method collected_fees --return-type u64 2>/dev/null | jq -r '.value')"
    protocol_fee_per_dispatch="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_protocol_fee" --method protocol_fee --return-type u64 2>/dev/null | jq -r '.value')"

    info "Dispatching Dusk -> EVM ($AMOUNT_TO_EVM wDUSK)..."
    local evm_recipient_pad32
    evm_recipient_pad32="$(pad_evm_address "$ANVIL_DEPLOYER")"

    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote \
      --rues-url "$DUSK_RUES_URL" \
      --keys "$CONSENSUS_KEYS" \
      --warp-contract "$dusk_warp" \
      --destination "$evm_domain" \
      --recipient "$evm_recipient_pad32" \
      --amount "$amount_to_evm_wei" >/dev/null

    local expected_evm_balance
    expected_evm_balance="$(
python3 - <<PY
print(int(${evm_balance_mid}) + int(${amount_to_evm_wei}))
PY
)"

    info "Waiting for relayer to mint on EVM (target balance=${expected_evm_balance})..."
    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
            fail "timeout waiting for Dusk->EVM delivery"
        fi
        assert_agents_alive "$relayer_pid" "$validator_pid" "$relayer_log" "$validator_log"
        local bal
        bal="$(cast call "$evm_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
        if [ "$bal" = "$expected_evm_balance" ]; then
            break
        fi
        sleep 2
    done
    info "Dusk->EVM delivered."

    local protocol_collected_after expected_protocol_collected
    protocol_collected_after="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_protocol_fee" --method collected_fees --return-type u64 2>/dev/null | jq -r '.value')"
    expected_protocol_collected="$((protocol_collected_before + protocol_fee_per_dispatch))"
    if [ "$protocol_collected_after" != "$expected_protocol_collected" ]; then
        fail "protocol fee custody mismatch: expected $expected_protocol_collected, got $protocol_collected_after"
    fi
    info "ProtocolFee collected the live Dusk dispatch fee ($protocol_fee_per_dispatch LUX)."

    # ----------------------------
    # Native DUSK route round trip
    # ----------------------------
    local native_send=100000000 native_return=40000000
    local evm_native_before evm_native_target
    evm_native_before="$(cast call "$evm_native_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    evm_native_target="$((evm_native_before + native_send))"
    info "Dispatching native DUSK -> EVM ($native_send LUX)..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote \
      --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" \
      --warp-contract "$dusk_warp_native" --destination "$evm_domain" \
      --recipient "$evm_recipient_pad32" --amount "$native_send" --native >/dev/null

    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
            fail "timeout waiting for native Dusk->EVM delivery"
        fi
        assert_agents_alive "$relayer_pid" "$validator_pid" "$relayer_log" "$validator_log"
        local native_evm_balance
        native_evm_balance="$(cast call "$evm_native_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
        [ "$native_evm_balance" = "$evm_native_target" ] && break
        sleep 2
    done
    local native_locked
    native_locked="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
      --contract 0100000000000000000000000000000000000000000000000000000000000000 \
      --method contract_balance --return-type u64 --arg-bytes32 "$dusk_warp_native" 2>/dev/null | jq -r '.value')"
    [ "$native_locked" = "$native_send" ] || fail "native custody mismatch after lock: $native_locked"

    info "Returning native DUSK EVM -> Dusk ($native_return LUX)..."
    cast send "$evm_native_token" "transferRemote(uint32,bytes32,uint256)" \
      "$dusk_domain" "0x${account_h256}" "$native_return" \
      --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null
    local native_locked_target="$((native_send - native_return))"
    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
            fail "timeout waiting for native EVM->Dusk delivery"
        fi
        assert_agents_alive "$relayer_pid" "$validator_pid" "$relayer_log" "$validator_log"
        native_locked="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
          --contract 0100000000000000000000000000000000000000000000000000000000000000 \
          --method contract_balance --return-type u64 --arg-bytes32 "$dusk_warp_native" 2>/dev/null | jq -r '.value')"
        [ "$native_locked" = "$native_locked_target" ] && break
        sleep 2
    done
    info "Native route round trip delivered with exact DUSK custody."

    # ----------------------------
    # DRC20 collateral round trip
    # ----------------------------
    local collateral_send=500000000000000000 collateral_return=200000000000000000
    local owner_token_before collateral_locked_before
    owner_token_before="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" drc20-balance \
      --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" --token "$dusk_warp" | jq -r '.balance')"
    collateral_locked_before="$("$DUSK_TX" drc20-balance --rues-url "$DUSK_RUES_URL" \
      --token "$dusk_warp" --account-contract "$dusk_warp_collateral" | jq -r '.balance')"
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" drc20-approve \
      --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" --token "$dusk_warp" \
      --spender "$dusk_warp_collateral" --amount "$collateral_send" >/dev/null

    local evm_collateral_before evm_collateral_target
    evm_collateral_before="$(cast call "$evm_collateral_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    evm_collateral_target="$((evm_collateral_before + collateral_send))"
    info "Dispatching DRC20 collateral Dusk -> EVM..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote \
      --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" \
      --warp-contract "$dusk_warp_collateral" --destination "$evm_domain" \
      --recipient "$evm_recipient_pad32" --amount "$collateral_send" >/dev/null
    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
            fail "timeout waiting for collateral Dusk->EVM delivery"
        fi
        assert_agents_alive "$relayer_pid" "$validator_pid" "$relayer_log" "$validator_log"
        local collateral_evm_balance
        collateral_evm_balance="$(cast call "$evm_collateral_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
        [ "$collateral_evm_balance" = "$evm_collateral_target" ] && break
        sleep 2
    done
    local owner_token_after collateral_locked_after
    owner_token_after="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" drc20-balance \
      --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" --token "$dusk_warp" | jq -r '.balance')"
    collateral_locked_after="$("$DUSK_TX" drc20-balance --rues-url "$DUSK_RUES_URL" \
      --token "$dusk_warp" --account-contract "$dusk_warp_collateral" | jq -r '.balance')"
    [ "$owner_token_after" = "$((owner_token_before - collateral_send))" ] || fail "owner collateral debit mismatch"
    [ "$collateral_locked_after" = "$((collateral_locked_before + collateral_send))" ] || fail "DRC20 custody mismatch after lock"

    info "Returning DRC20 collateral EVM -> Dusk..."
    cast send "$evm_collateral_token" "transferRemote(uint32,bytes32,uint256)" \
      "$dusk_domain" "0x${account_h256}" "$collateral_return" \
      --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null
    local owner_return_target="$((owner_token_after + collateral_return))"
    start_ts="$(date +%s)"
    while true; do
        now="$(date +%s)"
        if [ $((now - start_ts)) -gt "$TIMEOUT_SECS" ]; then
            tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"
            fail "timeout waiting for collateral EVM->Dusk delivery"
        fi
        assert_agents_alive "$relayer_pid" "$validator_pid" "$relayer_log" "$validator_log"
        owner_token_after="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" drc20-balance \
          --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" --token "$dusk_warp" | jq -r '.balance')"
        [ "$owner_token_after" = "$owner_return_target" ] && break
        sleep 2
    done
    collateral_locked_after="$("$DUSK_TX" drc20-balance --rues-url "$DUSK_RUES_URL" \
      --token "$dusk_warp" --account-contract "$dusk_warp_collateral" | jq -r '.balance')"
    [ "$collateral_locked_after" = "$((collateral_locked_before + collateral_send - collateral_return))" ] || fail "DRC20 custody mismatch after unlock"
    info "Collateral route round trip delivered with exact allowance and custody changes."

    protocol_collected_after="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_protocol_fee" --method collected_fees --return-type u64 2>/dev/null | jq -r '.value')"
    expected_protocol_collected="$((protocol_collected_before + 3 * protocol_fee_per_dispatch))"
    [ "$protocol_collected_after" = "$expected_protocol_collected" ] || fail "three-route protocol fee mismatch"

    # Cleanup agents + env.
    info "Stopping agents..."
    kill_pid "$relayer_pid"
    kill_pid "$validator_pid"
    CURRENT_RELAYER_PID=""
    CURRENT_VALIDATOR_PID=""
    if [ -n "$dusk_signer_key_file" ]; then
        rm -f "$dusk_signer_key_file" 2>/dev/null || true
    fi
    rm -f "$relayer_cfg" 2>/dev/null || true
    if [ -n "$validator_cfg" ]; then
        rm -f "$validator_cfg" 2>/dev/null || true
    fi

    info "Stopping environment..."
    bash "$SCRIPT_DIR/stop-env.sh" --force >/dev/null 2>&1 || true

    info "CASE OK: $ism"
}

main() {
    require_tools

    if [ -n "$ONLY" ]; then
        run_case "$ONLY"
        return 0
    fi

    run_case "testMock"
    run_case "messageIdMultisig"
}

main
