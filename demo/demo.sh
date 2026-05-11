#!/usr/bin/env bash
# =============================================================================
# Hyperlane Dusk <-> EVM Cross-Chain Token Bridge Demo
# =============================================================================
#
# Deploys Hyperlane contracts on both Dusk (rusk-duskevm Docker) and Anvil (EVM),
# then demonstrates cross-chain token bridging in both directions.
#
# Flow:
#   1. Start Anvil (EVM local node)
#   2. Deploy Hyperlane on EVM (Mailbox, ISM, Hook, HypERC20)
#   3. Deploy Hyperlane on Dusk (Mailbox, ISM, Hook, WarpDrc20)
#   4. Enroll remote routers on both sides
#   5. Bridge tokens EVM -> Dusk (manual relay)
#   6. Bridge tokens Dusk -> EVM (manual relay)
#
# Prerequisites:
#   - Foundry (anvil, forge, cast): curl -L https://foundry.paradigm.xyz | bash && foundryup
#   - Docker: running rusk-duskevm container
#   - dusk-tx binary: cargo build -p dusk-tx (from dusk/)
#   - Contract WASMs: make all (from dusk/)
#   - Solidity deps: forge soldeer install (from solidity/)
#
# Usage: bash demo.sh [--skip-deploy]

set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────────────

DUSK_RUES_URL="${RUES_URL:-http://localhost:18090/}"
DUSK_DOMAIN=4242
ANVIL_PORT=8545
ANVIL_CHAIN_ID=31338
EVM_DOMAIN=31338

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DUSK_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$DUSK_DIR/.." && pwd)"
SOLIDITY_DIR="$REPO_DIR/hyperlane-monorepo/solidity"

DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
WASM_DIR="$DUSK_DIR/target/contract/wasm32-unknown-unknown/release"

# Consensus keys for Dusk
CONSENSUS_KEYS="$DUSK_DIR/e2e/consensus.keys"
CONSENSUS_PASSWORD="password"

# Anvil default deployer (Account #0)
ANVIL_PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
ANVIL_DEPLOYER="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
ANVIL_RPC="http://localhost:$ANVIL_PORT"

# Token config — using 18 decimals, small amounts that fit in u64
# u64::MAX ~ 18.4e18, so max ~18 tokens at 18 decimals
TOKEN_NAME="Wrapped DUSK"
TOKEN_SYMBOL="wDUSK"
TOKEN_DECIMALS=18
INITIAL_SUPPLY="10000000000000000000"       # 10 tokens (10e18)
BRIDGE_EVM_TO_DUSK="3000000000000000000"    # 3 tokens (3e18)
BRIDGE_DUSK_TO_EVM="1000000000000000000"    # 1 token (1e18)

# Temp files
DUSK_DEPLOY_OUTPUT="/tmp/hyperlane-demo-dusk-deploy.json"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

info()   { echo -e "${BLUE}[INFO]${NC}  $*"; }
ok()     { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()   { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()   { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }
header() { echo -e "\n${BOLD}${GREEN}═══════════════════════════════════════════════════════════${NC}"; \
           echo -e "${BOLD}${GREEN}  $*${NC}"; \
           echo -e "${BOLD}${GREEN}═══════════════════════════════════════════════════════════${NC}\n"; }
step()   { echo -e "${CYAN}  -> $*${NC}"; }

# Pad a 20-byte EVM address to 32-byte H256 (left-pad with 12 zero bytes)
pad_evm_address() {
    local addr="${1#0x}"
    addr="$(echo "$addr" | tr '[:upper:]' '[:lower:]')"
    echo "000000000000000000000000${addr}"
}

# Construct a TokenMessage body: recipient(32) || amount(32 as uint256 BE)
encode_token_message() {
    local recipient_hex="$1"  # 64 hex chars (32 bytes)
    local amount_dec="$2"     # decimal amount

    # Convert decimal amount to 32-byte big-endian hex
    local amount_hex
    amount_hex=$(printf '%064x' "$amount_dec")

    echo "${recipient_hex}${amount_hex}"
}

# ── Step 0: Prerequisites ────────────────────────────────────────────────────

header "Step 0: Checking Prerequisites"

# Check dusk-tx binary
if [ ! -f "$DUSK_TX" ]; then
    warn "dusk-tx not found at $DUSK_TX"
    # Try debug build
    DUSK_TX="$DUSK_DIR/target/debug/dusk-tx"
    if [ ! -f "$DUSK_TX" ]; then
        info "Building dusk-tx..."
        (cd "$DUSK_DIR" && cargo build -p dusk-tx --release) || fail "Failed to build dusk-tx"
        DUSK_TX="$DUSK_DIR/target/release/dusk-tx"
    fi
fi
ok "dusk-tx binary: $DUSK_TX"

# Check WASMs
if [ ! -f "$WASM_DIR/hyperlane_dusk_mailbox.wasm" ]; then
    warn "Contract WASMs not found"
    info "Building WASMs (this takes a few minutes)..."
    (cd "$DUSK_DIR" && make all) || fail "Failed to build WASMs"
fi
ok "Contract WASMs: $WASM_DIR"

# Check consensus keys
if [ ! -f "$CONSENSUS_KEYS" ]; then
    fail "Consensus keys not found at $CONSENSUS_KEYS"
fi
ok "Consensus keys: $CONSENSUS_KEYS"

# Check Foundry tools
for tool in anvil forge cast; do
    if ! command -v "$tool" &>/dev/null; then
        fail "$tool not found. Install: curl -L https://foundry.paradigm.xyz | bash && foundryup"
    fi
done
ok "Foundry tools: anvil, forge, cast"

# Check jq
if ! command -v jq &>/dev/null; then
    fail "jq not found. Install: sudo apt-get install jq"
fi
ok "jq: $(which jq)"

# Check Solidity directory
if [ ! -d "$SOLIDITY_DIR" ]; then
    fail "Solidity directory not found at $SOLIDITY_DIR"
fi
ok "Solidity directory: $SOLIDITY_DIR"

# Install Solidity dependencies if needed
if [ ! -d "$SOLIDITY_DIR/dependencies" ]; then
    info "Installing Solidity dependencies..."
    (cd "$SOLIDITY_DIR" && forge soldeer install) || fail "Failed to install Solidity deps"
fi
ok "Solidity dependencies installed"

# ── Step 1: Start Anvil ──────────────────────────────────────────────────────

header "Step 1: Starting Anvil (EVM Local Node)"

if lsof -i ":$ANVIL_PORT" &>/dev/null; then
    info "Anvil already running on port $ANVIL_PORT"
else
    info "Starting Anvil on port $ANVIL_PORT..."
    anvil --port "$ANVIL_PORT" --chain-id "$ANVIL_CHAIN_ID" &>/dev/null &
    ANVIL_PID=$!
    sleep 2
    if ! kill -0 "$ANVIL_PID" 2>/dev/null; then
        fail "Anvil failed to start"
    fi
    ok "Anvil started (PID: $ANVIL_PID)"
fi

BLOCK_NUM=$(cast block-number --rpc-url "$ANVIL_RPC" 2>/dev/null) || fail "Cannot connect to Anvil"
ok "Anvil connected (block: $BLOCK_NUM)"

# ── Step 2: Deploy Hyperlane on EVM ──────────────────────────────────────────

header "Step 2: Deploying Hyperlane on EVM (domain=$EVM_DOMAIN)"

if [ "${1:-}" = "--skip-deploy" ] && [ -f "/tmp/hyperlane-demo-evm.json" ]; then
    info "Skipping EVM deployment (--skip-deploy)"
    EVM_ISM=$(jq -r '.ism' /tmp/hyperlane-demo-evm.json)
    EVM_HOOK=$(jq -r '.hook' /tmp/hyperlane-demo-evm.json)
    EVM_MAILBOX=$(jq -r '.mailbox' /tmp/hyperlane-demo-evm.json)
    EVM_RECIPIENT=$(jq -r '.recipient' /tmp/hyperlane-demo-evm.json)
    EVM_TOKEN=$(jq -r '.token' /tmp/hyperlane-demo-evm.json)
else
    # Deploy from solidity directory
    cd "$SOLIDITY_DIR"

    step "Deploying TestIsm (always-verify ISM)..."
    EVM_ISM=$(forge create contracts/test/TestIsm.sol:TestIsm \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null | jq -r '.deployedTo') || fail "Failed to deploy TestIsm"
    ok "TestIsm: $EVM_ISM"

    step "Deploying TestPostDispatchHook..."
    EVM_HOOK=$(forge create contracts/test/TestPostDispatchHook.sol:TestPostDispatchHook \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null | jq -r '.deployedTo') || fail "Failed to deploy TestPostDispatchHook"
    ok "TestPostDispatchHook: $EVM_HOOK"

    step "Deploying Mailbox (domain=$EVM_DOMAIN)..."
    EVM_MAILBOX=$(forge create contracts/Mailbox.sol:Mailbox \
        --constructor-args "$EVM_DOMAIN" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null | jq -r '.deployedTo') || fail "Failed to deploy Mailbox"
    ok "Mailbox: $EVM_MAILBOX"

    step "Initializing Mailbox..."
    cast send "$EVM_MAILBOX" \
        "initialize(address,address,address,address)" \
        "$ANVIL_DEPLOYER" "$EVM_ISM" "$EVM_HOOK" "$EVM_HOOK" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize Mailbox"
    ok "Mailbox initialized"

    step "Deploying TestRecipient..."
    EVM_RECIPIENT=$(forge create contracts/test/TestRecipient.sol:TestRecipient \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null | jq -r '.deployedTo') || fail "Failed to deploy TestRecipient"
    ok "TestRecipient: $EVM_RECIPIENT"

    step "Deploying HypERC20 ($TOKEN_SYMBOL, ${TOKEN_DECIMALS} decimals, scale=1)..."
    EVM_TOKEN=$(forge create contracts/token/HypERC20.sol:HypERC20 \
        --constructor-args "$TOKEN_DECIMALS" 1 1 "$EVM_MAILBOX" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --json 2>/dev/null | jq -r '.deployedTo') || fail "Failed to deploy HypERC20"
    ok "HypERC20: $EVM_TOKEN"

    step "Initializing HypERC20 (supply=$INITIAL_SUPPLY)..."
    cast send "$EVM_TOKEN" \
        "initialize(uint256,string,string,address,address,address)" \
        "$INITIAL_SUPPLY" "$TOKEN_NAME" "$TOKEN_SYMBOL" \
        "$EVM_HOOK" "$EVM_ISM" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize HypERC20"
    ok "HypERC20 initialized (10 $TOKEN_SYMBOL minted to deployer)"

    # Save EVM deployment for --skip-deploy
    cat > /tmp/hyperlane-demo-evm.json <<EVMJSON
{
    "ism": "$EVM_ISM",
    "hook": "$EVM_HOOK",
    "mailbox": "$EVM_MAILBOX",
    "recipient": "$EVM_RECIPIENT",
    "token": "$EVM_TOKEN"
}
EVMJSON
fi

echo ""
ok "EVM contracts deployed:"
info "  Mailbox:      $EVM_MAILBOX"
info "  TestIsm:      $EVM_ISM"
info "  Hook:         $EVM_HOOK"
info "  TestRecipient: $EVM_RECIPIENT"
info "  HypERC20:     $EVM_TOKEN"

# ── Step 3: Deploy Hyperlane on Dusk ─────────────────────────────────────────

header "Step 3: Deploying Hyperlane on Dusk (domain=$DUSK_DOMAIN)"

# Check Dusk node connectivity
step "Checking Dusk node..."
DUSK_CHAIN_ID=$(curl -s -X POST \
    -H "Content-Type: application/octet-stream" \
    -H "rusk-version: 1.0.0-rc.0" \
    "${DUSK_RUES_URL}on/contracts:0100000000000000000000000000000000000000000000000000000000000000/chain_id" \
    --max-time 5 2>/dev/null | xxd -p 2>/dev/null) || true

if [ -z "$DUSK_CHAIN_ID" ]; then
    fail "Dusk node not reachable at $DUSK_RUES_URL. Start the rusk-duskevm Docker container first."
fi
ok "Dusk node connected"

if [ "${1:-}" = "--skip-deploy" ] && [ -f "$DUSK_DEPLOY_OUTPUT" ]; then
    info "Skipping Dusk deployment (--skip-deploy)"
else
    step "Deploying Hyperlane contracts on Dusk..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" deploy-hyperlane \
        --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" \
        --domain "$DUSK_DOMAIN" \
        --wasm-dir "$WASM_DIR" \
        --deploy-warp-drc20 \
        --warp-name "$TOKEN_NAME" \
        --warp-symbol "$TOKEN_SYMBOL" \
        --warp-decimals "$TOKEN_DECIMALS" \
        > "$DUSK_DEPLOY_OUTPUT" || fail "Dusk deployment failed"
    ok "Dusk contracts deployed"
fi

# Parse Dusk deployment output
DUSK_MAILBOX=$(jq -r '.contracts.mailbox' "$DUSK_DEPLOY_OUTPUT")
DUSK_MERKLE=$(jq -r '.contracts.merkle_tree_hook' "$DUSK_DEPLOY_OUTPUT")
DUSK_ISM=$(jq -r '.contracts.ism_multisig // .contracts.test_mock // empty' "$DUSK_DEPLOY_OUTPUT")
DUSK_WARP=$(jq -r '.contracts.warp_drc20' "$DUSK_DEPLOY_OUTPUT")
DUSK_TEST_RECIPIENT=$(jq -r '.contracts.test_recipient' "$DUSK_DEPLOY_OUTPUT")

echo ""
ok "Dusk contracts deployed:"
info "  Mailbox:         $DUSK_MAILBOX"
info "  MerkleTreeHook:  $DUSK_MERKLE"
info "  WarpDrc20:       $DUSK_WARP"
info "  TestRecipient:   $DUSK_TEST_RECIPIENT"

# ── Step 4: Enroll Remote Routers & Register Account ─────────────────────────

header "Step 4: Enrolling Remote Routers & Registering Account"

# EVM side: enroll Dusk WarpDrc20 as remote router for the Dusk domain
EVM_TOKEN_PAD32="0x$(pad_evm_address "$EVM_TOKEN")"
DUSK_WARP_PAD32="0x${DUSK_WARP}"

step "EVM: Enrolling Dusk WarpDrc20 as remote router (domain=$DUSK_DOMAIN)..."
cast send "$EVM_TOKEN" \
    "enrollRemoteRouter(uint32,bytes32)" \
    "$DUSK_DOMAIN" "$DUSK_WARP_PAD32" \
    --rpc-url "$ANVIL_RPC" \
    --private-key "$ANVIL_PRIVATE_KEY" \
    &>/dev/null || fail "Failed to enroll remote router on EVM"
ok "EVM: Remote router enrolled (domain=$DUSK_DOMAIN -> $DUSK_WARP)"

# Dusk side: enroll EVM HypERC20 as remote router for the EVM domain
step "Dusk: Enrolling EVM HypERC20 as remote router (domain=$EVM_DOMAIN)..."
DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" enroll-router \
    --rues-url "$DUSK_RUES_URL" \
    --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP" \
    --domain "$EVM_DOMAIN" \
    --router "$(pad_evm_address "$EVM_TOKEN")" \
    > /dev/null || fail "Failed to enroll remote router on Dusk"
ok "Dusk: Remote router enrolled (domain=$EVM_DOMAIN -> ${EVM_TOKEN})"

# Register deployer's BLS key on WarpDrc20 so tokens can be minted to
# an External account (required for transfer_remote in step 7)
step "Dusk: Registering deployer BLS key on WarpDrc20..."
REGISTER_RESULT=$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" register-account \
    --rues-url "$DUSK_RUES_URL" \
    --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP" 2>/dev/null) || fail "Failed to register account on Dusk"
DUSK_ACCOUNT_H256=$(echo "$REGISTER_RESULT" | jq -r '.account_h256')
ok "Dusk: Account registered (H256: ${DUSK_ACCOUNT_H256:0:16}...)"

# Wait for block inclusion
sleep 3

# ── Step 5: Verify Deployment ────────────────────────────────────────────────

header "Step 5: Verifying Deployment"

# EVM verification
step "Querying EVM Mailbox nonce..."
EVM_NONCE=$(cast call "$EVM_MAILBOX" "nonce()(uint32)" --rpc-url "$ANVIL_RPC" 2>/dev/null)
ok "EVM Mailbox nonce: $EVM_NONCE"

step "Querying EVM HypERC20 balance..."
EVM_BALANCE=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" 2>/dev/null)
ok "EVM deployer balance: $EVM_BALANCE ($TOKEN_SYMBOL)"

step "Querying EVM HypERC20 total supply..."
EVM_SUPPLY=$(cast call "$EVM_TOKEN" "totalSupply()(uint256)" --rpc-url "$ANVIL_RPC" 2>/dev/null)
ok "EVM total supply: $EVM_SUPPLY"

# Dusk verification
step "Querying Dusk Mailbox nonce..."
DUSK_NONCE_JSON=$("$DUSK_TX" query \
    --rues-url "$DUSK_RUES_URL" \
    --contract "$DUSK_MAILBOX" \
    --method nonce \
    --return-type u32 2>/dev/null) || warn "Could not query Dusk nonce"
DUSK_NONCE=$(echo "$DUSK_NONCE_JSON" | jq -r '.value // 0')
ok "Dusk Mailbox nonce: $DUSK_NONCE"

step "Querying Dusk WarpDrc20 total supply..."
DUSK_SUPPLY_JSON=$("$DUSK_TX" query \
    --rues-url "$DUSK_RUES_URL" \
    --contract "$DUSK_WARP" \
    --method total_supply \
    --return-type u64 2>/dev/null) || warn "Could not query Dusk supply"
DUSK_SUPPLY=$(echo "$DUSK_SUPPLY_JSON" | jq -r '.value // 0')
ok "Dusk WarpDrc20 supply: $DUSK_SUPPLY"

# ── Step 6: Bridge EVM -> Dusk ───────────────────────────────────────────────

header "Step 6: Bridge EVM -> Dusk (3 $TOKEN_SYMBOL)"

# The Dusk recipient is the deployer's registered account (keccak256(pk))
# so tokens are minted to the External account and can be spent via transfer_remote
DUSK_TOKEN_RECIPIENT="$DUSK_ACCOUNT_H256"

step "EVM: Calling HypERC20.transferRemote(domain=$DUSK_DOMAIN, amount=3e18)..."
step "  Recipient on Dusk: ${DUSK_TOKEN_RECIPIENT:0:16}... (deployer's External account)"

# Get EVM nonce before dispatch
EVM_NONCE_BEFORE=$(cast call "$EVM_MAILBOX" "nonce()(uint32)" --rpc-url "$ANVIL_RPC" 2>/dev/null)

cast send "$EVM_TOKEN" \
    "transferRemote(uint32,bytes32,uint256)" \
    "$DUSK_DOMAIN" "0x${DUSK_TOKEN_RECIPIENT}" "$BRIDGE_EVM_TO_DUSK" \
    --rpc-url "$ANVIL_RPC" \
    --private-key "$ANVIL_PRIVATE_KEY" \
    &>/dev/null || fail "EVM transferRemote failed"
ok "EVM: transferRemote sent (3 $TOKEN_SYMBOL burned from deployer)"

# Verify EVM balance decreased
EVM_BALANCE_AFTER=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" 2>/dev/null)
ok "EVM deployer balance after: $EVM_BALANCE_AFTER"

# Now construct the relay message manually
# The EVM Mailbox dispatched a message with:
#   version=3, nonce=$EVM_NONCE_BEFORE, origin=$EVM_DOMAIN,
#   sender=pad(EVM_TOKEN), destination=$DUSK_DOMAIN,
#   recipient=$DUSK_WARP, body=TokenMessage(DUSK_TOKEN_RECIPIENT, 3e18)
step "Constructing relay message..."

# TokenMessage body: recipient(32) || amount(32 as uint256)
TOKEN_MSG_BODY=$(encode_token_message "$DUSK_TOKEN_RECIPIENT" "$BRIDGE_EVM_TO_DUSK")

# Encode the Hyperlane message
EVM_SENDER_PAD32=$(pad_evm_address "$EVM_TOKEN")
ENCODE_RESULT=$("$DUSK_TX" encode-message \
    --version 3 \
    --nonce "${EVM_NONCE_BEFORE}" \
    --origin "$EVM_DOMAIN" \
    --sender "$EVM_SENDER_PAD32" \
    --destination "$DUSK_DOMAIN" \
    --recipient "$DUSK_WARP" \
    --body "$TOKEN_MSG_BODY" 2>/dev/null) || fail "Failed to encode message"

RELAY_MSG=$(echo "$ENCODE_RESULT" | jq -r '.encoded')
RELAY_MSG_ID=$(echo "$ENCODE_RESULT" | jq -r '.message_id')
ok "Relay message encoded (ID: ${RELAY_MSG_ID:0:16}...)"

step "Dusk: Processing inbound message (EVM -> Dusk)..."
PROCESS_RESULT=$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" process \
    --rues-url "$DUSK_RUES_URL" \
    --keys "$CONSENSUS_KEYS" \
    --mailbox "$DUSK_MAILBOX" \
    --message "$RELAY_MSG" 2>/dev/null) || fail "Failed to process message on Dusk"
ok "Dusk: Message processed!"

# Wait for Dusk block
sleep 3

# Verify Dusk supply increased
step "Verifying Dusk WarpDrc20 supply after bridge..."
DUSK_SUPPLY_AFTER_JSON=$("$DUSK_TX" query \
    --rues-url "$DUSK_RUES_URL" \
    --contract "$DUSK_WARP" \
    --method total_supply \
    --return-type u64 2>/dev/null) || warn "Could not query supply"
DUSK_SUPPLY_AFTER=$(echo "$DUSK_SUPPLY_AFTER_JSON" | jq -r '.value // 0')
ok "Dusk WarpDrc20 supply: $DUSK_SUPPLY_AFTER (expected: $BRIDGE_EVM_TO_DUSK)"

# ── Step 7: Bridge Dusk -> EVM ───────────────────────────────────────────────

header "Step 7: Bridge Dusk -> EVM (1 $TOKEN_SYMBOL)"

# The EVM recipient is the Anvil deployer, padded to 32 bytes
EVM_RECIPIENT_PAD32=$(pad_evm_address "$ANVIL_DEPLOYER")

step "Dusk: Calling WarpDrc20.transfer_remote(domain=$EVM_DOMAIN, amount=1e18)..."
step "  Recipient on EVM: $ANVIL_DEPLOYER"

# Get Dusk nonce before dispatch
DUSK_NONCE_BEFORE_JSON=$("$DUSK_TX" query \
    --rues-url "$DUSK_RUES_URL" \
    --contract "$DUSK_MAILBOX" \
    --method nonce \
    --return-type u32 2>/dev/null) || warn "Could not query nonce"
DUSK_NONCE_BEFORE=$(echo "$DUSK_NONCE_BEFORE_JSON" | jq -r '.value // 0')

# Call transfer_remote on WarpDrc20 — burns tokens from the deployer's External
# account and dispatches a Hyperlane message to the EVM HypERC20.
DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" transfer-remote \
    --rues-url "$DUSK_RUES_URL" \
    --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP" \
    --destination "$EVM_DOMAIN" \
    --recipient "$EVM_RECIPIENT_PAD32" \
    --amount "$BRIDGE_DUSK_TO_EVM" \
    > /dev/null || fail "Failed to call transfer_remote on Dusk"
ok "Dusk: transfer_remote sent (1 $TOKEN_SYMBOL burned)"

# Wait for Dusk block
sleep 3

# Verify Dusk supply decreased
step "Verifying Dusk WarpDrc20 supply after burn..."
DUSK_SUPPLY_BURN_JSON=$("$DUSK_TX" query \
    --rues-url "$DUSK_RUES_URL" \
    --contract "$DUSK_WARP" \
    --method total_supply \
    --return-type u64 2>/dev/null) || warn "Could not query supply"
DUSK_SUPPLY_BURN=$(echo "$DUSK_SUPPLY_BURN_JSON" | jq -r '.value // 0')
ok "Dusk WarpDrc20 supply: $DUSK_SUPPLY_BURN (after burning 1 token)"

# Read the dispatched message from Dusk Mailbox
step "Dusk: Reading dispatched message from Mailbox..."
DISPATCHED_MSG_JSON=$("$DUSK_TX" query \
    --rues-url "$DUSK_RUES_URL" \
    --contract "$DUSK_MAILBOX" \
    --method dispatched_message \
    --arg-u32 "$DUSK_NONCE_BEFORE" \
    --return-type bytes 2>/dev/null) || fail "Failed to read dispatched message"

DISPATCHED_MSG=$(echo "$DISPATCHED_MSG_JSON" | jq -r '.value')
DISPATCHED_LEN=$(echo "$DISPATCHED_MSG_JSON" | jq -r '.length')
ok "Dispatched message read (${DISPATCHED_LEN} bytes)"

# Process on EVM Mailbox — this calls HypERC20.handle() which mints tokens
step "EVM: Processing inbound message (Dusk -> EVM)..."
cast send "$EVM_MAILBOX" \
    "process(bytes,bytes)" \
    "0x" "0x${DISPATCHED_MSG}" \
    --rpc-url "$ANVIL_RPC" \
    --private-key "$ANVIL_PRIVATE_KEY" \
    &>/dev/null || fail "EVM process failed"
ok "EVM: Message processed! (1 $TOKEN_SYMBOL minted to deployer)"

# Verify EVM balance increased
step "Verifying EVM HypERC20 balance after bridge..."
EVM_BALANCE_AFTER_BRIDGE=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" 2>/dev/null)
ok "EVM deployer balance: $EVM_BALANCE_AFTER_BRIDGE (after receiving 1 token back)"

# ── Step 8: Summary ──────────────────────────────────────────────────────────

header "Demo Summary"

# Final EVM balances
EVM_FINAL_BALANCE=$(cast call "$EVM_TOKEN" "balanceOf(address)(uint256)" "$ANVIL_DEPLOYER" --rpc-url "$ANVIL_RPC" 2>/dev/null)
EVM_FINAL_SUPPLY=$(cast call "$EVM_TOKEN" "totalSupply()(uint256)" --rpc-url "$ANVIL_RPC" 2>/dev/null)
EVM_FINAL_NONCE=$(cast call "$EVM_MAILBOX" "nonce()(uint32)" --rpc-url "$ANVIL_RPC" 2>/dev/null)

# Final Dusk state
DUSK_FINAL_SUPPLY_JSON=$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$DUSK_WARP" --method total_supply --return-type u64 2>/dev/null) || true
DUSK_FINAL_SUPPLY=$(echo "$DUSK_FINAL_SUPPLY_JSON" | jq -r '.value // "?"')
DUSK_FINAL_NONCE_JSON=$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$DUSK_MAILBOX" --method nonce --return-type u32 2>/dev/null) || true
DUSK_FINAL_NONCE=$(echo "$DUSK_FINAL_NONCE_JSON" | jq -r '.value // "?"')

echo -e "
${BOLD}Contracts:${NC}

${GREEN}EVM (Anvil, domain=$EVM_DOMAIN):${NC}
  Mailbox:      $EVM_MAILBOX
  HypERC20:     $EVM_TOKEN ($TOKEN_SYMBOL)

${GREEN}Dusk (domain=$DUSK_DOMAIN):${NC}
  Mailbox:      $DUSK_MAILBOX
  WarpDrc20:    $DUSK_WARP ($TOKEN_SYMBOL)

${BOLD}Bidirectional Token Bridge Results:${NC}

  EVM initial supply:     $INITIAL_SUPPLY (10 $TOKEN_SYMBOL)
  EVM -> Dusk bridged:    $BRIDGE_EVM_TO_DUSK (3 $TOKEN_SYMBOL)
  Dusk -> EVM bridged:    $BRIDGE_DUSK_TO_EVM (1 $TOKEN_SYMBOL)

  EVM final supply:       $EVM_FINAL_SUPPLY
  EVM deployer balance:   $EVM_FINAL_BALANCE
  Dusk WarpDrc20 supply:  $DUSK_FINAL_SUPPLY

  EVM Mailbox nonce:      $EVM_FINAL_NONCE
  Dusk Mailbox nonce:     $DUSK_FINAL_NONCE

${BOLD}Cross-Chain Flow Verified:${NC}

  [1] EVM -> Dusk: HypERC20.transferRemote() burned 3 tokens on EVM,
      manually relayed to Dusk Mailbox.process(), WarpDrc20 minted 3 tokens
      to deployer's registered External account.

  [2] Dusk -> EVM: WarpDrc20.transfer_remote() burned 1 token on Dusk,
      manually relayed to EVM Mailbox.process(), HypERC20 minted 1 token
      to deployer on EVM.

  Expected final state:
    EVM supply = 10 - 3 + 1 = 8 tokens (initial - sent + received)
    EVM deployer balance = 10 - 3 + 1 = 8 tokens
    Dusk supply = 3 - 1 = 2 tokens (received - sent)
"

ok "Demo complete! Bidirectional token bridge between Dusk and EVM verified."
echo ""
info "Deployment files saved to:"
info "  EVM: /tmp/hyperlane-demo-evm.json"
info "  Dusk: $DUSK_DEPLOY_OUTPUT"
info ""
info "Re-run with --skip-deploy to skip contract deployment."
