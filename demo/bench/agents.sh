#!/usr/bin/env bash
# =============================================================================
# Local agents benchmark: deployment, proxy, agents and sampler
# =============================================================================
#
# Runs the local rehearsal of the testnet deployment (docs/testnet.md,
# "Rehearse locally") with every agent behind proxy.py, one proxy port per
# agent and chain, so each request to Rusk or Anvil is logged with the agent
# that sent it. The agents are the rehearsal's: a relayer, three Dusk-origin
# validators (2 of 3) and one EVM-origin validator (1 of 1), with the configs
# `deploy/testnet.sh agent-configs` writes. Local development keys only.
#
#   export BENCH_DIR=/some/scratch/dir         # all state, logs and configs
#   SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true PID_FILE=$BENCH_DIR/env.pids \
#       BRIDGE_STATE_FILE=$BENCH_DIR/bridge-state.json bash demo/start-env.sh
#   bash demo/bench/agents.sh setup            # EVM core, testnet.sh deploy, configs
#   bash demo/bench/agents.sh proxy-start
#   bash demo/bench/agents.sh sampler-start
#   bash demo/bench/agents.sh start            # validators, then the relayer
#   bash demo/bench/agents.sh mark idle start  # phase markers for report.py
#   ...                                        # demo/bench/load.sh
#   bash demo/bench/agents.sh stop             # agents; then sampler-stop, proxy-stop
#
# AGENT_BIN_DIR and AGENT_RUN_DIR choose the agent binaries and the directory
# they run in (it must hold Hyperlane's config/ directory); they default to
# hyperlane-monorepo/rust/main. DUSK_TX is the dusk-tx binary.
#
# As on testnet, the agents batch their contract reads through Multicall for
# Dusk and the relayer sends up to 16 deliveries to Dusk together. setup
# deploys the multicall from DUSK_MULTICALL_WASM, its v0.1.0 Wasm (a
# dusk-multicall release asset, or its `make wasm`), at the ID it has on
# testnet, as deploy/local-evm-core.sh does. BATCH_READS=0 leaves
# batchContractAddress out of the configs, and DUSK_RELAYER_MAX_BATCH_SIZE
# sets the relayer's maxBatchSize (16).

set -euo pipefail
umask 077

DUSK_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BENCH_SCRIPTS="$DUSK_DIR/demo/bench"
: "${BENCH_DIR:?set BENCH_DIR to the benchmark state directory}"
mkdir -p "$BENCH_DIR"
BENCH_DIR="$(cd "$BENCH_DIR" && pwd)"
# shellcheck source=/dev/null
source "$DUSK_DIR/demo/.env.bridge"

DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
AGENT_RUN_DIR="${AGENT_RUN_DIR:-$DUSK_DIR/../hyperlane-monorepo/rust/main}"
AGENT_BIN_DIR="${AGENT_BIN_DIR:-$AGENT_RUN_DIR/target/debug}"
ENV_FILE="$BENCH_DIR/rehearsal.env"
AGENTS_DIR="$BENCH_DIR/agents"
LOG_DIR="$BENCH_DIR/logs"
RUSK_URL="http://127.0.0.1:${RUSK_HTTP_PORT}"
ANVIL_URL="http://127.0.0.1:${ANVIL_PORT}"

# Proxy ports, one per agent and chain. loadgen is the load generator's,
# harness the benchmark's own checks; report.py keeps both apart from the
# agents.
PROXY_ADMIN_PORT="${PROXY_ADMIN_PORT:-18099}"
declare -A DUSK_PORT=([relayer]=18100 [validator-dusk-1]=18101 [validator-dusk-2]=18102
    [validator-dusk-3]=18103 [loadgen]=18108 [harness]=18109)
declare -A EVM_PORT=([relayer]=18200 [validator-evm]=18201 [loadgen]=18208 [harness]=18209)
declare -A METRICS_PORT=([relayer]=9090 [validator-dusk-1]=9091 [validator-dusk-2]=9092
    [validator-dusk-3]=9093 [validator-evm]=9094)

# Anvil's public development keys: #0 deploys and sends, #1 is the relayer's
# EVM key, #2 the EVM-origin validator, #3-#5 the Dusk-origin validators.
EVM_DEPLOYER_KEY_DEV=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
RELAYER_EVM_KEY=0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d
EVM_ORIGIN_VALIDATOR_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
DUSK_ORIGIN_VALIDATOR_KEYS=(
    0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
    0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a
    0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba
)
AGENTS=(validator-evm validator-dusk-1 validator-dusk-2 validator-dusk-3 relayer)

# Multicall for Dusk v0.1.0, as deploy/local-evm-core.sh deploys it: its
# Wasm's SHA-256, its published owner and the ID they give with deploy
# nonce 0 on any chain.
MULTICALL_SHA256=65f77a27c5bf6c7bf78797071dadca102ca3a67e5112c42af0242660e03693d9
MULTICALL_OWNER=r2h82mj2ehNhYPphK3GHVpG98MhSsZNCAF9sdSS4nwQJKqVBwihZn7bZXb9VoWVSqtu6onZ8Tkm11zdnavdCfuCD1kS2We4VPxvLeDf73a1sGiG25HUFvCwQJm3Ud8q6KLW
MULTICALL_ID=93de287b7d349d678a0038fe7772e8960e1a84a1b5da2d0b1f62f532616ce1d4
MULTICALL_RECORD="$BENCH_DIR/deploy/multicall.json"
BATCH_READS="${BATCH_READS:-1}"
DUSK_RELAYER_MAX_BATCH_SIZE="${DUSK_RELAYER_MAX_BATCH_SIZE:-16}"

info() { echo "[INFO] $*" >&2; }
ok()   { echo "[OK]   $*" >&2; }
fail() { echo "[FAIL] $*" >&2; exit 1; }

[[ "$BATCH_READS" =~ ^[01]$ ]] || fail "BATCH_READS must be 1 (batch through Multicall for Dusk) or 0, not '$BATCH_READS'"
[[ "$DUSK_RELAYER_MAX_BATCH_SIZE" =~ ^[1-9][0-9]*$ ]] && [ "$DUSK_RELAYER_MAX_BATCH_SIZE" -le 64 ] \
    || fail "DUSK_RELAYER_MAX_BATCH_SIZE must be a whole number from 1 to 64, not '$DUSK_RELAYER_MAX_BATCH_SIZE'"

state() { jq -r "$1" "$BENCH_DIR/deploy/deployment.json"; }
dusk_query() { "$DUSK_TX" query --rues-url "http://127.0.0.1:${DUSK_PORT[harness]}/" "$@"; }

wait_until() {
    local what="$1" timeout="$2" start
    shift 2
    start="$(date +%s)"
    until "$@"; do
        [ $(($(date +%s) - start)) -lt "$timeout" ] || fail "timed out after ${timeout}s waiting for $what"
        sleep 3
    done
}

# ── setup ────────────────────────────────────────────────────────────────────

cmd_setup() {
    [ ! -e "$ENV_FILE" ] || fail "$ENV_FILE exists; use a fresh BENCH_DIR"
    cast block-number --rpc-url "$ANVIL_URL" >/dev/null 2>&1 || fail "Anvil is not running at $ANVIL_URL"
    local chain_id evm_chain_id
    chain_id="$(curl -sf -m 10 -X POST -H 'Content-Type: application/octet-stream' \
        "$RUSK_URL/on/contracts:0100000000000000000000000000000000000000000000000000000000000000/chain_id" \
        | od -An -tu1 | tr -d ' \n')"
    [[ "$chain_id" =~ ^[0-9]+$ ]] || fail "Rusk is not running at $RUSK_URL"
    evm_chain_id="$(cast chain-id --rpc-url "$ANVIL_URL")"

    # Multicall for Dusk, which the local genesis lacks, at the ID it has on
    # testnet, as deploy/local-evm-core.sh deploys it. dusk-tx checks that the
    # Wasm, nonce and owner give that ID before it sends anything.
    local dusk_multicall=""
    mkdir -p "$BENCH_DIR/deploy"
    if [ -n "${DUSK_MULTICALL_WASM:-}" ]; then
        [ "$(sha256sum "$DUSK_MULTICALL_WASM" 2>/dev/null | awk '{print $1}')" = "$MULTICALL_SHA256" ] \
            || fail "DUSK_MULTICALL_WASM ($DUSK_MULTICALL_WASM) isn't Multicall for Dusk v0.1.0, whose Wasm has SHA-256 $MULTICALL_SHA256"
        info "Deploying Multicall for Dusk..."
        DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" deploy-wasm --rues-url "$RUSK_URL/" \
            --keys "$CONSENSUS_KEYS" --wasm "$DUSK_MULTICALL_WASM" --owner "$MULTICALL_OWNER" --deploy-nonce 0 \
            --expected-id "$MULTICALL_ID" --expected-chain-id "$chain_id" > "$MULTICALL_RECORD" \
            || fail "Multicall for Dusk deployment failed: $(jq -r '.error // empty' "$MULTICALL_RECORD" 2>/dev/null)"
        # testnet.sh checks the multicall and names it in its configs only
        # when the agents are to batch through it.
        [ "$BATCH_READS" = 0 ] || dusk_multicall="$MULTICALL_ID"
        ok "Multicall for Dusk at $MULTICALL_ID"
    elif [ "$BATCH_READS" = 1 ]; then
        fail "Set DUSK_MULTICALL_WASM to Multicall for Dusk's v0.1.0 Wasm, which the agents batch their reads through, or BATCH_READS=0"
    fi

    # Hyperlane core on Anvil, as deploy/local-evm-core.sh deploys it, without
    # touching deploy/local.env or deploy/out/.
    info "Deploying the EVM core on Anvil..."
    deploy() {
        (cd "$SOLIDITY_DIR" && forge create "$1" --broadcast --json --rpc-url "$ANVIL_URL" \
            --private-key "$EVM_DEPLOYER_KEY_DEV" "${@:2}") | jq -er '.deployedTo'
    }
    send() { cast send "$@" --rpc-url "$ANVIL_URL" --private-key "$EVM_DEPLOYER_KEY_DEV" >/dev/null; }
    local mailbox merkle protocol_fee default_hook default_ism va aggregation_factory deployer
    deployer="$(cast wallet address --private-key "$EVM_DEPLOYER_KEY_DEV")"
    mailbox="$(deploy contracts/Mailbox.sol:Mailbox --constructor-args "$evm_chain_id")"
    merkle="$(deploy contracts/hooks/MerkleTreeHook.sol:MerkleTreeHook --constructor-args "$mailbox")"
    protocol_fee="$(deploy contracts/hooks/ProtocolFee.sol:ProtocolFee \
        --constructor-args 1000000000 1 "$deployer" "$deployer")"
    default_hook="$(deploy contracts/hooks/routing/FallbackDomainRoutingHook.sol:FallbackDomainRoutingHook \
        --constructor-args "$mailbox" "$deployer" "$merkle")"
    default_ism="$(deploy contracts/test/TestIsm.sol:TestIsm)"
    va="$(deploy contracts/isms/multisig/ValidatorAnnounce.sol:ValidatorAnnounce --constructor-args "$mailbox")"
    aggregation_factory="$(deploy contracts/hooks/aggregation/StaticAggregationHookFactory.sol:StaticAggregationHookFactory)"
    send "$mailbox" "initialize(address,address,address,address)" "$deployer" "$default_ism" "$default_hook" "$protocol_fee"

    # deploy/testnet.sh keeps its state in deploy/out/<DEPLOY_NAME>; a
    # relative name puts it in BENCH_DIR instead.
    local deploy_name
    deploy_name="$(python3 -c 'import os,sys; print(os.path.relpath(sys.argv[1], sys.argv[2]))' \
        "$BENCH_DIR/deploy" "$DUSK_DIR/deploy/out")"
    cat > "$ENV_FILE" <<EOF
# Local rehearsal settings for the agents benchmark. Public development keys only.
source "\$DUSK_DIR/deploy/testnet.env.example"
DEPLOY_NAME='$deploy_name'
DUSK_RUES_URL=$RUSK_URL/
DUSK_EXPECTED_CHAIN_ID=$chain_id
DUSK_KEYS=$CONSENSUS_KEYS
export DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD"
EVM_CHAIN_NAME=anvil
EVM_RPC=$ANVIL_URL
EVM_CHAIN_ID=$evm_chain_id
EVM_DOMAIN=$evm_chain_id
EVM_REORG_PERIOD=0
EVM_BLOCK_TIME=1
EVM_MAILBOX=$mailbox
EVM_MERKLE_TREE_HOOK=$merkle
EVM_VALIDATOR_ANNOUNCE=$va
EVM_ISM_FACTORY=
EVM_AGGREGATION_HOOK_FACTORY=$aggregation_factory
export EVM_DEPLOYER_KEY=$EVM_DEPLOYER_KEY_DEV
EVM_ORIGIN_VALIDATORS=$(cast wallet address --private-key "$EVM_ORIGIN_VALIDATOR_KEY")
EVM_ORIGIN_THRESHOLD=1
DUSK_ORIGIN_VALIDATORS=$(for k in "${DUSK_ORIGIN_VALIDATOR_KEYS[@]}"; do cast wallet address --private-key "$k"; done | paste -sd, -)
DUSK_ORIGIN_THRESHOLD=2
SOLIDITY_DIR=$SOLIDITY_DIR
AGENT_STATE_DIR=$BENCH_DIR/state
DUSK_SIGNER_KEY_FILE=$BENCH_DIR/secrets/dusk-signer.key
DUSK_MULTICALL=$dusk_multicall
DUSK_RELAYER_MAX_BATCH_SIZE=$DUSK_RELAYER_MAX_BATCH_SIZE
EOF
    info "Deploying with deploy/testnet.sh (a few minutes)..."
    TESTNET_ENV="$ENV_FILE" bash "$DUSK_DIR/deploy/testnet.sh" deploy
    TESTNET_ENV="$ENV_FILE" bash "$DUSK_DIR/deploy/testnet.sh" agent-configs

    # Register the deployer on the route, so deliveries to it pay out rather
    # than wait in escrow. Done before any agent shares its Dusk key.
    info "Registering the deployer's account on the native route..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" register-account \
        --rues-url "$RUSK_URL/" --keys "$CONSENSUS_KEYS" --warp-contract "$(state .dusk.warp_native)" \
        --expected-chain-id "$chain_id" >/dev/null
    mkdir -p "$BENCH_DIR/secrets"
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" export-signer-key \
        --keys "$CONSENSUS_KEYS" --out "$BENCH_DIR/secrets/dusk-signer.key" >/dev/null

    # Names for the proxy log.
    jq --arg multicall "$MULTICALL_ID" '{
        "0100000000000000000000000000000000000000000000000000000000000000": "transfer",
        ($multicall): "multicall"
      } + (.dusk | with_entries(select(.value | type == "string" and length == 64)
            | {key: .value, value: (if .key == "ism_multisig" then "ism" else .key end)}))
      + (.evm | with_entries(select(.value | type == "string" and startswith("0x"))
            | {key: (.value | ascii_downcase), value: .key}))' \
        "$BENCH_DIR/deploy/deployment.json" > "$BENCH_DIR/names.json"
    cmd_configs
    ok "Set up in $BENCH_DIR"
}

# ── configs ──────────────────────────────────────────────────────────────────

# Derive one config per agent from the testnet.sh templates, as
# deploy/local-agents.sh does, with each agent's RPC URLs on its proxy ports.
# Every Dusk chain batches its reads through the multicall setup deployed
# (batchContractAddress), unless BATCH_READS=0, and the relayer sends up to
# DUSK_RELAYER_MAX_BATCH_SIZE deliveries together (maxBatchSize), as the
# configs agent-configs writes for testnet do. `configs` alone rewrites them
# for the agents' next start.
cmd_configs() {
    local out="$BENCH_DIR/deploy" i name multicall=""
    if [ "$BATCH_READS" = 1 ]; then
        [ "$(jq -r '.contract_id // empty' "$MULTICALL_RECORD" 2>/dev/null)" = "$MULTICALL_ID" ] \
            || fail "No Multicall for Dusk deployed in $BENCH_DIR (set DUSK_MULTICALL_WASM for setup), or BATCH_READS=0"
        multicall="0x$MULTICALL_ID"
    fi
    # shellcheck disable=SC2016
    local batch_reads='if $multicall == "" then del(.chains.dusk.batchContractAddress)
        else .chains.dusk.batchContractAddress = $multicall end'
    mkdir -p "$AGENTS_DIR" "$LOG_DIR" "$BENCH_DIR/checkpoints" "$BENCH_DIR/state"
    jq --arg dusk "http://127.0.0.1:${DUSK_PORT[relayer]}/" --arg evm "http://127.0.0.1:${EVM_PORT[relayer]}" \
        --arg multicall "$multicall" --argjson batch "$DUSK_RELAYER_MAX_BATCH_SIZE" \
        '.allowLocalCheckpointSyncers = true
         | .chains.dusk.rpcUrls = [{http: $dusk}] | .chains.anvil.rpcUrls = [{http: $evm}]
         | .chains.dusk.maxBatchSize = $batch | '"$batch_reads" \
        "$out/relayer.json" > "$AGENTS_DIR/relayer.json"
    for i in 1 2 3; do
        name="validator-dusk-$i"
        jq --arg db "$BENCH_DIR/state/$name-db" --arg cursors "$BENCH_DIR/state/$name-events" \
            --arg cp "$BENCH_DIR/checkpoints/$name" --argjson port "${METRICS_PORT[$name]}" \
            --arg dusk "http://127.0.0.1:${DUSK_PORT[$name]}/" --arg multicall "$multicall" \
            '.db = $db | .chains.dusk.eventCursorDir = $cursors | .metricsPort = $port
             | .checkpointSyncer = {type: "localStorage", path: $cp}
             | .chains.dusk.rpcUrls = [{http: $dusk}] | '"$batch_reads" \
            "$out/validator-dusk.json" > "$AGENTS_DIR/$name.json"
    done
    jq --arg db "$BENCH_DIR/state/validator-evm-db" --arg cp "$BENCH_DIR/checkpoints/validator-evm" \
        --argjson port "${METRICS_PORT[validator-evm]}" --arg evm "http://127.0.0.1:${EVM_PORT[validator-evm]}" \
        '{db: $db, originChainName: "anvil", interval: 5, metricsPort: $port,
          checkpointSyncer: {type: "localStorage", path: $cp},
          chains: {anvil: (.chains.anvil | .rpcUrls = [{http: $evm}])}}' \
        "$out/relayer.json" > "$AGENTS_DIR/validator-evm.json"
    ok "Agent configs in $AGENTS_DIR: reads batched through ${multicall:-nothing (single queries)}, relayer maxBatchSize $DUSK_RELAYER_MAX_BATCH_SIZE"
}

# ── proxy and sampler ────────────────────────────────────────────────────────

# PACE_RELAYER holds the relayer's contract queries to that many a second at
# the proxy (0, the default: no limit). Rusk's default HTTP policy allows 180
# a second, burst 360, for all clients together. The agents pace their own
# (contractQueriesPerSecond, 100 by default); older ones, without that, sent
# far more while catching up after a restart and kept retrying, and needed
# 100 here (docs/costs.md, "Recovery").
PACE_RELAYER="${PACE_RELAYER:-0}"

cmd_proxy_start() {
    local args=() name
    for name in "${!DUSK_PORT[@]}"; do args+=(--listen "$name=${DUSK_PORT[$name]}:rusk"); done
    for name in "${!EVM_PORT[@]}"; do args+=(--listen "$name=${EVM_PORT[$name]}:evm"); done
    [ "$PACE_RELAYER" = 0 ] || args+=(--pace "relayer@rusk=$PACE_RELAYER")
    mkdir -p "$LOG_DIR"
    setsid python3 "$BENCH_SCRIPTS/proxy.py" --log "$LOG_DIR/requests.jsonl" \
        --names "$BENCH_DIR/names.json" --rusk "$RUSK_URL" --evm "$ANVIL_URL" \
        --domain "$(jq -r .dusk_domain "$BENCH_DIR/deploy/deployment.json")" \
        --domain "$(jq -r .evm_domain "$BENCH_DIR/deploy/deployment.json")" \
        --admin "$PROXY_ADMIN_PORT" "${args[@]}" >>"$LOG_DIR/proxy.out" 2>&1 </dev/null &
    echo "$!" > "$BENCH_DIR/proxy.pid"
    sleep 1
    kill -0 "$(cat "$BENCH_DIR/proxy.pid")" 2>/dev/null || fail "proxy failed to start; see $LOG_DIR/proxy.out"
    ok "proxy running (pid $(cat "$BENCH_DIR/proxy.pid"))"
}

cmd_sampler_start() {
    local args=() name
    # demo/start-env.sh records the node and Anvil as name:pid in PID_FILE.
    sed -n 's/^rusk:\([0-9]*\)$/\1/p' "$PID_FILE" > "$BENCH_DIR/rusk.pid"
    sed -n 's/^anvil:\([0-9]*\)$/\1/p' "$PID_FILE" > "$BENCH_DIR/anvil.pid"
    for name in "${AGENTS[@]}"; do
        args+=(--pidfile "$name=$AGENTS_DIR/$name.pid" --path "$name-log=$LOG_DIR/$name.log")
    done
    args+=(--pidfile "rusk=$BENCH_DIR/rusk.pid" --pidfile "anvil=$BENCH_DIR/anvil.pid"
        --pidfile "proxy=$BENCH_DIR/proxy.pid")
    args+=(--path "relayer-db=$BENCH_DIR/state/relayer-db"
        --path "relayer-events=$BENCH_DIR/state/relayer-dusk-events"
        --path "validator-evm-db=$BENCH_DIR/state/validator-evm-db"
        --path "validator-evm-checkpoints=$BENCH_DIR/checkpoints/validator-evm")
    for name in validator-dusk-1 validator-dusk-2 validator-dusk-3; do
        args+=(--path "$name-db=$BENCH_DIR/state/$name-db" --path "$name-events=$BENCH_DIR/state/$name-events"
            --path "$name-checkpoints=$BENCH_DIR/checkpoints/$name")
    done
    setsid python3 "$BENCH_SCRIPTS/sample.py" --out "$LOG_DIR/resources.jsonl" --interval 5 \
        "${args[@]}" >>"$LOG_DIR/sampler.out" 2>&1 </dev/null &
    echo "$!" > "$BENCH_DIR/sampler.pid"
    ok "sampler running (pid $!)"
}

stop_pidfile() {
    local file="$1" pid
    pid="$(cat "$file" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
        for _ in $(seq 1 20); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
        kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$file"
}

# ── agents ───────────────────────────────────────────────────────────────────

start_agent() {
    local name="$1" bin="$2" pid
    shift 2
    pid="$(cat "$AGENTS_DIR/$name.pid" 2>/dev/null || true)"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        ok "$name is already running (pid $pid)"
        return
    fi
    # The agent gets only these variables, and the keys in "$@". Hyperlane
    # applies every HYP_* variable over its config files, so one left in the
    # operator's shell, such as HYP_CHAINS_DUSK_CUSTOMRPCURLS, would send the
    # agent past the proxy to another node; others change what the agent or
    # its dusk-tx does (CONFIG_FILES, DUSK_TX_BIN, RUST_LOG, AWS_*).
    (cd "$AGENT_RUN_DIR" && exec setsid env -i PATH="$PATH" HOME="$HOME" \
        DUSK_TX_BIN="$BENCH_SCRIPTS/dusk-tx-shim.sh" BENCH_DUSK_TX_REAL="$DUSK_TX" \
        BENCH_DUSK_TX_LOG="$LOG_DIR/dusk-tx.jsonl" BENCH_AGENT="$name" \
        CONFIG_FILES="$AGENTS_DIR/$name.json" "$@" "$AGENT_BIN_DIR/$bin") \
        >>"$LOG_DIR/$name.log" 2>&1 </dev/null &
    echo "$!" > "$AGENTS_DIR/$name.pid"
    printf '{"ts":%s,"event":"start","agent":"%s","pid":%s}\n' "$(date +%s.%N)" "$name" "$!" \
        >> "$BENCH_DIR/phases.jsonl"
    sleep 2
    kill -0 "$!" 2>/dev/null || fail "$name exited at startup; see $LOG_DIR/$name.log"
    ok "$name started (pid $!)"
}

evm_announced() {
    cast call "$(state .evm.validator_announce)" "getAnnouncedValidators()(address[])" \
        --rpc-url "http://127.0.0.1:${EVM_PORT[harness]}" | tr '[:upper:]' '[:lower:]' \
        | grep -q "$(cast wallet address --private-key "$EVM_ORIGIN_VALIDATOR_KEY" | tr '[:upper:]' '[:lower:]')"
}

dusk_announced_at_least() {
    local count
    count="$(dusk_query --contract "$(state .dusk.validator_announce)" --method announced_validator_count \
        --return-type u32 2>/dev/null | jq -r '.value // 0')"
    [ "${count:-0}" -ge "$1" ]
}

cmd_start() {
    local wanted=("$@") name i
    [ "${#wanted[@]}" -gt 0 ] || wanted=("${AGENTS[@]}")
    for name in "${wanted[@]}"; do
        case "$name" in
            validator-evm)
                start_agent "$name" validator HYP_VALIDATOR_KEY="$EVM_ORIGIN_VALIDATOR_KEY" \
                    HYP_CHAINS_ANVIL_SIGNER_KEY="$EVM_ORIGIN_VALIDATOR_KEY"
                wait_until "the EVM-origin validator's announcement" 90 evm_announced ;;
            validator-dusk-[123])
                i="${name##*-}"
                start_agent "$name" validator HYP_VALIDATOR_KEY="${DUSK_ORIGIN_VALIDATOR_KEYS[$((i - 1))]}"
                # The validators announce from one Dusk account; one at a time.
                wait_until "$name's announcement" 180 dusk_announced_at_least "$i" ;;
            relayer)
                start_agent relayer relayer HYP_CHAINS_ANVIL_SIGNER_KEY="$RELAYER_EVM_KEY" ;;
            *) fail "unknown agent $name" ;;
        esac
    done
}

cmd_stop() {
    local wanted=("$@") name
    [ "${#wanted[@]}" -gt 0 ] || wanted=(relayer validator-dusk-3 validator-dusk-2 validator-dusk-1 validator-evm)
    for name in "${wanted[@]}"; do
        if [ -f "$AGENTS_DIR/$name.pid" ]; then
            printf '{"ts":%s,"event":"stop","agent":"%s"}\n' "$(date +%s.%N)" "$name" >> "$BENCH_DIR/phases.jsonl"
            stop_pidfile "$AGENTS_DIR/$name.pid"
            ok "stopped $name"
        fi
    done
}

cmd_mark() {
    printf '{"ts":%s,"phase":"%s","event":"%s"}\n' "$(date +%s.%N)" "$1" "$2" >> "$BENCH_DIR/phases.jsonl"
}

cmd_status() {
    local name pid
    for name in "${AGENTS[@]}" proxy sampler; do
        pid="$(cat "$AGENTS_DIR/$name.pid" "$BENCH_DIR/$name.pid" 2>/dev/null | head -1 || true)"
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then echo "$name: running ($pid)"; else echo "$name: stopped"; fi
    done
}

case "${1:-}" in
    setup) cmd_setup ;;
    configs) cmd_configs ;;
    proxy-start) cmd_proxy_start ;;
    proxy-stop) stop_pidfile "$BENCH_DIR/proxy.pid" ;;
    proxy-down) curl -sf "http://127.0.0.1:$PROXY_ADMIN_PORT/down?names=${2:-}" ;;
    proxy-up) curl -sf "http://127.0.0.1:$PROXY_ADMIN_PORT/up?names=${2:-}" ;;
    sampler-start) cmd_sampler_start ;;
    sampler-stop) stop_pidfile "$BENCH_DIR/sampler.pid" ;;
    start) shift; cmd_start "$@" ;;
    stop) shift; cmd_stop "$@" ;;
    mark) cmd_mark "$2" "$3" ;;
    status) cmd_status ;;
    *) fail "usage: agents.sh setup|configs|proxy-start|proxy-stop|proxy-down [names]|proxy-up [names]|sampler-start|sampler-stop|start [agents]|stop [agents]|mark <phase> start|end|status" ;;
esac
