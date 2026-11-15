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
#   5. `deploy/testnet.sh claim` pays the Dusk IGP's and ProtocolFee's fees
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
# How long a delivery may take, and how long an unpaid message is watched.
DELIVERY_TIMEOUT="${DELIVERY_TIMEOUT:-600}"
UNPAID_WATCH="${UNPAID_WATCH:-120}"
# Anvil account #0, the rehearsal's EVM deployer, sends the EVM transfers.
EVM_USER_KEY="$EVM_DEPLOYER_KEY"

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
dtx() { "$DUSK_TX" "$@" --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS"; }

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
dtx dispatch --mailbox "$DUSK_MAILBOX" --test-recipient "$DUSK_RECIPIENT" --destination "$EVM_DOMAIN" \
    --recipient "$(pad32 "$recipient")" --body "0x$(printf 'ab%.0s' $(seq 2048))" >/dev/null || fail "Dusk dispatch failed"
heavy_id="$(dusk_latest_id)"
info "Dusk -> TestRecipient $recipient: 5-byte message 0x$small_id, 2 KiB message 0x$heavy_id"
wait_until "$DELIVERY_TIMEOUT" "the 5-byte message from Dusk was delivered" evm_delivered "$small_id"
info "delivering it used $(evm_delivery_gas "$small_id") gas"

# ── 3. A warp transfer from Anvil ────────────────────────────────────────────

quote="$(cast call "$EVM_TOKEN" "quoteGasPayment(uint32)(uint256)" "$DUSK_DOMAIN" --rpc-url "$EVM_RPC" | num)"
account="$(dtx account --offline | jq -r .account_h256)"
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

# ── 5. Collect the fees ──────────────────────────────────────────────────────

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
