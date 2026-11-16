#!/usr/bin/env bash
# =============================================================================
# Rehearse the testnet deployment against the local demo environment
# =============================================================================
#
# Stands up a stand-in for Sepolia's Hyperlane core contracts on the local
# Anvil, hooked up as on Sepolia: the Mailbox requires a ProtocolFee hook
# (1 wei), and its default hook routes by destination, falling back to the
# MerkleTreeHook the validators sign for a destination it has no route for,
# such as Dusk. Also a ValidatorAnnounce and a StaticAggregationHookFactory.
# Then writes deploy/local.env so that deploy/testnet.sh targets the local
# Rusk node and Anvil instead of the real networks.
#
# A route's hook replaces the default hook, so as on Sepolia only a route
# hook that inserts into the MerkleTreeHook itself gets its messages signed.
#
# Requires `bash demo/start-env.sh` to be running.
#
#   bash deploy/local-evm-core.sh
#   TESTNET_ENV=deploy/local.env bash deploy/testnet.sh deploy
#   TESTNET_ENV=deploy/local.env bash deploy/testnet.sh agent-configs
#   bash deploy/local-agents.sh start
#
# Every run deploys a new EVM core, so state from an earlier rehearsal in
# deploy/out/local-rehearsal/ no longer applies and is moved aside.
#
# Validator roles use Anvil's development accounts:
#   #2           stands in for Hyperlane's Sepolia validator (EVM origin)
#   #3, #4, #5   Dusk-origin validators, threshold 2
# Their private keys are Anvil's well-known development keys.
#
# With DUSK_MULTICALL_WASM set to Multicall for Dusk's v0.1.0 Wasm (a release
# asset of dusk-multicall, or its `make wasm`), the local Rusk node gets the
# multicall too, which its genesis lacks: deployed with nonce 0 and the
# published owner, it lands at the ID it has on testnet and mainnet, and the
# rehearsal's agents batch their contract reads through it as testnet's do.

set -euo pipefail

DUSK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=/dev/null
source "$DUSK_DIR/demo/.env.bridge"

fail() { echo "[FAIL] $*" >&2; exit 1; }
ok() { echo "[OK]   $*" >&2; }
warn() { echo "[WARN] $*" >&2; }

# Multicall for Dusk v0.1.0: its Wasm's SHA-256, its published owner and
# the ID they give with deploy nonce 0 on any chain.
MULTICALL_SHA256=65f77a27c5bf6c7bf78797071dadca102ca3a67e5112c42af0242660e03693d9
MULTICALL_OWNER=r2h82mj2ehNhYPphK3GHVpG98MhSsZNCAF9sdSS4nwQJKqVBwihZn7bZXb9VoWVSqtu6onZ8Tkm11zdnavdCfuCD1kS2We4VPxvLeDf73a1sGiG25HUFvCwQJm3Ud8q6KLW
MULTICALL_ID=93de287b7d349d678a0038fe7772e8960e1a84a1b5da2d0b1f62f532616ce1d4

cast block-number --rpc-url "$ANVIL_RPC" >/dev/null 2>&1 || fail "Anvil is not running at $ANVIL_RPC (run demo/start-env.sh)"
# The chain ID transactions are signed for. /on/node/info reports null on a
# local node, which runs without a network ID.
DUSK_CHAIN_ID="$(curl -sf -m 10 -X POST -H 'Content-Type: application/octet-stream' \
    "${DUSK_RUES_URL%/}/on/contracts:0100000000000000000000000000000000000000000000000000000000000000/chain_id" \
    | od -An -tu1 | tr -d ' \n')"
[[ "$DUSK_CHAIN_ID" =~ ^[0-9]+$ ]] || fail "Rusk is not running at $DUSK_RUES_URL"
EVM_CHAIN_ID="$(cast chain-id --rpc-url "$ANVIL_RPC")"

DUSK_MULTICALL=""
if [ -n "${DUSK_MULTICALL_WASM:-}" ]; then
    [ "$(sha256sum "$DUSK_MULTICALL_WASM" 2>/dev/null | awk '{print $1}')" = "$MULTICALL_SHA256" ] \
        || fail "DUSK_MULTICALL_WASM ($DUSK_MULTICALL_WASM) isn't Multicall for Dusk v0.1.0, whose Wasm has SHA-256 $MULTICALL_SHA256"
    # dusk-tx checks the Wasm, nonce and owner give the published ID before
    # it sends anything. It takes a contract already at the ID only once Rusk
    # reports the published owner for it and it answers as the multicall
    # does (--check-multicall), and checks the contract it deploys the same
    # way.
    multicall="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" deploy-wasm --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" --wasm "$DUSK_MULTICALL_WASM" --owner "$MULTICALL_OWNER" --deploy-nonce 0 \
        --expected-id "$MULTICALL_ID" --expected-chain-id "$DUSK_CHAIN_ID" --check-multicall)" \
        || fail "Multicall for Dusk deployment failed: $(jq -r '.error // empty' <<<"$multicall" 2>/dev/null)"
    DUSK_MULTICALL="$MULTICALL_ID"
    ok "Multicall for Dusk at $MULTICALL_ID: $(jq -r 'if .deployed then "deployed, \(.gas_spent) gas" else "already there, checked (\(.verified | join(", ")))" end' <<<"$multicall")"
else
    warn "DUSK_MULTICALL_WASM is not set: the local chain has no Multicall for Dusk, and the rehearsal's agents make single contract queries"
fi

deploy() {
    local contract="$1"; shift
    (cd "$SOLIDITY_DIR" && forge create "$contract" --broadcast --json \
        --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" "$@") | jq -er '.deployedTo'
}
send() { cast send "$@" --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" >/dev/null; }

MAILBOX="$(deploy contracts/Mailbox.sol:Mailbox --constructor-args "$EVM_CHAIN_ID")"
MERKLE="$(deploy contracts/hooks/MerkleTreeHook.sol:MerkleTreeHook --constructor-args "$MAILBOX")"
# Sepolia's: maximum 1 gwei; Hyperlane's own scripts set the fee to 0, and
# Sepolia quotes 1 wei, which exercises the split of msg.value.
PROTOCOL_FEE="$(deploy contracts/hooks/ProtocolFee.sol:ProtocolFee \
    --constructor-args 1000000000 1 "$ANVIL_DEPLOYER" "$ANVIL_DEPLOYER")"
DEFAULT_HOOK="$(deploy contracts/hooks/routing/FallbackDomainRoutingHook.sol:FallbackDomainRoutingHook \
    --constructor-args "$MAILBOX" "$ANVIL_DEPLOYER" "$MERKLE")"
DEFAULT_ISM="$(deploy contracts/test/TestIsm.sol:TestIsm)"
VA="$(deploy contracts/isms/multisig/ValidatorAnnounce.sol:ValidatorAnnounce --constructor-args "$MAILBOX")"
AGGREGATION_HOOK_FACTORY="$(deploy contracts/hooks/aggregation/StaticAggregationHookFactory.sol:StaticAggregationHookFactory)"
send "$MAILBOX" "initialize(address,address,address,address)" \
    "$ANVIL_DEPLOYER" "$DEFAULT_ISM" "$DEFAULT_HOOK" "$PROTOCOL_FEE"
ok "Local EVM core: Mailbox $MAILBOX, MerkleTreeHook $MERKLE, ValidatorAnnounce $VA"
ok "Required hook: ProtocolFee $PROTOCOL_FEE; default hook: FallbackDomainRoutingHook $DEFAULT_HOOK, falling back to the MerkleTreeHook"

REHEARSAL_OUT="$DUSK_DIR/deploy/out/local-rehearsal"
if [ -e "$REHEARSAL_OUT" ]; then
    STALE="$REHEARSAL_OUT.$(date +%Y%m%d-%H%M%S)"
    mv "$REHEARSAL_OUT" "$STALE"
    ok "Moved the previous rehearsal's state to $STALE"
fi

cat > "$DUSK_DIR/deploy/local.env" <<EOF
# Generated by deploy/local-evm-core.sh for a local rehearsal. Not for real
# networks: the keys below are public development keys.
source "\$DUSK_DIR/deploy/testnet.env.example"
DEPLOY_NAME=local-rehearsal
DUSK_RUES_URL=$DUSK_RUES_URL
DUSK_EXPECTED_CHAIN_ID=$DUSK_CHAIN_ID
DUSK_KEYS=$CONSENSUS_KEYS
export DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD"
EVM_CHAIN_NAME=anvil
EVM_RPC=$ANVIL_RPC
EVM_CHAIN_ID=$EVM_CHAIN_ID
EVM_DOMAIN=$EVM_CHAIN_ID
EVM_REORG_PERIOD=0
EVM_BLOCK_TIME=1
EVM_MAILBOX=$MAILBOX
EVM_MERKLE_TREE_HOOK=$MERKLE
EVM_VALIDATOR_ANNOUNCE=$VA
EVM_ISM_FACTORY=
EVM_AGGREGATION_HOOK_FACTORY=$AGGREGATION_HOOK_FACTORY
export EVM_DEPLOYER_KEY=$ANVIL_PRIVATE_KEY
EVM_ORIGIN_VALIDATORS=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
EVM_ORIGIN_THRESHOLD=1
DUSK_ORIGIN_VALIDATORS=0x90F79bf6EB2c4f870365E785982E1f101E93b906,0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65,0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc
DUSK_ORIGIN_THRESHOLD=2
SOLIDITY_DIR=$SOLIDITY_DIR
AGENT_STATE_DIR="\$DUSK_DIR/deploy/out/local-rehearsal/agents/state"
DUSK_SIGNER_KEY_FILE="\$DUSK_DIR/deploy/out/local-rehearsal/agents/dusk-signer.key"
# Multicall for Dusk on the local chain, or empty without one.
DUSK_MULTICALL=$DUSK_MULTICALL
EOF
ok "Wrote deploy/local.env. Next: TESTNET_ENV=deploy/local.env bash deploy/testnet.sh deploy"
