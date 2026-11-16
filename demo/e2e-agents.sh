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
#   bash demo/e2e-agents.sh --only messageIdMultisig --validators 3 --threshold 2
#
# In the messageIdMultisig case the script runs one validator per key and
# origin chain (--validators, default 1, at most 5), with the given multisig
# threshold on both chains. After the route checks it runs a quorum check:
# with exactly `threshold` validators per origin messages still deliver, with
# one fewer they stall, and they deliver once a validator is restarted.
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

# The agents run in AGENT_RUN_DIR, which holds Hyperlane's config/ directory,
# from the binaries in AGENT_BIN_DIR: by default hyperlane-monorepo/rust/main
# and its debug build, which this script builds first; with AGENT_BIN_DIR
# set, the binaries there as they are. Each agent starts with only PATH,
# HOME, DUSK_TX_BIN and its config, as deploy/local-agents.sh starts them:
# Hyperlane applies every HYP_* variable over the config files, so one left
# in the shell, such as HYP_CHAINS_DUSK_CUSTOMRPCURLS, would point the agent
# at another node, and others (CONFIG_FILES, RUST_LOG, AWS_*) change what it
# does.
AGENT_RUN_DIR="${AGENT_RUN_DIR:-$SCRIPT_DIR/../../hyperlane-monorepo/rust/main}"
BUILD_AGENTS=true
[ -z "${AGENT_BIN_DIR:-}" ] || BUILD_AGENTS=false
AGENT_BIN_DIR="${AGENT_BIN_DIR:-$AGENT_RUN_DIR/target/debug}"

# Anvil's deterministic, pre-funded development accounts. Keep the operator
# on account #0, the relayer on #1, and the validator on #2 so independent
# processes never compete for one EVM nonce stream during the live test.
E2E_ANVIL_RELAYER_PRIVATE_KEY="${E2E_ANVIL_RELAYER_PRIVATE_KEY:-0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d}"
E2E_ANVIL_VALIDATOR_PRIVATE_KEY="${E2E_ANVIL_VALIDATOR_PRIVATE_KEY:-0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a}"
E2E_ANVIL_VALIDATOR="${E2E_ANVIL_VALIDATOR:-0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC}"
# Validator i uses Anvil account #(2 + i). Account #2 is the historical single
# validator above.
E2E_VALIDATOR_KEYS_ALL=(
    "$E2E_ANVIL_VALIDATOR_PRIVATE_KEY"
    0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
    0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a
    0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba
    0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e
)
VALIDATOR_COUNT="${E2E_VALIDATOR_COUNT:-1}"
VALIDATOR_THRESHOLD="${E2E_VALIDATOR_THRESHOLD:-1}"
QUORUM_STALL_SECS="${QUORUM_STALL_SECS:-60}"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

ONLY=""
AMOUNT_TO_DUSK="${AMOUNT_TO_DUSK:-3}" # whole tokens
AMOUNT_TO_EVM="${AMOUNT_TO_EVM:-1}"   # whole tokens
TIMEOUT_SECS="${TIMEOUT_SECS:-240}"

# PIDs of the currently running agents (used by the EXIT trap). Validator i
# of each origin sits at index i; a deliberately stopped validator has "".
CURRENT_RELAYER_PID=""
VALIDATOR_PIDS=()
VALIDATOR_LOGS=()
VALIDATOR_CFGS=()
DUSK_VALIDATOR_PIDS=()
DUSK_VALIDATOR_LOGS=()
DUSK_VALIDATOR_CFGS=()
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
        --validators)
            VALIDATOR_COUNT="${2:-}"; shift 2 ;;
        --threshold)
            VALIDATOR_THRESHOLD="${2:-}"; shift 2 ;;
        *)
            fail "Unknown argument: $1" ;;
    esac
done

[[ "$VALIDATOR_COUNT" =~ ^[1-5]$ ]] || fail "--validators must be 1 to 5"
[[ "$VALIDATOR_THRESHOLD" =~ ^[1-5]$ ]] && [ "$VALIDATOR_THRESHOLD" -le "$VALIDATOR_COUNT" ] \
    || fail "--threshold must be between 1 and --validators"
E2E_VALIDATOR_KEYS=("${E2E_VALIDATOR_KEYS_ALL[@]:0:$VALIDATOR_COUNT}")
E2E_VALIDATOR_ADDRESSES=()
for key in "${E2E_VALIDATOR_KEYS[@]}"; do
    E2E_VALIDATOR_ADDRESSES+=("$(cast wallet address --private-key "$key")") \
        || fail "cannot derive validator address"
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
    if [ "${E2E_SDK_CHECK:-false}" = "true" ]; then
        command -v node >/dev/null 2>&1 || fail "node not found"
        [ -f "$SCRIPT_DIR/../ts-sdk/dist/index.js" ] && \
          [ -f "$SCRIPT_DIR/../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js" ] \
          || fail "Run make test-sdk before enabling E2E_SDK_CHECK"
    fi
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

kill_all_validators() {
    local pid
    for pid in "${VALIDATOR_PIDS[@]}" "${DUSK_VALIDATOR_PIDS[@]}"; do
        kill_pid "$pid"
    done
    VALIDATOR_PIDS=()
    DUSK_VALIDATOR_PIDS=()
}

cleanup() {
    # Best-effort cleanup on failures/timeouts.
    kill_pid "$CURRENT_RELAYER_PID"
    kill_all_validators
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

# Usage: tail_logs_on_fail <relayer_log> [ignored...]. Tails the relayer and
# every validator log of the current case.
tail_logs_on_fail() {
    local relayer_log="$1" log lines=120
    [ "${#VALIDATOR_LOGS[@]}" -le 1 ] || lines=60
    echo "" >&2
    echo "=== relayer log (tail) ===" >&2
    tail -n 120 "$relayer_log" 2>/dev/null || true
    for log in "${VALIDATOR_LOGS[@]}" "${DUSK_VALIDATOR_LOGS[@]}"; do
        echo "" >&2
        echo "=== $(basename "$log") (tail) ===" >&2
        tail -n "$lines" "$log" 2>/dev/null || true
    done
}

# Usage: assert_agents_alive <relayer_pid> <ignored> <relayer_log> [ignored].
# Checks the relayer and every validator that has not been deliberately
# stopped.
assert_agents_alive() {
    local relayer_pid="$1" relayer_log="$3" i

    if ! kill -0 "$relayer_pid" 2>/dev/null; then
        tail_logs_on_fail "$relayer_log"
        fail "relayer exited unexpectedly"
    fi
    for i in "${!VALIDATOR_PIDS[@]}"; do
        if [ -n "${VALIDATOR_PIDS[$i]}" ] && ! kill -0 "${VALIDATOR_PIDS[$i]}" 2>/dev/null; then
            tail_logs_on_fail "$relayer_log"
            fail "EVM-origin validator $i exited unexpectedly"
        fi
    done
    for i in "${!DUSK_VALIDATOR_PIDS[@]}"; do
        if [ -n "${DUSK_VALIDATOR_PIDS[$i]}" ] && ! kill -0 "${DUSK_VALIDATOR_PIDS[$i]}" 2>/dev/null; then
            tail_logs_on_fail "$relayer_log"
            fail "Dusk-origin validator $i exited unexpectedly"
        fi
    done
}

# start_validator <anvil|dusk> <index>: start (or restart) one validator.
# Dusk-origin validators announce with the shared demo Moonlight signer, so
# wait for each announcement before starting the next to avoid nonce races.
start_validator() {
    local kind="$1" i="$2" cfg log pid
    if [ "$kind" = "anvil" ]; then
        cfg="${VALIDATOR_CFGS[$i]}"; log="${VALIDATOR_LOGS[$i]}"
    else
        cfg="${DUSK_VALIDATOR_CFGS[$i]}"; log="${DUSK_VALIDATOR_LOGS[$i]}"
    fi
    (cd "$AGENT_RUN_DIR" && \
      exec env -i PATH="$PATH" HOME="$HOME" DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$cfg" "$AGENT_BIN_DIR/validator" \
      >>"$log" 2>&1) &
    pid="$!"
    if [ "$kind" = "anvil" ]; then VALIDATOR_PIDS[i]="$pid"; else DUSK_VALIDATOR_PIDS[i]="$pid"; fi
    sleep 2
    kill -0 "$pid" 2>/dev/null || { tail_logs_on_fail /dev/null; fail "$kind-origin validator $i failed to start"; }
}

stop_validator() {
    local kind="$1" i="$2"
    if [ "$kind" = "anvil" ]; then
        kill_pid "${VALIDATOR_PIDS[$i]}"; VALIDATOR_PIDS[i]=""
    else
        kill_pid "${DUSK_VALIDATOR_PIDS[$i]}"; DUSK_VALIDATOR_PIDS[i]=""
    fi
}

wait_for_dusk_announcements() {
    local va="$1" want="$2" start count
    start="$(date +%s)"
    while true; do
        count="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$va" \
          --method announced_validator_count --return-type u32 2>/dev/null | jq -r '.value // 0')"
        [ "${count:-0}" -ge "$want" ] && return 0
        [ $(( $(date +%s) - start )) -le 120 ] \
          || { tail_logs_on_fail /dev/null; fail "timeout waiting for $want Dusk validator announcement(s) (have ${count:-0})"; }
        sleep 3
    done
}

# Prove the live multisig threshold on both chains: deliveries succeed with
# exactly `threshold` validators per origin, stall with one fewer, and resume
# when a validator comes back.
run_quorum_check() {
    local relayer_pid="$1" relayer_log="$2" evm_token="$3" dusk_warp="$4"
    local dusk_domain="$5" evm_domain="$6" account_h256="$7" evm_recipient="$8"
    local n="$VALIDATOR_COUNT" m="$VALIDATOR_THRESHOLD" i one
    one="$(to_wei 1)"

    dusk_supply() {
        "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_warp" \
          --method total_supply --return-type u64 2>/dev/null | jq -r '.value // 0'
    }
    evm_balance() {
        cast call "$evm_token" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" \
          --rpc-url "$ANVIL_RPC" | awk '{print $1}'
    }
    send_to_dusk() {
        cast send "$evm_token" "transferRemote(uint32,bytes32,uint256)" \
          "$dusk_domain" "0x${account_h256}" "$one" \
          --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null
    }
    send_to_evm() {
        DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote \
          --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" \
          --warp-contract "$dusk_warp" --destination "$evm_domain" \
          --recipient "$evm_recipient" --amount "$one" >/dev/null
    }
    # wait_value <label> <getter> <target>
    wait_value() {
        local label="$1" getter="$2" target="$3" start
        start="$(date +%s)"
        until [ "$($getter)" = "$target" ]; do
            [ $(( $(date +%s) - start )) -le "$TIMEOUT_SECS" ] \
              || { tail_logs_on_fail "$relayer_log"; fail "quorum check: timeout waiting for $label"; }
            assert_agents_alive "$relayer_pid" "" "$relayer_log"
            sleep 3
        done
    }
    add() { python3 -c 'import sys; print(int(sys.argv[1]) + int(sys.argv[2]))' "$1" "$2"; }

    local supply_target balance_target
    if [ "$n" -gt "$m" ]; then
        for ((i = m; i < n; i++)); do stop_validator anvil "$i"; stop_validator dusk "$i"; done
        info "Quorum check: $m of $n validators per origin running, expecting delivery..."
        supply_target="$(add "$(dusk_supply)" "$one")"
        send_to_dusk
        wait_value "EVM->Dusk delivery at threshold" dusk_supply "$supply_target"
        balance_target="$(add "$(evm_balance)" "$one")"
        send_to_evm
        wait_value "Dusk->EVM delivery at threshold" evm_balance "$balance_target"
        info "Delivered in both directions with exactly $m signatures available."
    fi

    stop_validator anvil "$((m - 1))"
    stop_validator dusk "$((m - 1))"
    info "Quorum check: $((m - 1)) of $n validators per origin running, expecting no delivery for ${QUORUM_STALL_SECS}s..."
    # Each send burns on its source chain, so take the snapshot afterwards.
    local supply_before balance_before
    send_to_dusk
    send_to_evm
    supply_before="$(dusk_supply)"
    balance_before="$(evm_balance)"
    local start
    start="$(date +%s)"
    while [ $(( $(date +%s) - start )) -lt "$QUORUM_STALL_SECS" ]; do
        assert_agents_alive "$relayer_pid" "" "$relayer_log"
        [ "$(dusk_supply)" = "$supply_before" ] || fail "quorum check: EVM->Dusk delivered below threshold"
        [ "$(evm_balance)" = "$balance_before" ] || fail "quorum check: Dusk->EVM delivered below threshold"
        sleep 5
    done
    info "Nothing delivered below threshold."

    info "Quorum check: restarting validator $((m - 1)) on both origins..."
    start_validator anvil "$((m - 1))"
    start_validator dusk "$((m - 1))"
    wait_value "stalled EVM->Dusk delivery after restart" dusk_supply "$(add "$supply_before" "$one")"
    wait_value "stalled Dusk->EVM delivery after restart" evm_balance "$(add "$balance_before" "$one")"
    info "Quorum check OK: stalled messages delivered once the threshold was met again."
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
          --multisig-validators "$(IFS=,; echo "${E2E_VALIDATOR_ADDRESSES[*]}")" \
          --multisig-threshold "$VALIDATOR_THRESHOLD" >"$deploy_log" 2>&1 || {
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

    # Compute deterministic paths before invoking the generator, but do not
    # claim ownership yet. If another invocation already owns the run ID, the
    # generator must fail without letting this process delete that directory.
    local cfg_json relayer_cfg validator_cfg dusk_validator_cfg generated_signer_key_file
    local expected_relayer_cfg expected_validator_cfg expected_dusk_validator_cfg expected_run_dir
    expected_run_dir="/tmp/hyperlane-agent-${ism}-${run_id}"
    expected_relayer_cfg="$expected_run_dir/relayer.json"
    expected_validator_cfg="$expected_run_dir/validator-anvil.json"
    expected_dusk_validator_cfg="$expected_run_dir/validator-dusk.json"
    dusk_signer_key_file="$expected_run_dir/dusk-signer.key"
    # Generate agent configs.
    cfg_json="$(
        ANVIL_RELAYER_PRIVATE_KEY="$E2E_ANVIL_RELAYER_PRIVATE_KEY" \
        ANVIL_VALIDATOR_PRIVATE_KEY="$E2E_ANVIL_VALIDATOR_PRIVATE_KEY" \
        ANVIL_VALIDATOR_PRIVATE_KEYS="$(IFS=,; echo "${E2E_VALIDATOR_KEYS[*]}")" \
        bash "$SCRIPT_DIR/gen-agent-configs.sh" --ism "$ism" --run-id "$run_id"
    )"
    GENERATED_AGENT_RUN_DIRS+=("$expected_run_dir")
    GENERATED_AGENT_CONFIG_FILES+=("$expected_relayer_cfg")
    if [ "$ism" = "messageIdMultisig" ]; then
        mapfile -t VALIDATOR_CFGS < <(echo "$cfg_json" | jq -r '.validators[]')
        mapfile -t DUSK_VALIDATOR_CFGS < <(echo "$cfg_json" | jq -r '.duskValidators[]')
        [ "${#VALIDATOR_CFGS[@]}" -eq "$VALIDATOR_COUNT" ] && [ "${#DUSK_VALIDATOR_CFGS[@]}" -eq "$VALIDATOR_COUNT" ] \
          || fail "generator returned the wrong number of validator configs"
        GENERATED_AGENT_CONFIG_FILES+=("${VALIDATOR_CFGS[@]}" "${DUSK_VALIDATOR_CFGS[@]}")
    else
        VALIDATOR_CFGS=()
        DUSK_VALIDATOR_CFGS=()
    fi
    GENERATED_DUSK_SIGNER_KEY_FILES+=("$dusk_signer_key_file")
    relayer_cfg="$(echo "$cfg_json" | jq -r '.relayer')"
    validator_cfg="$(echo "$cfg_json" | jq -r '.validator // empty')"
    dusk_validator_cfg="$(echo "$cfg_json" | jq -r '.duskValidator // empty')"
    generated_signer_key_file="$(echo "$cfg_json" | jq -r '.duskSignerKeyFile // empty')"
    [ "$relayer_cfg" = "$expected_relayer_cfg" ] || fail "generator returned an unexpected relayer config path"
    [ "$generated_signer_key_file" = "$dusk_signer_key_file" ] || fail "generator returned an unexpected Dusk signer path"
    if [ "$ism" = "messageIdMultisig" ]; then
        [ "$validator_cfg" = "$expected_validator_cfg" ] || fail "generator returned an unexpected validator config path"
        [ "$dusk_validator_cfg" = "$expected_dusk_validator_cfg" ] || fail "generator returned an unexpected Dusk validator config path"
    else
        [ -z "$validator_cfg" ] || fail "TestMock generator unexpectedly returned a validator config"
        [ -z "$dusk_validator_cfg" ] || fail "TestMock generator unexpectedly returned a Dusk validator config"
    fi

    # Load state and complete the route-owner setup mutation before starting
    # agents. In the multisig case the Dusk-origin validator immediately
    # self-announces with the same demo signer, so submitting this withdrawal
    # after agent startup would create an artificial Moonlight nonce race.
    local state="$BRIDGE_STATE_FILE"
    local evm_token evm_native_token evm_collateral_token
    local dusk_mailbox dusk_warp dusk_warp_native dusk_warp_collateral dusk_protocol_fee
    local account_h256 evm_domain dusk_domain dusk_chain_id_hex dusk_chain_id
    evm_token="$(jq -r '.evm.token' "$state")"
    evm_native_token="$(jq -r '.evm.native_token' "$state")"
    evm_collateral_token="$(jq -r '.evm.collateral_token' "$state")"
    dusk_mailbox="$(jq -r '.dusk.mailbox' "$state")"
    dusk_warp="$(jq -r '.dusk.warp_drc20' "$state")"
    dusk_warp_native="$(jq -r '.dusk.warp_native' "$state")"
    dusk_warp_collateral="$(jq -r '.dusk.warp_drc20_collateral' "$state")"
    dusk_protocol_fee="$(jq -r '.dusk.protocol_fee' "$state")"
    account_h256="$(jq -r '.account_h256' "$state")"
    evm_domain="$(jq -r '.evm_domain' "$state")"
    dusk_domain="$(jq -r '.dusk_domain' "$state")"
    dusk_chain_id_hex="$(jq -er '.dusk_chain_id | strings | select(test("^[0-9A-Fa-f]{2}$"))' "$state")" \
      || fail "deployment state has an invalid Dusk chain ID"
    dusk_chain_id=$((16#$dusk_chain_id_hex))

    # Exercise the stacked withdrawal ABI on live Rusk before using the same
    # route for message delivery. The route owner withdraws one LUX from only
    # that route's Mailbox credit, then the later outbound transfer proves the
    # remaining credit is still usable.
    info "Withdrawing one LUX of WarpDrc20 dispatch credit on live Rusk..."
    local credit_before credit_after
    credit_before="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
      --contract "$dusk_mailbox" --method fee_credit --return-type u64 \
      --arg-bytes32 "$dusk_warp" | jq -r '.value')"
    [[ "$credit_before" =~ ^[1-9][0-9]*$ ]] \
      || fail "invalid pre-withdrawal dispatch credit: $credit_before"
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" withdraw-dispatch \
      --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" \
      --target "$dusk_warp" --amount 1 \
      --expected-chain-id "$dusk_chain_id" >/dev/null
    credit_after="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" \
      --contract "$dusk_mailbox" --method fee_credit --return-type u64 \
      --arg-bytes32 "$dusk_warp" | jq -r '.value')"
    [[ "$credit_after" =~ ^[0-9]+$ ]] \
      || fail "invalid post-withdrawal dispatch credit: $credit_after"
    [ "$credit_after" -eq $((credit_before - 1)) ] \
      || fail "dispatch-credit withdrawal mismatch: before=$credit_before after=$credit_after"
    info "Live dispatch-credit withdrawal confirmed"

    # Build agent binaries (incremental).
    [ "$BUILD_AGENTS" = false ] || (cd "$AGENT_RUN_DIR" && cargo build -p relayer -p validator >/dev/null)

    local relayer_log="/tmp/hyperlane-relayer-${ism}-${run_id}.log"
    local validator_log="/tmp/hyperlane-validator-${ism}-${run_id}.log"
    local dusk_validator_log="/tmp/hyperlane-validator-dusk-${ism}-${run_id}.log"

    local relayer_pid="" validator_pid="" dusk_validator_pid=""

    # Start validators first (needed for messageIdMultisig metadata).
    VALIDATOR_PIDS=(); DUSK_VALIDATOR_PIDS=(); VALIDATOR_LOGS=(); DUSK_VALIDATOR_LOGS=()
    if [ "$ism" = "messageIdMultisig" ]; then
        local i dusk_va
        dusk_va="$(jq -r '.dusk.validator_announce' "$BRIDGE_STATE_FILE")"
        for i in "${!VALIDATOR_CFGS[@]}"; do
            VALIDATOR_LOGS[i]="/tmp/hyperlane-validator-${ism}-${run_id}-$i.log"
            DUSK_VALIDATOR_LOGS[i]="/tmp/hyperlane-validator-dusk-${ism}-${run_id}-$i.log"
        done
        validator_log="${VALIDATOR_LOGS[0]}"
        dusk_validator_log="${DUSK_VALIDATOR_LOGS[0]}"
        for i in "${!VALIDATOR_CFGS[@]}"; do
            info "Starting EVM-origin validator $i (${E2E_VALIDATOR_ADDRESSES[$i]})..."
            start_validator anvil "$i"
        done
        for i in "${!DUSK_VALIDATOR_CFGS[@]}"; do
            info "Starting Dusk-origin validator $i (${E2E_VALIDATOR_ADDRESSES[$i]})..."
            start_validator dusk "$i"
            wait_for_dusk_announcements "$dusk_va" "$((i + 1))"
        done
        validator_pid="${VALIDATOR_PIDS[0]}"
        dusk_validator_pid="${DUSK_VALIDATOR_PIDS[0]}"
    fi

    info "Starting relayer..."
    (cd "$AGENT_RUN_DIR" && \
      exec env -i PATH="$PATH" HOME="$HOME" DUSK_TX_BIN="$DUSK_TX" CONFIG_FILES="$relayer_cfg" "$AGENT_BIN_DIR/relayer" \
      >"$relayer_log" 2>&1) &
    relayer_pid="$!"
    CURRENT_RELAYER_PID="$relayer_pid"
    sleep 2
    kill -0 "$relayer_pid" 2>/dev/null || { tail_logs_on_fail "$relayer_log" "${validator_pid:+$validator_log}"; fail "relayer failed to start"; }

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
    local expected_evm_after_burn
    expected_evm_after_burn="$(python3 -c 'import sys; print(int(sys.argv[1]) - int(sys.argv[2]))' "$evm_balance_before" "$amount_to_dusk_wei")"
    [ "$evm_balance_mid" = "$expected_evm_after_burn" ] || fail "EVM synthetic burn mismatch"
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
    info "Collateral route round trip delivered with exact token custody."

    protocol_collected_after="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_protocol_fee" --method collected_fees --return-type u64 2>/dev/null | jq -r '.value')"
    expected_protocol_collected="$((protocol_collected_before + 3 * protocol_fee_per_dispatch))"
    [ "$protocol_collected_after" = "$expected_protocol_collected" ] || fail "three-route protocol fee mismatch"

    # Cross-chain conservation after every queued route has settled. Python
    # keeps u64/u256 amounts exact above bash's signed 64-bit arithmetic range.
    local final_dusk_supply final_evm_supply native_evm_supply collateral_evm_supply
    final_dusk_supply="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$dusk_warp" --method total_supply --return-type u64 2>/dev/null | jq -r '.value')"
    final_evm_supply="$(cast call "$evm_token" "totalSupply()(uint256)" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    native_evm_supply="$(cast call "$evm_native_token" "totalSupply()(uint256)" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    collateral_evm_supply="$(cast call "$evm_collateral_token" "totalSupply()(uint256)" --rpc-url "$ANVIL_RPC" | awk '{print $1}')"
    python3 - "$final_dusk_supply" "$final_evm_supply" "$dusk_supply_before" "$INITIAL_SUPPLY" <<'PY_CHECK'
import sys
actual_dusk, actual_evm, initial_dusk, initial_evm = map(int, sys.argv[1:])
assert actual_dusk + actual_evm == initial_dusk + initial_evm, "synthetic cross-chain supply changed"
PY_CHECK
    [ "$native_evm_supply" = "$native_locked" ] || fail "native wrapped supply is not exactly backed by DUSK custody"
    [ "$collateral_evm_supply" = "$collateral_locked_after" ] || fail "collateral wrapped supply is not exactly backed by DRC20 custody"
    info "All three routes conserve supply and collateral across chains."

    if [ "${E2E_SDK_CHECK:-false}" = "true" ]; then
        info "Checking the TypeScript SDK against the completed live routes..."
        DUSK_RUES_URL="$DUSK_RUES_URL" BRIDGE_STATE_FILE="$BRIDGE_STATE_FILE" \
          TOKEN_NAME="$TOKEN_NAME" TOKEN_DECIMALS="$TOKEN_DECIMALS" \
          EXPECTED_DUSK_SUPPLY="$(python3 -c 'import sys; print(int(sys.argv[1]) + int(sys.argv[2]) - int(sys.argv[3]))' "$dusk_supply_before" "$amount_to_dusk_wei" "$amount_to_evm_wei")" \
          node "$SCRIPT_DIR/../ts-sdk/tests/live.mjs"
    fi

    if [ "$ism" = "messageIdMultisig" ]; then
        run_quorum_check "$relayer_pid" "$relayer_log" "$evm_token" "$dusk_warp" \
          "$dusk_domain" "$evm_domain" "$account_h256" "$evm_recipient_pad32"
    fi

    # Cleanup agents + env.
    info "Stopping agents..."
    kill_pid "$relayer_pid"
    kill_all_validators
    CURRENT_RELAYER_PID=""
    if [ -n "$dusk_signer_key_file" ]; then
        rm -f "$dusk_signer_key_file" 2>/dev/null || true
    fi
    rm -f "$relayer_cfg" "${VALIDATOR_CFGS[@]}" "${DUSK_VALIDATOR_CFGS[@]}" 2>/dev/null || true

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
