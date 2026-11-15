#!/usr/bin/env bash
# =============================================================================
# Run the Hyperlane agents for a local rehearsal of the testnet deployment
# =============================================================================
#
# After `deploy/testnet.sh deploy` and `agent-configs` with
# TESTNET_ENV=deploy/local.env, derives one config per agent from the
# generated templates and starts:
#   - the relayer (Anvil #1 signs its EVM transactions);
#   - the Dusk-origin validators (Anvil #3, #4, #5), each with its own
#     database, event cursors, checkpoint directory and metrics port;
#   - a stand-in for Hyperlane's Sepolia validator (Anvil #2), which signs
#     checkpoints for EVM-origin messages.
# All agents send their Dusk transactions from the deployer's key, exported
# to DUSK_SIGNER_KEY_FILE. The validators start one at a time so their
# announcements don't race for the account's nonce.
#
#   bash deploy/local-agents.sh start
#   bash deploy/local-agents.sh status
#   bash deploy/local-agents.sh stop
#
# Configs, checkpoints, logs and PID files go to
# deploy/out/<DEPLOY_NAME>/agents/. The agents run from
# hyperlane-monorepo/rust/main, which holds the config/ directory they load.

set -euo pipefail
umask 077

DUSK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${TESTNET_ENV:-$DUSK_DIR/deploy/local.env}"
[ -f "$ENV_FILE" ] || { echo "error: $ENV_FILE not found (run deploy/local-evm-core.sh)" >&2; exit 1; }
# shellcheck source=/dev/null
source "$ENV_FILE"

DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
OUT_DIR="$DUSK_DIR/deploy/out/$DEPLOY_NAME"
AGENTS_DIR="$OUT_DIR/agents"
DUSK_RELAYER_MAX_BATCH_SIZE="${DUSK_RELAYER_MAX_BATCH_SIZE:-32}"
AGENT_RUN_DIR="${AGENT_RUN_DIR:-$DUSK_DIR/../hyperlane-monorepo/rust/main}"
AGENT_BIN_DIR="${AGENT_BIN_DIR:-$AGENT_RUN_DIR/target/debug}"

info() { echo "[INFO] $*" >&2; }
ok()   { echo "[OK]   $*" >&2; }
fail() { echo "[FAIL] $*" >&2; exit 1; }

# Anvil's public development keys #1-#5.
RELAYER_EVM_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
EVM_ORIGIN_VALIDATOR_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
DUSK_ORIGIN_VALIDATOR_KEYS=(
    0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
    0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a
    0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba
)
EVM_SIGNER_ENV="HYP_CHAINS_$(tr '[:lower:]' '[:upper:]' <<<"$EVM_CHAIN_NAME")_SIGNER_KEY"

agent_names() {
    local i
    echo validator-evm
    for i in "${!DUSK_ORIGIN_VALIDATOR_KEYS[@]}"; do echo "validator-dusk-$((i + 1))"; done
    echo relayer
}

pid_of() { cat "$AGENTS_DIR/$1.pid" 2>/dev/null || true; }
is_running() { local pid; pid="$(pid_of "$1")"; [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; }

start_agent() {
    local name="$1" bin="$2"; shift 2
    if is_running "$name"; then
        ok "$name is already running (pid $(pid_of "$name"))"
        return
    fi
    # The agent gets only these variables, and the keys in "$@". Hyperlane
    # applies every HYP_* variable over its config files, so one left in the
    # operator's shell, such as HYP_CHAINS_DUSK_CUSTOMRPCURLS, would point the
    # rehearsal's agents at another node; others change what the agent or its
    # dusk-tx does (CONFIG_FILES, DUSK_TX_BIN, RUST_LOG, AWS_*), and the env
    # file's own secrets (EVM_DEPLOYER_KEY, DUSK_CONSENSUS_PASSWORD) aren't
    # the agents' to hold.
    (cd "$AGENT_RUN_DIR" && exec setsid env -i PATH="$PATH" HOME="$HOME" DUSK_TX_BIN="$DUSK_TX" \
        CONFIG_FILES="$AGENTS_DIR/$name.json" "$@" "$AGENT_BIN_DIR/$bin") \
        >>"$AGENTS_DIR/logs/$name.log" 2>&1 </dev/null &
    echo "$!" > "$AGENTS_DIR/$name.pid"
    sleep 2
    is_running "$name" || fail "$name exited at startup; see $AGENTS_DIR/logs/$name.log"
    ok "$name started (pid $(pid_of "$name"), log $AGENTS_DIR/logs/$name.log)"
}

wait_until() {
    local what="$1" timeout="$2"; shift 2
    local start
    start="$(date +%s)"
    until "$@"; do
        [ $(($(date +%s) - start)) -lt "$timeout" ] || fail "timed out after ${timeout}s waiting for $what"
        sleep 3
    done
}

dusk_announced_at_least() {
    local count
    count="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$(jq -r .dusk.validator_announce "$OUT_DIR/deployment.json")" \
        --method announced_validator_count --return-type u32 2>/dev/null | jq -r '.value // 0')"
    [ "${count:-0}" -ge "$1" ]
}

evm_announced() {
    cast call "$EVM_VALIDATOR_ANNOUNCE" "getAnnouncedValidators()(address[])" --rpc-url "$EVM_RPC" \
        | tr '[:upper:]' '[:lower:]' | grep -q "$(cast wallet address --private-key "$1" | tr '[:upper:]' '[:lower:]')"
}

write_configs() {
    local i port=9091 cfg
    mkdir -p "$AGENTS_DIR/logs" "$AGENTS_DIR/checkpoints" "$AGENT_STATE_DIR"
    # The relayer sends up to DUSK_RELAYER_MAX_BATCH_SIZE deliveries to Dusk
    # together, as consecutive transactions, as agent-configs sets it.
    jq --argjson batch "$DUSK_RELAYER_MAX_BATCH_SIZE" \
        '.allowLocalCheckpointSyncers = true | .chains.dusk.maxBatchSize = $batch' \
        "$OUT_DIR/relayer.json" > "$AGENTS_DIR/relayer.json"
    for i in "${!DUSK_ORIGIN_VALIDATOR_KEYS[@]}"; do
        cfg="validator-dusk-$((i + 1))"
        jq --arg db "$AGENT_STATE_DIR/$cfg-db" --arg cursors "$AGENT_STATE_DIR/$cfg-events" \
            --arg cp "$AGENTS_DIR/checkpoints/$cfg" --argjson port "$port" \
            '.db = $db | .chains.dusk.eventCursorDir = $cursors | .metricsPort = $port
             | .checkpointSyncer = {type: "localStorage", path: $cp}' \
            "$OUT_DIR/validator-dusk.json" > "$AGENTS_DIR/$cfg.json"
        port=$((port + 1))
    done
    jq --arg evm "$EVM_CHAIN_NAME" --arg db "$AGENT_STATE_DIR/validator-evm-db" \
        --arg cp "$AGENTS_DIR/checkpoints/validator-evm" --argjson port "$port" \
        '{db: $db, originChainName: $evm, interval: 5, metricsPort: $port,
          checkpointSyncer: {type: "localStorage", path: $cp},
          chains: {($evm): .chains[$evm]}}' \
        "$OUT_DIR/relayer.json" > "$AGENTS_DIR/validator-evm.json"
}

cmd_start() {
    [[ "$EVM_RPC" =~ ^http://(localhost|127\.0\.0\.1)[:/] ]] \
        || fail "local-agents.sh uses public development keys and is for local rehearsals only (EVM_RPC is $EVM_RPC)"
    [[ "$DUSK_RELAYER_MAX_BATCH_SIZE" =~ ^[1-9][0-9]*$ ]] && [ "$DUSK_RELAYER_MAX_BATCH_SIZE" -le 64 ] \
        || fail "DUSK_RELAYER_MAX_BATCH_SIZE must be a whole number from 1 to 64, not '$DUSK_RELAYER_MAX_BATCH_SIZE'"
    [ -f "$OUT_DIR/relayer.json" ] && [ -f "$OUT_DIR/validator-dusk.json" ] \
        || fail "No agent configs in $OUT_DIR; run: TESTNET_ENV=$ENV_FILE bash deploy/testnet.sh agent-configs"
    [ -x "$AGENT_BIN_DIR/relayer" ] && [ -x "$AGENT_BIN_DIR/validator" ] \
        || fail "Agent binaries not found in $AGENT_BIN_DIR (in hyperlane-monorepo/rust/main: cargo build --bin relayer --bin validator)"

    # The development keys must be the validators the deployment trusts.
    local key address
    address="$(cast wallet address --private-key "$EVM_ORIGIN_VALIDATOR_KEY")"
    grep -qi "$address" <<<"$EVM_ORIGIN_VALIDATORS" || fail "EVM_ORIGIN_VALIDATORS does not include $address (Anvil #2)"
    for key in "${DUSK_ORIGIN_VALIDATOR_KEYS[@]}"; do
        address="$(cast wallet address --private-key "$key")"
        grep -qi "$address" <<<"$DUSK_ORIGIN_VALIDATORS" || fail "DUSK_ORIGIN_VALIDATORS does not include $address"
    done

    if [ ! -f "$DUSK_SIGNER_KEY_FILE" ]; then
        mkdir -p "$(dirname "$DUSK_SIGNER_KEY_FILE")"
        "$DUSK_TX" export-signer-key --keys "$DUSK_KEYS" --out "$DUSK_SIGNER_KEY_FILE" >/dev/null \
            || fail "Could not export the Dusk signer key to $DUSK_SIGNER_KEY_FILE"
        ok "Wrote the Dusk signer key file $DUSK_SIGNER_KEY_FILE"
    fi
    write_configs

    start_agent validator-evm validator \
        HYP_VALIDATOR_KEY="$EVM_ORIGIN_VALIDATOR_KEY" "$EVM_SIGNER_ENV=$EVM_ORIGIN_VALIDATOR_KEY"
    wait_until "the EVM-origin validator's announcement" 60 evm_announced "$EVM_ORIGIN_VALIDATOR_KEY"
    local i
    for i in "${!DUSK_ORIGIN_VALIDATOR_KEYS[@]}"; do
        start_agent "validator-dusk-$((i + 1))" validator HYP_VALIDATOR_KEY="${DUSK_ORIGIN_VALIDATOR_KEYS[$i]}"
        wait_until "Dusk validator $((i + 1))'s announcement" 120 dusk_announced_at_least "$((i + 1))"
    done
    start_agent relayer relayer "$EVM_SIGNER_ENV=$RELAYER_EVM_KEY"
    ok "All agents running. Stop them with: bash deploy/local-agents.sh stop"
}

cmd_stop() {
    local name pid
    for name in $(agent_names); do
        if is_running "$name"; then
            pid="$(pid_of "$name")"
            kill "$pid" 2>/dev/null || true
            for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
            kill -9 "$pid" 2>/dev/null || true
            ok "Stopped $name (pid $pid)"
        fi
        rm -f "$AGENTS_DIR/$name.pid"
    done
}

cmd_status() {
    local name
    for name in $(agent_names); do
        if is_running "$name"; then
            echo "$name: running (pid $(pid_of "$name"))"
        else
            echo "$name: stopped"
        fi
    done
}

case "${1:-}" in
    start) cmd_start ;;
    stop) cmd_stop ;;
    status) cmd_status ;;
    *) fail "usage: bash deploy/local-agents.sh start|status|stop" ;;
esac
