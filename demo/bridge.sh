#!/usr/bin/env bash
# =============================================================================
# Hyperlane Dusk <-> EVM Token Bridge
# =============================================================================
#
# Simple CLI for bridging tokens between Dusk and EVM (Anvil).
#
# Usage:
#   bash bridge.sh status           Show balances on both chains
#   bash bridge.sh to-dusk <amount> Bridge tokens EVM -> Dusk (whole tokens)
#   bash bridge.sh to-evm <amount>  Bridge tokens Dusk -> EVM (whole tokens)
#
# Requires: start-env.sh running + deploy.sh completed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"

# ── Helpers ──────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

info()   { echo -e "${BLUE}[INFO]${NC}  $*"; }
ok()     { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()   { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()   { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }
step()   { echo -e "${CYAN}  -> $*${NC}"; }

# Convert whole tokens to wei (18 decimals)
to_wei() {
    local amount="$1"
    # Validate: must be a positive integer
    if ! [[ "$amount" =~ ^[1-9][0-9]*$ ]]; then
        fail "Invalid amount: '$amount'. Use whole numbers (e.g., 5 for 5 $TOKEN_SYMBOL)"
    fi
    echo "${amount}000000000000000000"
}

# Format wei as human-readable tokens
from_wei() {
    local wei="$1"
    # Strip cast's annotation (e.g., "7000000000000000000 [7e18]" → "7000000000000000000")
    wei="${wei%% \[*}"
    wei="${wei%% *}"
    # Strip leading zeros
    wei="${wei#"${wei%%[!0]*}"}"
    [ -z "$wei" ] && wei="0"

    if command -v bc &>/dev/null; then
        echo "$(echo "scale=4; $wei / 1000000000000000000" | bc) $TOKEN_SYMBOL"
    else
        # Fallback: integer division
        local tokens=$((wei / 1000000000000000000))
        echo "$tokens $TOKEN_SYMBOL"
    fi
}

# Pad 20-byte EVM address to 32-byte H256
pad_evm_address() {
    local addr="${1#0x}"
    addr="$(echo "$addr" | tr '[:upper:]' '[:lower:]')"
    echo "000000000000000000000000${addr}"
}

# Construct TokenMessage body: recipient(32) || amount(32 as uint256 BE)
encode_token_message() {
    local recipient_hex="$1"  # 64 hex chars
    local amount_dec="$2"     # decimal
    local amount_hex
    amount_hex=$(printf '%064x' "$amount_dec")
    echo "${recipient_hex}${amount_hex}"
}

# ── Load Deployment State ────────────────────────────────────────────────────

if [ ! -f "$BRIDGE_STATE_FILE" ]; then
    fail "No deployment found at $BRIDGE_STATE_FILE

  Deploy first:
    bash demo/deploy.sh"
fi

# Parse state
EVM_MAILBOX=$(jq -r '.evm.mailbox' "$BRIDGE_STATE_FILE")
EVM_TOKEN=$(jq -r '.evm.token' "$BRIDGE_STATE_FILE")
DUSK_MAILBOX=$(jq -r '.dusk.mailbox' "$BRIDGE_STATE_FILE")
DUSK_WARP=$(jq -r '.dusk.warp_drc20' "$BRIDGE_STATE_FILE")
DUSK_ACCOUNT_H256=$(jq -r '.account_h256' "$BRIDGE_STATE_FILE")

# ── Command: status ──────────────────────────────────────────────────────────

cmd_status() {
    echo ""
    echo -e "${BOLD}Hyperlane Bridge Status${NC}"
    echo -e "${DIM}$(printf '%.0s─' {1..50})${NC}"
    echo ""

    # EVM
    echo -e "${BOLD}${GREEN}EVM (Anvil, domain=$EVM_DOMAIN)${NC}"

    local evm_balance evm_supply evm_nonce
    evm_balance=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" 2>/dev/null) || evm_balance="?"
    evm_supply=$(cast call "$EVM_TOKEN" "totalSupply()(uint256)" \
        --rpc-url "$ANVIL_RPC" 2>/dev/null) || evm_supply="?"
    evm_nonce=$(cast call "$EVM_MAILBOX" "nonce()(uint32)" \
        --rpc-url "$ANVIL_RPC" 2>/dev/null) || evm_nonce="?"

    echo "  Mailbox:        $EVM_MAILBOX"
    echo "  HypERC20:       $EVM_TOKEN ($TOKEN_SYMBOL)"
    echo "  Deployer:       $ANVIL_DEPLOYER"
    echo -e "  Balance:        ${BOLD}$(from_wei "$evm_balance")${NC}"
    echo "  Total Supply:   $(from_wei "$evm_supply")"
    echo "  Mailbox Nonce:  $evm_nonce"
    echo ""

    # Dusk
    echo -e "${BOLD}${GREEN}Dusk (domain=$DUSK_DOMAIN)${NC}"

    local dusk_supply_json dusk_supply dusk_nonce_json dusk_nonce
    dusk_supply_json=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$DUSK_WARP" \
        --method total_supply \
        --return-type u64 2>/dev/null) || dusk_supply_json='{"value":"?"}'
    dusk_supply=$(echo "$dusk_supply_json" | jq -r '.value // "?"')

    dusk_nonce_json=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$DUSK_MAILBOX" \
        --method nonce \
        --return-type u32 2>/dev/null) || dusk_nonce_json='{"value":"?"}'
    dusk_nonce=$(echo "$dusk_nonce_json" | jq -r '.value // "?"')

    echo "  Mailbox:        $DUSK_MAILBOX"
    echo "  WarpDrc20:      $DUSK_WARP ($TOKEN_SYMBOL)"
    echo "  Account:        ${DUSK_ACCOUNT_H256:0:16}..."
    echo -e "  WarpDrc20 Supply: ${BOLD}$(from_wei "$dusk_supply")${NC}"
    echo "  Mailbox Nonce:  $dusk_nonce"
    echo ""

    # Explorer links
    echo -e "${DIM}Explorers:${NC}"
    echo "  Dusk: $DUSK_EXPLORER_URL"
    echo "  EVM:  $EVM_EXPLORER_URL"
    echo ""
}

# ── Command: to-dusk ─────────────────────────────────────────────────────────

cmd_to_dusk() {
    local amount="$1"
    local amount_wei
    amount_wei=$(to_wei "$amount")

    echo ""
    echo -e "${BOLD}Bridge EVM -> Dusk: $amount $TOKEN_SYMBOL${NC}"
    echo -e "${DIM}$(printf '%.0s─' {1..50})${NC}"
    echo ""

    # Step 1: Burn tokens on EVM via transferRemote
    step "Getting EVM Mailbox nonce..."
    local evm_nonce_before
    evm_nonce_before=$(cast call "$EVM_MAILBOX" "nonce()(uint32)" --rpc-url "$ANVIL_RPC" 2>/dev/null) \
        || fail "Cannot query EVM nonce"

    step "Calling HypERC20.transferRemote(domain=$DUSK_DOMAIN, amount=$amount_wei)..."
    local tx_result
    tx_result=$(cast send "$EVM_TOKEN" \
        "transferRemote(uint32,bytes32,uint256)" \
        "$DUSK_DOMAIN" "0x${DUSK_ACCOUNT_H256}" "$amount_wei" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null) || fail "EVM transferRemote failed"

    local tx_hash
    tx_hash=$(echo "$tx_result" | jq -r '.transactionHash // empty')
    ok "Burned $amount $TOKEN_SYMBOL on EVM"
    if [ -n "$tx_hash" ]; then
        echo -e "  ${DIM}TX: $EVM_EXPLORER_URL/tx/$tx_hash${NC}"
    fi

    # Step 2: Construct relay message
    step "Encoding relay message..."
    local token_msg_body evm_sender_pad32 encode_result relay_msg relay_msg_id
    token_msg_body=$(encode_token_message "$DUSK_ACCOUNT_H256" "$amount_wei")
    evm_sender_pad32=$(pad_evm_address "$EVM_TOKEN")

    encode_result=$("$DUSK_TX" encode-message \
        --version 3 \
        --nonce "${evm_nonce_before}" \
        --origin "$EVM_DOMAIN" \
        --sender "$evm_sender_pad32" \
        --destination "$DUSK_DOMAIN" \
        --recipient "$DUSK_WARP" \
        --body "$token_msg_body" 2>/dev/null) || fail "Failed to encode message"

    relay_msg=$(echo "$encode_result" | jq -r '.encoded')
    relay_msg_id=$(echo "$encode_result" | jq -r '.message_id')
    ok "Message encoded (ID: ${relay_msg_id:0:16}...)"

    # Step 3: Deliver to Dusk
    step "Processing message on Dusk Mailbox..."
    "$DUSK_TX" process \
        --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" \
        --password "$CONSENSUS_PASSWORD" \
        --mailbox "$DUSK_MAILBOX" \
        --message "$relay_msg" \
        2>&1 >/dev/null || fail "Dusk process failed"
    ok "Message processed on Dusk!"

    # Wait for Dusk block (~10s block time)
    step "Waiting for Dusk block confirmation..."
    sleep 15

    # Step 4: Verify
    echo ""
    echo -e "${BOLD}Result:${NC}"

    local evm_balance_after dusk_supply_after_json dusk_supply_after
    evm_balance_after=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" 2>/dev/null) || evm_balance_after="?"
    dusk_supply_after_json=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" --contract "$DUSK_WARP" \
        --method total_supply --return-type u64 2>/dev/null) || dusk_supply_after_json='{"value":"?"}'
    dusk_supply_after=$(echo "$dusk_supply_after_json" | jq -r '.value // "?"')

    echo -e "  EVM balance:     $(from_wei "$evm_balance_after")"
    echo -e "  Dusk supply:     $(from_wei "$dusk_supply_after")"
    echo ""
    ok "Bridged $amount $TOKEN_SYMBOL from EVM to Dusk"
    echo ""
}

# ── Command: to-evm ──────────────────────────────────────────────────────────

cmd_to_evm() {
    local amount="$1"
    local amount_wei
    amount_wei=$(to_wei "$amount")

    echo ""
    echo -e "${BOLD}Bridge Dusk -> EVM: $amount $TOKEN_SYMBOL${NC}"
    echo -e "${DIM}$(printf '%.0s─' {1..50})${NC}"
    echo ""

    # Step 1: Get Dusk nonce before dispatch
    step "Getting Dusk Mailbox nonce..."
    local dusk_nonce_before_json dusk_nonce_before
    dusk_nonce_before_json=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$DUSK_MAILBOX" \
        --method nonce \
        --return-type u32 2>/dev/null) || fail "Cannot query Dusk nonce"
    dusk_nonce_before=$(echo "$dusk_nonce_before_json" | jq -r '.value // 0')

    # Step 2: Burn tokens on Dusk via transfer_remote
    local evm_recipient_pad32
    evm_recipient_pad32=$(pad_evm_address "$ANVIL_DEPLOYER")

    step "Calling WarpDrc20.transfer_remote(domain=$EVM_DOMAIN, amount=$amount_wei)..."
    "$DUSK_TX" transfer-remote \
        --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" \
        --password "$CONSENSUS_PASSWORD" \
        --warp-contract "$DUSK_WARP" \
        --destination "$EVM_DOMAIN" \
        --recipient "$evm_recipient_pad32" \
        --amount "$amount_wei" \
        2>&1 > /dev/null || fail "Dusk transfer_remote failed"
    ok "Burned $amount $TOKEN_SYMBOL on Dusk"

    # Wait for Dusk block (~10s block time)
    step "Waiting for Dusk block confirmation..."
    sleep 15

    # Step 3: Read dispatched message
    step "Reading dispatched message from Dusk Mailbox..."
    local dispatched_msg_json dispatched_msg dispatched_len
    dispatched_msg_json=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$DUSK_MAILBOX" \
        --method dispatched_message \
        --arg-u32 "$dusk_nonce_before" \
        --return-type bytes 2>&1) || fail "Failed to read dispatched message"

    dispatched_msg=$(echo "$dispatched_msg_json" | jq -r '.value')
    dispatched_len=$(echo "$dispatched_msg_json" | jq -r '.length')
    ok "Dispatched message read (${dispatched_len} bytes)"

    # Step 4: Deliver to EVM
    step "Processing message on EVM Mailbox..."
    local evm_tx_result evm_tx_hash
    evm_tx_result=$(cast send "$EVM_MAILBOX" \
        "process(bytes,bytes)" \
        "0x" "0x${dispatched_msg}" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null) || fail "EVM process failed"

    evm_tx_hash=$(echo "$evm_tx_result" | jq -r '.transactionHash // empty')
    ok "Message processed on EVM!"
    if [ -n "$evm_tx_hash" ]; then
        echo -e "  ${DIM}TX: $EVM_EXPLORER_URL/tx/$evm_tx_hash${NC}"
    fi

    # Step 5: Verify
    echo ""
    echo -e "${BOLD}Result:${NC}"

    local evm_balance_after dusk_supply_after_json dusk_supply_after
    evm_balance_after=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" 2>/dev/null) || evm_balance_after="?"
    dusk_supply_after_json=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" --contract "$DUSK_WARP" \
        --method total_supply --return-type u64 2>/dev/null) || dusk_supply_after_json='{"value":"?"}'
    dusk_supply_after=$(echo "$dusk_supply_after_json" | jq -r '.value // "?"')

    echo -e "  EVM balance:     $(from_wei "$evm_balance_after")"
    echo -e "  Dusk supply:     $(from_wei "$dusk_supply_after")"
    echo ""
    ok "Bridged $amount $TOKEN_SYMBOL from Dusk to EVM"
    echo ""
}

# ── Dispatch ─────────────────────────────────────────────────────────────────

case "${1:-}" in
    status)
        cmd_status
        ;;
    to-dusk)
        [ -z "${2:-}" ] && fail "Usage: bridge.sh to-dusk <amount>

  Example: bash demo/bridge.sh to-dusk 3"
        cmd_to_dusk "$2"
        ;;
    to-evm)
        [ -z "${2:-}" ] && fail "Usage: bridge.sh to-evm <amount>

  Example: bash demo/bridge.sh to-evm 1"
        cmd_to_evm "$2"
        ;;
    -h|--help|help)
        echo ""
        echo -e "${BOLD}Hyperlane Dusk <-> EVM Token Bridge${NC}"
        echo ""
        echo "Usage:"
        echo "  bridge.sh status            Show balances on both chains"
        echo "  bridge.sh to-dusk <amount>  Bridge tokens EVM -> Dusk"
        echo "  bridge.sh to-evm <amount>   Bridge tokens Dusk -> EVM"
        echo ""
        echo "Examples:"
        echo "  bash demo/bridge.sh status"
        echo "  bash demo/bridge.sh to-dusk 3    # Bridge 3 wDUSK to Dusk"
        echo "  bash demo/bridge.sh to-evm 1     # Bridge 1 wDUSK to EVM"
        echo ""
        echo "Amounts are in whole tokens (e.g., 3 = 3.0000 wDUSK = 3e18 wei)."
        echo ""
        ;;
    *)
        echo "Usage: bridge.sh {status|to-dusk|to-evm|help} [amount]"
        echo "Run 'bridge.sh help' for more info."
        exit 1
        ;;
esac
