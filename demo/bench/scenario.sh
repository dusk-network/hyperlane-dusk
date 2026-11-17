#!/usr/bin/env bash
# =============================================================================
# Local agents benchmark: the whole run, phase by phase
# =============================================================================
#
# Starts a fresh local environment, deploys the testnet rehearsal, runs the
# agents behind the proxy and goes through these phases, each recorded in
# $BENCH_DIR/phases.jsonl for report.py:
#
#   baseline   node and Anvil with no agents                (BASELINE_SECS)
#   startup    validators announce one by one, relayer starts
#   idle       all five agents, no traffic                   (IDLE_SECS)
#   d2e        D2E_STEADY Dusk -> EVM transfers, each sent once the last is
#              in a block
#   d2e-backlog the relayer stops while D2E_BACKLOG more are sent, then
#              restarts and finds them all waiting
#   e2d        E2D_STEADY EVM -> Dusk transfers, one every E2D_PERIOD seconds
#   e2d-burst  E2D_BURST at once, with a relayer restart RESTART_AFTER
#              seconds after it has sent the first of their deliveries
#   outage     the agents' Rusk ports refuse connections for OUTAGE_SECS while
#              OUTAGE_TRANSFERS EVM -> Dusk transfers are sent
#   idle2      no traffic again, after the load                (IDLE2_SECS)
#
#   BENCH_DIR=/scratch/dir DUSK_MULTICALL_WASM=/path/to/dusk_multicall.wasm \
#       bash demo/bench/scenario.sh
#   python3 demo/bench/report.py $BENCH_DIR
#
# PHASES names the load phases to run, in the order above (all by default);
# the idle phases always run. Agent binaries (AGENT_BIN_DIR, AGENT_RUN_DIR),
# Multicall for Dusk (DUSK_MULTICALL_WASM, BATCH_READS), pipelined
# deliveries (PIPELINE_DELIVERIES, DUSK_RELAYER_MAX_BATCH_SIZE) and pacing at
# the proxy (PACE_RELAYER) are set as for agents.sh.

set -euo pipefail

DUSK_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
: "${BENCH_DIR:?set BENCH_DIR to an empty scratch directory}"
mkdir -p "$BENCH_DIR/logs" "$BENCH_DIR/tmp"
BENCH_DIR="$(cd "$BENCH_DIR" && pwd)"
export BENCH_DIR
export PID_FILE="$BENCH_DIR/env.pids" BRIDGE_STATE_FILE="$BENCH_DIR/bridge-state.json"
# shellcheck source=/dev/null
source "$DUSK_DIR/demo/.env.bridge"
DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
export DUSK_TX

BASELINE_SECS="${BASELINE_SECS:-180}"
WARMUP_SECS="${WARMUP_SECS:-120}"
IDLE_SECS="${IDLE_SECS:-660}"
D2E_STEADY="${D2E_STEADY:-60}"
D2E_BACKLOG="${D2E_BACKLOG:-40}"
D2E_AMOUNT="${D2E_AMOUNT:-1000000}"
E2D_STEADY="${E2D_STEADY:-60}"
E2D_PERIOD="${E2D_PERIOD:-25}"
E2D_BURST="${E2D_BURST:-40}"
E2D_AMOUNT="${E2D_AMOUNT:-500000}"
RESTART_AFTER="${RESTART_AFTER:-2}"
OUTAGE_SECS="${OUTAGE_SECS:-90}"
OUTAGE_TRANSFERS="${OUTAGE_TRANSFERS:-6}"
IDLE2_SECS="${IDLE2_SECS:-480}"
DELIVERY_TIMEOUT="${DELIVERY_TIMEOUT:-1800}"
PHASES="${PHASES:-d2e d2e-backlog e2d e2d-burst outage}"

A=(bash "$DUSK_DIR/demo/bench/agents.sh")
L=(bash "$DUSK_DIR/demo/bench/load.sh")
info() { echo "[INFO] $(date +%T) $*" >&2; }
fail() { echo "[FAIL] $(date +%T) $*" >&2; exit 1; }
mark() { "${A[@]}" mark "$1" "$2"; info "phase $1 $2"; }
pause() { local end=$(($(date +%s) + $1)); while [ "$(date +%s)" -lt "$end" ]; do sleep 1; done; }
want() { [[ " $PHASES " == *" $1 "* ]]; }
for phase in $PHASES; do
    [[ " d2e d2e-backlog e2d e2d-burst outage " == *" $phase "* ]] || fail "unknown phase '$phase' in PHASES"
done
[ -n "${DUSK_MULTICALL_WASM:-}" ] || [ "${BATCH_READS:-1}" = 0 ] \
    || fail "Set DUSK_MULTICALL_WASM to Multicall for Dusk's v0.1.0 Wasm, which the agents batch their reads through, or BATCH_READS=0"

cleanup() {
    "${A[@]}" stop >/dev/null 2>&1 || true
    "${A[@]}" sampler-stop >/dev/null 2>&1 || true
    "${A[@]}" proxy-stop >/dev/null 2>&1 || true
    cp /tmp/rusk-dev.log "$BENCH_DIR/logs/rusk.log" 2>/dev/null || true
    cp /tmp/anvil.log "$BENCH_DIR/logs/anvil.log" 2>/dev/null || true
    bash "$DUSK_DIR/demo/stop-env.sh" --force >/dev/null 2>&1 || true
}
trap cleanup EXIT

HARNESS_EVM="http://127.0.0.1:18209"
HARNESS_DUSK="http://127.0.0.1:18109/"
state() { jq -r "$1" "$BENCH_DIR/deploy/deployment.json"; }
evm_balance() {
    cast call "$(state .evm.token)" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" \
        --rpc-url "$HARNESS_EVM" | awk '{print $1}'
}
# The native route's DUSK that no escrow holds: a delivery to Dusk takes
# from it, whether it pays the deployer or waits in escrow for its claim.
route_locked() {
    local custody pending
    custody="$("$DUSK_TX" query --rues-url "$HARNESS_DUSK" \
        --contract 0100000000000000000000000000000000000000000000000000000000000000 \
        --method contract_balance --return-type u64 --arg-bytes32 "$(state .dusk.warp_native)" \
        | jq -r '.value')"
    pending="$("$DUSK_TX" query --rues-url "$HARNESS_DUSK" --contract "$(state .dusk.warp_native)" \
        --method pending_total --return-type u64 | jq -r '.value')"
    echo $((custody - pending))
}
wait_for() {
    # wait_for <label> <getter> <target>
    local start
    start="$(date +%s)"
    until [ "$($2)" = "$3" ]; do
        [ $(($(date +%s) - start)) -lt "$DELIVERY_TIMEOUT" ] || fail "timed out waiting for $1"
        sleep 10
    done
    info "$1 reached $3"
}
# Waits until the relayer has sent a delivery to Dusk (a dusk-tx `process`
# that succeeded) that it started at or after $1.
wait_first_delivery_since() {
    local start
    start="$(date +%s)"
    until jq -e -s --argjson since "$1" 'any(.[]; .agent == "relayer" and .fn == "process" and .rc == 0
            and .ts >= $since)' "$BENCH_DIR/logs/dusk-tx.jsonl" >/dev/null 2>&1; do
        [ $(($(date +%s) - start)) -lt "$DELIVERY_TIMEOUT" ] || fail "timed out waiting for the relayer to deliver to Dusk"
        sleep 0.5
    done
}
gql() { curl -sf -m 60 -X POST "${HARNESS_DUSK%/}/on/graphql/query" --data "$1"; }

info "Starting a fresh local environment..."
bash "$DUSK_DIR/demo/stop-env.sh" --force >/dev/null 2>&1 || true
TMPDIR="$BENCH_DIR/tmp" SKIP_OTTERSCAN=true SKIP_DUSK_EXPLORER=true \
    bash "$DUSK_DIR/demo/start-env.sh" > "$BENCH_DIR/logs/start-env.log" 2>&1 \
    || fail "start-env.sh failed; see $BENCH_DIR/logs/start-env.log"
info "Deploying the rehearsal..."
"${A[@]}" setup > "$BENCH_DIR/logs/setup.log" 2>&1 || fail "setup failed; see $BENCH_DIR/logs/setup.log"
"${A[@]}" proxy-start
"${A[@]}" sampler-start

mark baseline start; pause "$BASELINE_SECS"; mark baseline end

mark startup start
"${A[@]}" start
mark startup end
mark warmup start; pause "$WARMUP_SECS"; mark warmup end

mark idle start; pause "$IDLE_SECS"; mark idle end

# Dusk -> EVM. The relayer sends no Dusk transactions in this direction, so
# the load can use the development account the agents share.
if want d2e; then
    mark d2e start
    evm_target="$(python3 -c 'import sys; print(int(sys.argv[1]) + int(sys.argv[2]) * int(sys.argv[3]))' \
        "$(evm_balance)" "$D2E_STEADY" "$D2E_AMOUNT")"
    "${L[@]}" dusk-to-evm "$D2E_STEADY" "$D2E_AMOUNT"
    wait_for "EVM balance after the Dusk -> EVM transfers" evm_balance "$evm_target"
    mark d2e end
fi

# The relayer is down while more Dusk -> EVM transfers are sent, then finds
# them all waiting when it restarts.
if want d2e-backlog; then
    mark d2e-backlog start
    evm_target="$(python3 -c 'import sys; print(int(sys.argv[1]) + int(sys.argv[2]) * int(sys.argv[3]))' \
        "$(evm_balance)" "$D2E_BACKLOG" "$D2E_AMOUNT")"
    "${A[@]}" stop relayer
    "${L[@]}" dusk-to-evm "$D2E_BACKLOG" "$D2E_AMOUNT"
    mark relayer-restart-1 start
    "${A[@]}" start relayer
    wait_for "EVM balance after the relayer restart" evm_balance "$evm_target"
    mark relayer-restart-1 end
    mark d2e-backlog end
fi

# EVM -> Dusk, paced below one per Dusk block.
if want e2d; then
    mark e2d start
    locked="$(route_locked)"
    "${L[@]}" evm-to-dusk "$E2D_STEADY" "$E2D_PERIOD" "$E2D_AMOUNT"
    locked=$((locked - E2D_STEADY * E2D_AMOUNT))
    wait_for "route custody after the paced EVM -> Dusk transfers" route_locked "$locked"
    mark e2d end
fi

# A burst, with a relayer restart while it is sending their deliveries.
if want e2d-burst; then
    mark e2d-burst start
    locked="$(route_locked)"
    burst_start="$(date +%s.%N)"
    "${L[@]}" evm-to-dusk-burst "$E2D_BURST" "$E2D_AMOUNT"
    wait_first_delivery_since "$burst_start"
    pause "$RESTART_AFTER"
    mark relayer-restart-2 start
    "${A[@]}" stop relayer
    "${A[@]}" start relayer
    mark relayer-restart-2 end
    locked=$((locked - E2D_BURST * E2D_AMOUNT))
    wait_for "route custody after the EVM -> Dusk burst" route_locked "$locked"
    mark e2d-burst end
fi

# The agents lose the node: their Rusk ports refuse connections.
if want outage; then
    mark outage start
    locked="$(route_locked)"
    "${A[@]}" proxy-down "relayer@rusk,validator-dusk-1@rusk,validator-dusk-2@rusk,validator-dusk-3@rusk" >/dev/null
    "${L[@]}" evm-to-dusk "$OUTAGE_TRANSFERS" "$((OUTAGE_SECS / OUTAGE_TRANSFERS))" "$E2D_AMOUNT"
    "${A[@]}" proxy-up "relayer@rusk,validator-dusk-1@rusk,validator-dusk-2@rusk,validator-dusk-3@rusk" >/dev/null
    mark outage-restored start
    locked=$((locked - OUTAGE_TRANSFERS * E2D_AMOUNT))
    wait_for "route custody after the outage" route_locked "$locked"
    mark outage-restored end
    mark outage end
fi

mark idle2 start; pause "$IDLE2_SECS"; mark idle2 end

"${A[@]}" stop

# What report.py needs from the chains: the block of each Dusk dispatch, the
# outcome of every transaction the agents sent to Dusk (a delivery sent with
# --no-wait reports no block), the Mailbox's transactions by block, and the
# receipts of the relayer's EVM deliveries.
info "Collecting dispatch heights, Dusk transactions and delivery receipts..."
mailbox="$(state .dusk.mailbox)"
count="$("$DUSK_TX" query --rues-url "$HARNESS_DUSK" --contract "$mailbox" --method nonce \
    --return-type u32 | jq -r .value)"
for ((n = 0; n < count; n++)); do
    "$DUSK_TX" query --rues-url "$HARNESS_DUSK" --contract "$mailbox" --method dispatched_block_height \
        --return-type u64 --arg-u32 "$n" | jq -c --argjson n "$n" '{nonce: $n, height: .value}'
done > "$BENCH_DIR/logs/dusk-dispatches.jsonl"
jq -r 'select(.result.tx != null) | .result.tx' "$BENCH_DIR/logs/dusk-tx.jsonl" | sort -u | while read -r hash; do
    gql "query { tx(hash: \"$hash\") { id blockHeight gasSpent err } }" \
        | jq -c --arg hash "$hash" 'if .errors then error("GraphQL error") else .tx // {id: $hash, missing: true} end' \
        || echo "[WARN] couldn't read transaction $hash back" >&2
done > "$BENCH_DIR/logs/dusk-txs.jsonl"
# Rusk reads at most 200 blocks a query.
tip="$(gql 'query { block(height: -1) { header { height } } }' | jq -r '.block.header.height')"
for ((from = 0; from <= tip; from += 200)); do
    to=$((from + 199 < tip ? from + 199 : tip))
    gql "query { blockTxs(range: [$from, $to], contract: \"$mailbox\") {
        id blockHeight gasSpent err tx { callData { fnName data } } } }" \
        | jq -c '.blockTxs[] | {id, height: .blockHeight, gas: .gasSpent, err, fn: .tx.callData.fnName,
            data: .tx.callData.data}' \
        || echo "[WARN] couldn't read the Mailbox's transactions in blocks $from to $to" >&2
done > "$BENCH_DIR/logs/dusk-mailbox-txs.jsonl"
jq -r 'select(.agent == "relayer" and (.kind | startswith("eth_sendRawTransaction"))) | .txhash // empty' \
    "$BENCH_DIR/logs/requests.jsonl" | sort -u | while read -r hash; do
    cast receipt "$hash" --json --rpc-url "$HARNESS_EVM" \
        | jq -c '{tx: .transactionHash, block: .blockNumber, gas: .gasUsed, status: .status}'
done > "$BENCH_DIR/logs/evm-receipts.jsonl"
info "Done. Report: python3 demo/bench/report.py $BENCH_DIR"
