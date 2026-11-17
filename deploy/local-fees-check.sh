#!/usr/bin/env bash
# =============================================================================
# Check the relaying fees in a local rehearsal of the testnet deployment
# =============================================================================
#
# After the rehearsal is deployed and its agents run (docs/testnet.md,
# "Rehearse locally"):
#
#   bash deploy/local-fees-check.sh
#
# checks that the relayer delivers what was paid for and nothing else, and
# that the operator collects the fees:
#
#   1. A warp transfer from Dusk pays the Dusk IGP and is delivered.
#   2. Two messages from Dusk to a TestRecipient on Anvil each pay for as
#      much gas as a transfer does. A short one is delivered. One with a
#      2 KiB body, which the recipient stores, takes far more gas than it paid
#      for and isn't.
#   3. A warp transfer from Anvil pays the token's quote, which includes our
#      IGP, and is delivered.
#   4. A message from Anvil sent through the Mailbox's default hook pays our
#      IGP nothing and isn't delivered; paying for half the gas it needs
#      doesn't deliver it; paying for the rest does.
#   5. The 2 KiB message from Dusk, which paid for too little gas, is topped
#      up with `dusk-tx pay-for-gas`: half of what it lacks doesn't deliver
#      it; the rest, found by its nonce, does.
#   6. A burst of transfers from Anvil is delivered, every delivery once and
#      none failing: one at a time, each in a block of its own, as
#      agent-configs writes the relayer's config, or, with the experimental
#      pipelined deliveries turned on in it, several sharing a block.
#   7. `deploy/testnet.sh claim` pays the Dusk IGP's and ProtocolFee's fees
#      to the deployer, and the Anvil IGP's to its beneficiary.
#
# Every Dusk transaction here is signed with DUSK_KEYS, which the rehearsal's
# agents share, so it runs them while the relayer has nothing to deliver to
# Dusk. Local endpoints and development keys only.

set -euo pipefail
umask 077

DUSK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${TESTNET_ENV:-$DUSK_DIR/deploy/local.env}"
[ -f "$ENV_FILE" ] || { echo "error: $ENV_FILE not found (run deploy/local-evm-core.sh)" >&2; exit 1; }
# shellcheck source=/dev/null
source "$ENV_FILE"

DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
STATE="$DUSK_DIR/deploy/out/$DEPLOY_NAME/deployment.json"
RELAYER_LOG="$DUSK_DIR/deploy/out/$DEPLOY_NAME/agents/logs/relayer.log"
RELAYER_CONFIG="$DUSK_DIR/deploy/out/$DEPLOY_NAME/agents/relayer.json"
# How long a delivery may take, and how long an unpaid message is watched.
DELIVERY_TIMEOUT="${DELIVERY_TIMEOUT:-600}"
UNPAID_WATCH="${UNPAID_WATCH:-120}"
# Anvil account #0, the rehearsal's EVM deployer, sends the EVM transfers.
EVM_USER_KEY="$EVM_DEPLOYER_KEY"
# How many transfers from Anvil step 6 sends at once.
BURST="${BURST:-8}"

info() { echo "[INFO] $*" >&2; }
ok()   { echo "[OK]   $*" >&2; }
fail() { echo "[FAIL] $*" >&2; exit 1; }

[[ "$EVM_RPC" =~ ^http://(localhost|127\.0\.0\.1)(:[0-9]+)?(/|$) ]] \
    && [[ "$DUSK_RUES_URL" =~ ^http://(localhost|127\.0\.0\.1)(:[0-9]+)?(/|$) ]] \
    || fail "local-fees-check.sh uses development keys and is for local rehearsals only"
[ -f "$STATE" ] && [ -n "$(jq -r '.evm.token // empty' "$STATE")" ] \
    || fail "No deployment in $STATE; run deploy/testnet.sh deploy"
[ -f "$RELAYER_LOG" ] || fail "No relayer log at $RELAYER_LOG; run deploy/local-agents.sh start"

s() { jq -r "$1" "$STATE"; }
num() { awk 'NR == 1 {print $1}'; }
pad32() { local a="${1#0x}"; printf '000000000000000000000000%s' "${a,,}"; }
dq() { "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" "$@" | jq -er '.value'; }
# A Dusk transaction signed with DUSK_KEYS, for the rehearsal's chain only.
dtx() { "$DUSK_TX" "$@" --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID"; }
daccount() { "$DUSK_TX" account "$@" --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS"; }

EVM_USER="$(cast wallet address --private-key "$EVM_USER_KEY")"
DUSK_IGP="$(s .dusk.igp)" DUSK_PF="$(s .dusk.protocol_fee)" DUSK_MAILBOX="$(s .dusk.mailbox)"
DUSK_ROUTE="$(s .dusk.warp_native)" DUSK_RECIPIENT="$(s .dusk.test_recipient)"
EVM_IGP="$(s .evm.igp)" EVM_TOKEN="$(s .evm.token)"
# The EVM gas a message from Dusk without hook metadata pays for.
PAID_GAS="$(TESTNET_ENV="$ENV_FILE" bash "$DUSK_DIR/deploy/testnet.sh" fees --json | jq -er '.dusk_to_evm.gas')" \
    || fail "Can't read the fee settings"

# wait_until <seconds> <what> <command...>
wait_until() {
    local timeout="$1" what="$2" start
    shift 2
    start="$(date +%s)"
    until "$@" >/dev/null 2>&1; do
        [ $(($(date +%s) - start)) -lt "$timeout" ] || fail "timed out after ${timeout}s waiting for $what"
        sleep 5
    done
    ok "$what, after $(($(date +%s) - start))s"
}
# never_within <seconds> <what> <command...>: the command never succeeds.
never_within() {
    local seconds="$1" what="$2" end
    shift 2
    end=$(($(date +%s) + seconds))
    while [ "$(date +%s)" -lt "$end" ]; do
        ! "$@" >/dev/null 2>&1 || fail "$what"
        sleep 5
    done
}
evm_delivered() { [ "$(cast call "$EVM_MAILBOX" "delivered(bytes32)(bool)" "0x${1#0x}" --rpc-url "$EVM_RPC")" = true ]; }
dusk_delivered() { [ "$(dq --contract "$DUSK_MAILBOX" --method delivered --return-type bool --arg-bytes32 "${1#0x}")" = true ]; }
# The gas the Anvil transaction that delivered a message used. cast logs
# doesn't filter by an indexed argument, so jq does.
evm_delivery_gas() {
    local tx
    tx="$(cast logs --from-block 0 --address "$EVM_MAILBOX" "ProcessId(bytes32)" --rpc-url "$EVM_RPC" --json \
        | jq -r --arg id "0x${1#0x}" '[.[] | select(.topics[1] == $id)][0].transactionHash')"
    cast receipt "$tx" gasUsed --rpc-url "$EVM_RPC"
}
# What the relayer last said about a message's gas payment. Its log names
# the message only at the debug level; otherwise, how often it refused one.
GAS_REFUSAL='Repreparing message: (Gas payment requirement not met|GasPaymentNotFound)'
relayer_on() {
    local said
    said="$(grep -a "${1#0x}" "$RELAYER_LOG" | grep -aoE "$GAS_REFUSAL" | tail -1 || true)"
    [ -n "$said" ] || said="$(grep -acE "$GAS_REFUSAL" "$RELAYER_LOG" || true) refusals for gas payment in its log"
    echo "$said"
}
# A message the Dusk Mailbox dispatched last, by its nonce.
dusk_latest_id() { dq --contract "$DUSK_MAILBOX" --method latest_dispatched_id --return-type bytes32; }
gql() { curl -sf -m 30 -X POST "${DUSK_RUES_URL%/}/on/graphql/query" --data "$1"; }
dusk_height() { gql 'query { block(height: -1) { header { height } } }' | jq -er '.block.header.height'; }

# ── 1. A warp transfer from Dusk ─────────────────────────────────────────────

minted="$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$EVM_USER" --rpc-url "$EVM_RPC" | num)"
igp_before="$(dq --contract "$DUSK_IGP" --method claimable_fees --return-type u64)"
out="$(dtx transfer-remote --native --warp-contract "$DUSK_ROUTE" --destination "$EVM_DOMAIN" \
    --recipient "$EVM_USER" --amount 1000000000)" || fail "Dusk transfer failed: $out"
transfer_id="$(dusk_latest_id)"
paid=$(( $(dq --contract "$DUSK_IGP" --method claimable_fees --return-type u64) - igp_before ))
info "Dusk -> $EVM_CHAIN_NAME transfer 0x$transfer_id: dispatch fee $(jq -r .dispatch_fee <<<"$out") LUX, $paid of it to the Dusk IGP"
wait_until "$DELIVERY_TIMEOUT" "the transfer from Dusk was delivered" evm_delivered "$transfer_id"
[ "$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$EVM_USER" --rpc-url "$EVM_RPC" | num)" = "$((minted + 1000000000))" ] \
    || fail "the transfer from Dusk minted the wrong amount"
info "delivering it on $EVM_CHAIN_NAME used $(evm_delivery_gas "$transfer_id") gas; it paid for $PAID_GAS"

# ── 2. Messages from Dusk that pay for as much gas as a transfer ───────────

recipient="$(cd "$SOLIDITY_DIR" && forge create contracts/test/TestRecipient.sol:TestRecipient --broadcast --json \
    --rpc-url "$EVM_RPC" --private-key "$EVM_USER_KEY" 2>/dev/null | jq -er '.deployedTo')" \
    || fail "Can't deploy a TestRecipient on $EVM_CHAIN_NAME"
# The Dusk TestRecipient dispatches with its own Mailbox credit: 6.001 DUSK
# a message at the default prices.
dtx fund-dispatch --mailbox "$DUSK_MAILBOX" --payer "$DUSK_RECIPIENT" --amount 20000000000 >/dev/null \
    || fail "Can't fund the Dusk TestRecipient's dispatch credit"
dtx dispatch --mailbox "$DUSK_MAILBOX" --test-recipient "$DUSK_RECIPIENT" --destination "$EVM_DOMAIN" \
    --recipient "$(pad32 "$recipient")" --body 0x736d616c6c >/dev/null || fail "Dusk dispatch failed"
small_id="$(dusk_latest_id)"
heavy_body="0x$(printf 'ab%.0s' $(seq 2048))"
dtx dispatch --mailbox "$DUSK_MAILBOX" --test-recipient "$DUSK_RECIPIENT" --destination "$EVM_DOMAIN" \
    --recipient "$(pad32 "$recipient")" --body "$heavy_body" >/dev/null || fail "Dusk dispatch failed"
heavy_id="$(dusk_latest_id)"
heavy_nonce=$(( $(dq --contract "$DUSK_MAILBOX" --method nonce --return-type u32) - 1 ))
info "Dusk -> TestRecipient $recipient: 5-byte message 0x$small_id, 2 KiB message 0x$heavy_id (nonce $heavy_nonce)"
wait_until "$DELIVERY_TIMEOUT" "the 5-byte message from Dusk was delivered" evm_delivered "$small_id"
small_gas="$(evm_delivery_gas "$small_id")"
info "delivering it used $small_gas gas"

# ── 3. A warp transfer from Anvil ────────────────────────────────────────────

quote="$(cast call "$EVM_TOKEN" "quoteGasPayment(uint32)(uint256)" "$DUSK_DOMAIN" --rpc-url "$EVM_RPC" | num)"
account="$(daccount --offline | jq -r .account_h256)"
locked="$(dq --contract 0100000000000000000000000000000000000000000000000000000000000000 \
    --method contract_balance --return-type u64 --arg-bytes32 "$DUSK_ROUTE")"
igp_balance="$(cast balance "$EVM_IGP" --rpc-url "$EVM_RPC")"
tx="$(cast send "$EVM_TOKEN" "transferRemote(uint32,bytes32,uint256)" "$DUSK_DOMAIN" "0x$account" 500000000 \
    --value "$quote" --rpc-url "$EVM_RPC" --private-key "$EVM_USER_KEY" --json | jq -er '.transactionHash')" \
    || fail "the transfer from $EVM_CHAIN_NAME failed"
evm_transfer_id="$(cast receipt "$tx" --json --rpc-url "$EVM_RPC" \
    | jq -r --arg s "$(cast sig-event 'DispatchId(bytes32)')" '.logs[] | select(.topics[0] == $s) | .topics[1]')"
info "$EVM_CHAIN_NAME -> Dusk transfer $evm_transfer_id paid $quote wei, $(( $(cast balance "$EVM_IGP" --rpc-url "$EVM_RPC") - igp_balance )) of it to the EVM IGP"
wait_until "$DELIVERY_TIMEOUT" "the transfer from $EVM_CHAIN_NAME was delivered" dusk_delivered "$evm_transfer_id"
# A route pays an account it hasn't paid before into escrow. The deployer's
# claim pays it out and registers the deployer, so a later transfer to it
# pays out directly.
pending="$(dq --contract "$DUSK_ROUTE" --method pending_balance --return-type u64 --arg-bytes32 "$account")"
if [ "$pending" != 0 ]; then
    [ "$pending" = 500000000 ] || fail "the transfer from $EVM_CHAIN_NAME escrowed $pending LUX"
    dtx claim-pending --warp-contract "$DUSK_ROUTE" >/dev/null || fail "Can't claim the deployer's escrow on $DUSK_ROUTE"
    ok "claimed the transfer from escrow, which registered the deployer on the native route"
fi
[ "$(dq --contract 0100000000000000000000000000000000000000000000000000000000000000 --method contract_balance \
    --return-type u64 --arg-bytes32 "$DUSK_ROUTE")" = "$((locked - 500000000))" ] \
    || fail "the transfer from $EVM_CHAIN_NAME released the wrong amount"

# ── 4. A message from Anvil that pays our IGP nothing, then too little ───────

body=0x756e70616964
fee="$(cast call "$EVM_MAILBOX" "quoteDispatch(uint32,bytes32,bytes)(uint256)" "$DUSK_DOMAIN" "0x$DUSK_RECIPIENT" "$body" \
    --rpc-url "$EVM_RPC" | num)"
tx="$(cast send "$EVM_MAILBOX" "dispatch(uint32,bytes32,bytes)" "$DUSK_DOMAIN" "0x$DUSK_RECIPIENT" "$body" \
    --value "$fee" --rpc-url "$EVM_RPC" --private-key "$EVM_USER_KEY" --json | jq -er '.transactionHash')"
unpaid_id="$(cast receipt "$tx" --json --rpc-url "$EVM_RPC" \
    | jq -r --arg s "$(cast sig-event 'DispatchId(bytes32)')" '.logs[] | select(.topics[0] == $s) | .topics[1]')"
info "$EVM_CHAIN_NAME -> Dusk message $unpaid_id through the default hook, which paid $fee wei and nothing to our IGP"
never_within "$UNPAID_WATCH" "the relayer delivered a message nobody paid for" dusk_delivered "$unpaid_id"
ok "not delivered after ${UNPAID_WATCH}s; the relayer: $(relayer_on "$unpaid_id")"

pay_for() {
    local gas="$1" amount
    amount="$(cast call "$EVM_IGP" "quoteGasPayment(uint32,uint256)(uint256)" "$DUSK_DOMAIN" "$gas" --rpc-url "$EVM_RPC" | num)"
    cast send "$EVM_IGP" "payForGas(bytes32,uint32,uint256,address)" "$unpaid_id" "$DUSK_DOMAIN" "$gas" "$EVM_USER" \
        --value "$amount" --rpc-url "$EVM_RPC" --private-key "$EVM_USER_KEY" >/dev/null
    info "paid the EVM IGP $amount wei for $gas Dusk gas for it"
}
required="$(jq -r '.gasPaymentEnforcement[1].gasFraction' "$DUSK_DIR/deploy/out/$DEPLOY_NAME/relayer.json")"
required=$(( DUSK_RELAYER_GAS_LIMIT * ${required%/*} / ${required#*/} ))
half=$((required / 2))
pay_for "$half"
never_within "$UNPAID_WATCH" "the relayer delivered a message that paid for $half of the $required gas it requires" \
    dusk_delivered "$unpaid_id"
ok "not delivered after ${UNPAID_WATCH}s with $half gas paid for; the relayer: $(relayer_on "$unpaid_id")"
pay_for "$((required - half))"
wait_until "$DELIVERY_TIMEOUT" "the message was delivered once it had paid for $required gas" dusk_delivered "$unpaid_id"

# The 2 KiB message from Dusk is still waiting.
evm_delivered "$heavy_id" && fail "the relayer delivered the 2 KiB message from Dusk, which paid for $PAID_GAS gas"
ok "the 2 KiB message from Dusk is still undelivered; the relayer: $(relayer_on "$heavy_id")"

# ── 5. Top up the 2 KiB message from Dusk ────────────────────────────────────

# What delivering it takes, at most: the 5-byte message's delivery, plus
# what the recipient's handle takes for the 2 KiB body (which counts the
# transaction's base cost a second time). The relayer relays a message from
# Dusk once the gas it paid for covers its gas fraction of its estimate.
handle_gas="$(cast estimate "$recipient" "handle(uint32,bytes32,bytes)" "$DUSK_DOMAIN" "0x$DUSK_RECIPIENT" \
    "$heavy_body" --from "$EVM_MAILBOX" --rpc-url "$EVM_RPC" | num)" || fail "Can't estimate the 2 KiB handle"
fraction="$(jq -r '.gasPaymentEnforcement[0].gasFraction' "$DUSK_DIR/deploy/out/$DEPLOY_NAME/relayer.json")"
needed=$(( ((small_gas + handle_gas) * ${fraction%/*} + ${fraction#*/} - 1) / ${fraction#*/} ))
missing=$(( needed - PAID_GAS ))
[ "$missing" -gt 1 ] || fail "the 2 KiB message isn't delivered, yet it paid for the $needed gas estimated for it"
info "the 2 KiB message needs about $((small_gas + handle_gas)) gas, so $needed paid for at $fraction; it paid for $PAID_GAS"

igp_count() { dq --contract "$DUSK_IGP" --method gas_payment_count --return-type u32; }
records="$(igp_count)"
first=$(( missing / 2 ))
out="$(dtx pay-for-gas --igp "$DUSK_IGP" --message-id "$heavy_id" --destination "$EVM_DOMAIN" --gas-amount "$first")" \
    || fail "the first top-up failed: $out"
info "paid the Dusk IGP $(jq -r .paid <<<"$out") LUX for $first more gas for it"
never_within "$UNPAID_WATCH" "the relayer delivered the 2 KiB message from Dusk with $((PAID_GAS + first)) of the $needed gas it needs paid for" \
    evm_delivered "$heavy_id"
ok "not delivered after ${UNPAID_WATCH}s with $((PAID_GAS + first)) gas paid for; the relayer: $(relayer_on "$heavy_id")"

# The rest, with a tenth more for the estimate, for the message dusk-tx
# finds at its nonce, and a deposit twice the quote: the IGP keeps its price
# and sends the rest back.
rest=$(( missing - first + missing / 10 ))
quote="$("$DUSK_TX" pay-for-gas --rues-url "$DUSK_RUES_URL" --igp "$DUSK_IGP" --nonce "$heavy_nonce" \
    --gas-amount "$rest" --quote-only | jq -er '.quote')" || fail "Can't quote the second top-up"
out="$(dtx pay-for-gas --igp "$DUSK_IGP" --nonce "$heavy_nonce" --gas-amount "$rest" --max-payment $((2 * quote)))" \
    || fail "the second top-up failed: $out"
[ "$(jq -r .message_id <<<"$out")" = "$heavy_id" ] && [ "$(jq -r .destination <<<"$out")" = "$EVM_DOMAIN" ] \
    || fail "pay-for-gas --nonce $heavy_nonce paid for $(jq -c '{message_id, destination}' <<<"$out"), not the 2 KiB message"
[ "$(jq -r .paid <<<"$out")" = "$quote" ] && [ "$(jq -r .refunded <<<"$out")" = "$quote" ] \
    || fail "the second top-up paid $(jq -r .paid <<<"$out") of its $((2 * quote)) LUX deposit, not the $quote quoted"
[ "$(igp_count)" = $((records + 2)) ] || fail "the Dusk IGP recorded $(( $(igp_count) - records )) payments, not 2"
info "paid $quote LUX for $rest more gas, and got the other $quote back"
wait_until "$DELIVERY_TIMEOUT" "the 2 KiB message from Dusk was delivered once it had paid for $((PAID_GAS + first + rest)) gas" \
    evm_delivered "$heavy_id"
info "delivering it used $(evm_delivery_gas "$heavy_id") gas"

# ── 6. A burst of transfers from Anvil ───────────────────────────────────────

# The relayer delivers to Dusk as its config says: one at a time, as
# agent-configs writes it, each delivery in a block of its own; or, with
# pipelineDeliveries and a maxBatchSize above 1 (experimental), several as
# consecutive transactions of its account that share a block.
pipelined="$(jq -r '(.chains.dusk.pipelineDeliveries == true) and ((.chains.dusk.maxBatchSize // 1) > 1)' \
    "$RELAYER_CONFIG")" || fail "Can't read the relayer's config $RELAYER_CONFIG"
from_height="$(dusk_height)"
quote="$(cast call "$EVM_TOKEN" "quoteGasPayment(uint32)(uint256)" "$DUSK_DOMAIN" --rpc-url "$EVM_RPC" | num)"
nonce="$(cast nonce "$EVM_USER" --rpc-url "$EVM_RPC")"
burst_txs=()
for i in $(seq 0 $((BURST - 1))); do
    # A fixed gas limit: each estimate would see the state before the burst.
    burst_txs+=("$(cast send "$EVM_TOKEN" "transferRemote(uint32,bytes32,uint256)" "$DUSK_DOMAIN" "0x$account" \
        1000000 --value "$quote" --nonce $((nonce + i)) --gas-limit 400000 --async \
        --rpc-url "$EVM_RPC" --private-key "$EVM_USER_KEY")") || fail "burst transfer $i failed"
done
burst_ids=()
for tx in "${burst_txs[@]}"; do
    burst_ids+=("$(cast receipt "$tx" --json --rpc-url "$EVM_RPC" \
        | jq -er --arg s "$(cast sig-event 'DispatchId(bytes32)')" '.logs[] | select(.topics[0] == $s) | .topics[1]')") \
        || fail "burst transfer $tx dispatched nothing"
done
info "$BURST transfers from $EVM_CHAIN_NAME sent at once"
burst_delivered() { local id; for id in "${burst_ids[@]}"; do dusk_delivered "$id" || return 1; done; }
wait_until "$DELIVERY_TIMEOUT" "the $BURST transfers from $EVM_CHAIN_NAME were delivered" burst_delivered
# The Mailbox's process transactions since the burst: those executed, by
# block, and those that failed, which none may.
txs="$(gql "query { blockTxs(range: [$from_height, $(dusk_height)], contract: \"$DUSK_MAILBOX\") { blockHeight err tx { callData { fnName } } } }" \
    | jq -c '[.blockTxs[] | select(.tx.callData.fnName == "process")]')" \
    || fail "Can't read the Dusk Mailbox's transactions since block $from_height"
per_block="$(jq -c '[.[] | select(.err == null) | .blockHeight] | group_by(.) | map({height: .[0], deliveries: length})' <<<"$txs")"
info "deliveries to Dusk by block: $per_block"
[ "$(jq '[.[] | select(.err != null)] | length' <<<"$txs")" = 0 ] \
    || fail "deliveries to Dusk failed since the burst: $(jq -c '[.[] | select(.err != null) | {blockHeight, err}]' <<<"$txs")"
[ "$(jq '[.[].deliveries] | add // 0' <<<"$per_block")" -ge "$BURST" ] \
    || fail "the Dusk Mailbox executed fewer than $BURST deliveries since the burst"
most="$(jq '[.[].deliveries] | max // 0' <<<"$per_block")"
if [ "$pipelined" = true ]; then
    [ "$most" -ge 2 ] \
        || fail "no two of the $BURST deliveries shared a Dusk block, though the relayer's config pipelines them"
else
    [ "$most" -le 1 ] \
        || fail "the relayer sent $most deliveries to Dusk in one block, though its config delivers one at a time"
fi
ok "the relayer delivered the $BURST transfers in $(jq length <<<"$per_block") Dusk blocks, up to $most in one ($([ "$pipelined" = true ] && echo "pipelined" || echo "one at a time")), and none failed"

# ── 7. Collect the fees ──────────────────────────────────────────────────────

beneficiary="$(cast call "$EVM_IGP" "beneficiary()(address)" --rpc-url "$EVM_RPC")"
evm_before="$(cast balance "$beneficiary" --rpc-url "$EVM_RPC")"
evm_held="$(cast balance "$EVM_IGP" --rpc-url "$EVM_RPC")"
dusk_held=$(( $(dq --contract "$DUSK_IGP" --method claimable_fees --return-type u64)
    + $(dq --contract "$DUSK_PF" --method claimable_fees --return-type u64) ))
TESTNET_ENV="$ENV_FILE" bash "$DUSK_DIR/deploy/testnet.sh" claim
[ "$(cast balance "$EVM_IGP" --rpc-url "$EVM_RPC")" = 0 ] || fail "the EVM IGP still holds fees"
[ "$(dq --contract "$DUSK_IGP" --method claimable_fees --return-type u64)" = 0 ] \
    && [ "$(dq --contract "$DUSK_PF" --method claimable_fees --return-type u64)" = 0 ] \
    || fail "the Dusk fee contracts still hold fees"
gained=$(( $(cast balance "$beneficiary" --rpc-url "$EVM_RPC") - evm_before ))
[ "$beneficiary" != "$EVM_USER" ] && [ "$gained" != "$evm_held" ] \
    && fail "the EVM IGP's beneficiary gained $gained wei of the $evm_held it held"
ok "claimed $dusk_held LUX on Dusk for the deployer, and $evm_held wei on $EVM_CHAIN_NAME for $beneficiary"
ok "Relaying fees check passed"
