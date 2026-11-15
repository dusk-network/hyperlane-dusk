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
#              seconds into delivering them
#   outage     the agents' Rusk ports refuse connections for OUTAGE_SECS while
#              OUTAGE_TRANSFERS EVM -> Dusk transfers are sent
#   idle2      no traffic again, after the load                (IDLE2_SECS)
#
#   BENCH_DIR=/scratch/dir bash demo/bench/scenario.sh
#   python3 demo/bench/report.py $BENCH_DIR
#
# Agent binaries (AGENT_BIN_DIR, AGENT_RUN_DIR) and the relayer's pacing
# (PACE_RELAYER) are set as for agents.sh.

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
RESTART_AFTER="${RESTART_AFTER:-60}"
OUTAGE_SECS="${OUTAGE_SECS:-90}"
OUTAGE_TRANSFERS="${OUTAGE_TRANSFERS:-6}"
IDLE2_SECS="${IDLE2_SECS:-480}"
DELIVERY_TIMEOUT="${DELIVERY_TIMEOUT:-1800}"

A=(bash "$DUSK_DIR/demo/bench/agents.sh")
L=(bash "$DUSK_DIR/demo/bench/load.sh")
info() { echo "[INFO] $(date +%T) $*" >&2; }
fail() { echo "[FAIL] $(date +%T) $*" >&2; exit 1; }
mark() { "${A[@]}" mark "$1" "$2"; info "phase $1 $2"; }
pause() { local end=$(($(date +%s) + $1)); while [ "$(date +%s)" -lt "$end" ]; do sleep 1; done; }

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
route_locked() {
    "$DUSK_TX" query --rues-url "$HARNESS_DUSK" \
        --contract 0100000000000000000000000000000000000000000000000000000000000000 \
        --method contract_balance --return-type u64 --arg-bytes32 "$(state .dusk.warp_native)" \
        | jq -r '.value'
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
mark d2e start
evm_target="$(python3 -c 'import sys; print(int(sys.argv[1]) + int(sys.argv[2]) * int(sys.argv[3]))' \
    "$(evm_balance)" "$D2E_STEADY" "$D2E_AMOUNT")"
"${L[@]}" dusk-to-evm "$D2E_STEADY" "$D2E_AMOUNT"
wait_for "EVM balance after the Dusk -> EVM transfers" evm_balance "$evm_target"
mark d2e end

# The relayer is down while more Dusk -> EVM transfers are sent, then finds
# them all waiting when it restarts.
mark d2e-backlog start
evm_target="$(python3 -c 'import sys; print(int(sys.argv[1]) + int(sys.argv[2]) * int(sys.argv[3]))' \
    "$evm_target" "$D2E_BACKLOG" "$D2E_AMOUNT")"
"${A[@]}" stop relayer
"${L[@]}" dusk-to-evm "$D2E_BACKLOG" "$D2E_AMOUNT"
mark relayer-restart-1 start
"${A[@]}" start relayer
wait_for "EVM balance after the relayer restart" evm_balance "$evm_target"
mark relayer-restart-1 end
mark d2e-backlog end

# EVM -> Dusk, paced below one per Dusk block.
mark e2d start
locked="$(route_locked)"
"${L[@]}" evm-to-dusk "$E2D_STEADY" "$E2D_PERIOD" "$E2D_AMOUNT"
locked=$((locked - E2D_STEADY * E2D_AMOUNT))
wait_for "route custody after the paced EVM -> Dusk transfers" route_locked "$locked"
mark e2d end

# A burst, with a relayer restart while it is delivering.
mark e2d-burst start
"${L[@]}" evm-to-dusk-burst "$E2D_BURST" "$E2D_AMOUNT"
pause "$RESTART_AFTER"
mark relayer-restart-2 start
"${A[@]}" stop relayer
"${A[@]}" start relayer
mark relayer-restart-2 end
locked=$((locked - E2D_BURST * E2D_AMOUNT))
wait_for "route custody after the EVM -> Dusk burst" route_locked "$locked"
mark e2d-burst end

# The agents lose the node: their Rusk ports refuse connections.
mark outage start
"${A[@]}" proxy-down "relayer@rusk,validator-dusk-1@rusk,validator-dusk-2@rusk,validator-dusk-3@rusk" >/dev/null
"${L[@]}" evm-to-dusk "$OUTAGE_TRANSFERS" "$((OUTAGE_SECS / OUTAGE_TRANSFERS))" "$E2D_AMOUNT"
"${A[@]}" proxy-up "relayer@rusk,validator-dusk-1@rusk,validator-dusk-2@rusk,validator-dusk-3@rusk" >/dev/null
mark outage-restored start
locked=$((locked - OUTAGE_TRANSFERS * E2D_AMOUNT))
wait_for "route custody after the outage" route_locked "$locked"
mark outage-restored end
mark outage end

mark idle2 start; pause "$IDLE2_SECS"; mark idle2 end

"${A[@]}" stop

# What report.py needs from the chains: the block of each Dusk dispatch, and
# the receipts of the relayer's EVM deliveries.
info "Collecting dispatch heights and delivery receipts..."
mailbox="$(state .dusk.mailbox)"
count="$("$DUSK_TX" query --rues-url "$HARNESS_DUSK" --contract "$mailbox" --method nonce \
    --return-type u32 | jq -r .value)"
for ((n = 0; n < count; n++)); do
    "$DUSK_TX" query --rues-url "$HARNESS_DUSK" --contract "$mailbox" --method dispatched_block_height \
        --return-type u64 --arg-u32 "$n" | jq -c --argjson n "$n" '{nonce: $n, height: .value}'
done > "$BENCH_DIR/logs/dusk-dispatches.jsonl"
jq -r 'select(.agent == "relayer" and (.kind | startswith("eth_sendRawTransaction"))) | .txhash // empty' \
    "$BENCH_DIR/logs/requests.jsonl" | sort -u | while read -r hash; do
    cast receipt "$hash" --json --rpc-url "$HARNESS_EVM" \
        | jq -c '{tx: .transactionHash, block: .blockNumber, gas: .gasUsed, status: .status}'
done > "$BENCH_DIR/logs/evm-receipts.jsonl"
info "Done. Report: python3 demo/bench/report.py $BENCH_DIR"
