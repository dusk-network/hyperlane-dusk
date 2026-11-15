#!/usr/bin/env bash
# =============================================================================
# Load for the local agents benchmark: warp transfers on the native route
# =============================================================================
#
# Sends transfers the way docs/testnet.md ("Check it works") does, through
# the proxy's loadgen ports, and logs one JSON line per transfer to
# $BENCH_DIR/logs/load.jsonl. Run after `demo/bench/agents.sh setup`.
#
#   load.sh dusk-to-evm <count> [amount]           one after another
#   load.sh evm-to-dusk <count> <period> [amount]   one every <period> seconds
#   load.sh evm-to-dusk-burst <count> [amount]      all at once
#
# Amounts are in LUX (the EVM token has 9 decimals, so the same units).
# dusk-tx returns once its transaction is included and the next one takes the
# account's next nonce, so Dusk -> EVM transfers from one account go one per
# two blocks: Rusk builds a block's candidate when its round starts, before
# the next transfer arrives. The relayer sends its Dusk transactions from the
# same local development account: don't run dusk-to-evm while the relayer is
# delivering to Dusk, or the two compete for the account's nonce.

set -euo pipefail

DUSK_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
: "${BENCH_DIR:?set BENCH_DIR to the benchmark state directory}"
# shellcheck source=/dev/null
source "$DUSK_DIR/demo/.env.bridge"
DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
STATE="$BENCH_DIR/deploy/deployment.json"
LOG="$BENCH_DIR/logs/load.jsonl"
DUSK_URL="http://127.0.0.1:${LOADGEN_DUSK_PORT:-18108}/"
EVM_URL="http://127.0.0.1:${LOADGEN_EVM_PORT:-18208}"
SENDER_KEY="${ANVIL_PRIVATE_KEY}"

fail() { echo "[FAIL] $*" >&2; exit 1; }
now() { date +%s.%N; }

ROUTE="$(jq -r .dusk.warp_native "$STATE")"
TOKEN="$(jq -r .evm.token "$STATE")"
DUSK_DOMAIN="$(jq -r .dusk_domain "$STATE")"
EVM_DOMAIN="$(jq -r .evm_domain "$STATE")"

dusk_to_evm() {
    local count="$1" amount="${2:-1000000}" i start out rc
    for i in $(seq 1 "$count"); do
        start="$(now)"
        rc=0
        out="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote --native \
            --rues-url "$DUSK_URL" --keys "$CONSENSUS_KEYS" --warp-contract "$ROUTE" \
            --destination "$EVM_DOMAIN" --recipient "${ANVIL_DEPLOYER#0x}" --amount "$amount" \
            2>>"$BENCH_DIR/logs/load.stderr")" || rc=$?
        jq -cn --argjson ts "$start" --argjson end "$(now)" --argjson i "$i" --argjson rc "$rc" \
            --arg out "$out" '{ts: $ts, end: $end, dir: "dusk-to-evm", i: $i, rc: $rc}
              + (($out | fromjson? // {error: $out[0:300]})
                 | {tx: .tx_id, height: .block_height, gas: .gas_spent, error})' >> "$LOG"
        [ "$rc" -eq 0 ] || echo "[WARN] Dusk -> EVM transfer $i failed: ${out:0:300}" >&2
    done
}

evm_send() {
    # evm_send <i> <amount> [extra cast args]: one transferRemote; logs its receipt.
    local i="$1" amount="$2" start out rc=0
    shift 2
    start="$(now)"
    out="$(cast send "$TOKEN" "transferRemote(uint32,bytes32,uint256)" "$DUSK_DOMAIN" "0x$RECIPIENT" \
        "$amount" --value "$QUOTE" --rpc-url "$EVM_URL" --private-key "$SENDER_KEY" --json "$@" \
        2>>"$BENCH_DIR/logs/load.stderr")" || rc=$?
    jq -cn --argjson ts "$start" --argjson end "$(now)" --argjson i "$i" --argjson rc "$rc" \
        --arg dir "${DIRECTION:-evm-to-dusk}" --arg out "$out" \
        '{ts: $ts, end: $end, dir: $dir, i: $i, rc: $rc}
          + (($out | fromjson?) as $r | if $r == null then {error: $out[0:300]}
             elif ($r | type) == "string" then {tx: $r}
             else {tx: $r.transactionHash, block: ($r.blockNumber | tonumber? // .),
                   status: $r.status, dispatch: ([$r.logs[]?
                     | select(.topics[0] == "0x769f711d20c679153d382254f59892613b58a97cc876b249134ac25c80f9c814")
                     | .data][0])} end)' >> "$LOG"
    [ "$rc" -eq 0 ] || echo "[WARN] EVM -> Dusk transfer $i failed: ${out:0:300}" >&2
}

evm_setup() {
    RECIPIENT="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" account --offline \
        --keys "$CONSENSUS_KEYS" | jq -er .account_h256)"
    QUOTE="$(cast call "$TOKEN" "quoteGasPayment(uint32)(uint256)" "$DUSK_DOMAIN" --rpc-url "$EVM_URL" | awk '{print $1}')"
}

evm_to_dusk() {
    local count="$1" period="$2" amount="${3:-500000}" i next
    evm_setup
    next="$(date +%s)"
    for i in $(seq 1 "$count"); do
        evm_send "$i" "$amount"
        next=$((next + period))
        while [ "$(date +%s)" -lt "$next" ]; do sleep 0.2; done
    done
}

evm_to_dusk_burst() {
    local count="$1" amount="${2:-500000}" i nonce
    evm_setup
    nonce="$(cast nonce "$(cast wallet address --private-key "$SENDER_KEY")" --rpc-url "$EVM_URL")"
    for i in $(seq 1 "$count"); do
        # A fixed limit: each parallel estimate sees the state before the burst,
        # and a later Merkle insertion can cost more than it did then.
        DIRECTION=evm-to-dusk-burst evm_send "$i" "$amount" --nonce "$((nonce + i - 1))" \
            --gas-limit 400000 &
    done
    wait
}

mkdir -p "$(dirname "$LOG")"
case "${1:-}" in
    dusk-to-evm) dusk_to_evm "${2:?count}" "${3:-}" ;;
    evm-to-dusk) evm_to_dusk "${2:?count}" "${3:?period}" "${4:-}" ;;
    evm-to-dusk-burst) evm_to_dusk_burst "${2:?count}" "${3:-}" ;;
    *) fail "usage: load.sh dusk-to-evm <count> [amount] | evm-to-dusk <count> <period> [amount] | evm-to-dusk-burst <count> [amount]" ;;
esac
