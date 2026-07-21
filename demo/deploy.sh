#!/usr/bin/env bash
# =============================================================================
# Deploy Hyperlane Contracts on Both Chains
# =============================================================================
#
# Deploys Hyperlane on EVM (Anvil) and Dusk (Rusk), enrolls routers, and
# registers the deployer's BLS account for token bridging.
#
# Usage:
#   bash deploy.sh --dusk-ism testMock
#   bash deploy.sh --dusk-ism messageIdMultisig --multisig-validators <addr> --multisig-threshold 1
#   bash deploy.sh --skip-deploy  Reuse existing deployment files
#   bash deploy.sh --reset      Delete existing and redeploy
#
# Requires: start-env.sh running (Rusk + Anvil)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BRIDGE_STATE_OVERRIDE="${BRIDGE_STATE_FILE:-}"
source "$SCRIPT_DIR/.env.bridge"
if [ -n "$BRIDGE_STATE_OVERRIDE" ]; then
    BRIDGE_STATE_FILE="$BRIDGE_STATE_OVERRIDE"
fi

# ── Helpers ──────────────────────────────────────────────────────────────────

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
header() { echo -e "\n${BOLD}${GREEN}═══ $* ═══${NC}\n"; }
step()   { echo -e "${CYAN}  -> $*${NC}"; }

pad_evm_address() {
    local addr="${1#0x}"
    addr="$(echo "$addr" | tr '[:upper:]' '[:lower:]')"
    echo "000000000000000000000000${addr}"
}

# ── Parse Arguments ──────────────────────────────────────────────────────────

SKIP_DEPLOY=false
RESET=false

# Dusk Mailbox default ISM. `testMock` is permissive (no validator/metadata).
# `messageIdMultisig` requires running a validator agent for the EVM origin chain.
DUSK_DEFAULT_ISM="${DUSK_DEFAULT_ISM:-}"
MULTISIG_VALIDATORS="${MULTISIG_VALIDATORS:-$ANVIL_DEPLOYER}"
MULTISIG_THRESHOLD="${MULTISIG_THRESHOLD:-1}"
DUSK_DISPATCH_FEE_CREDIT="${DUSK_DISPATCH_FEE_CREDIT:-1000000000}"
DUSK_IGP_GAS_OVERHEAD="${DUSK_IGP_GAS_OVERHEAD:-50000}"
DUSK_IGP_TOKEN_EXCHANGE_RATE="${DUSK_IGP_TOKEN_EXCHANGE_RATE:-10000000000}"
DUSK_IGP_GAS_PRICE="${DUSK_IGP_GAS_PRICE:-1}"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --skip-deploy)
            SKIP_DEPLOY=true
            shift
            ;;
        --reset)
            RESET=true
            shift
            ;;
        --dusk-ism)
            DUSK_DEFAULT_ISM="${2:-}"
            shift 2
            ;;
        --multisig-validators)
            MULTISIG_VALIDATORS="${2:-}"
            shift 2
            ;;
        --multisig-threshold)
            MULTISIG_THRESHOLD="${2:-}"
            shift 2
            ;;
        *)
            fail "Unknown argument: $1"
            ;;
    esac
done

if [ "$SKIP_DEPLOY" = false ] && [ -z "$DUSK_DEFAULT_ISM" ]; then
    fail "Select the Dusk Mailbox policy explicitly with --dusk-ism testMock or --dusk-ism messageIdMultisig"
fi
if [ -n "$DUSK_DEFAULT_ISM" ] && [ "$DUSK_DEFAULT_ISM" != "testMock" ] && [ "$DUSK_DEFAULT_ISM" != "messageIdMultisig" ]; then
    fail "Invalid --dusk-ism '$DUSK_DEFAULT_ISM' (expected: testMock or messageIdMultisig)"
fi

if [ "$RESET" = true ]; then
    info "Resetting — removing existing deployment files..."
    rm -f /tmp/hyperlane-demo-evm.json
    rm -f /tmp/hyperlane-demo-dusk-deploy.json
    rm -f "$BRIDGE_STATE_FILE"
fi

# ── Prerequisites ────────────────────────────────────────────────────────────

header "Prerequisites"

# Check binaries
[ -f "$DUSK_TX" ] || fail "dusk-tx not found at $DUSK_TX — run start-env.sh first"
ok "dusk-tx: $DUSK_TX"

[ -f "$WASM_DIR/hyperlane_dusk_mailbox.wasm" ] || fail "WASMs not found — run start-env.sh first"
ok "Contract WASMs: $WASM_DIR"

[ -f "$CONSENSUS_KEYS" ] || fail "Consensus keys not found at $CONSENSUS_KEYS"
ok "Consensus keys: $CONSENSUS_KEYS"

for tool in forge cast jq; do
    command -v "$tool" &>/dev/null || fail "$tool not found"
done
ok "Tools: forge, cast, jq"

[ -d "$SOLIDITY_DIR" ] || fail "Solidity directory not found at $SOLIDITY_DIR"
if [ ! -d "$SOLIDITY_DIR/dependencies" ]; then
    info "Installing Solidity dependencies..."
    (cd "$SOLIDITY_DIR" && forge soldeer install) || fail "Failed to install Solidity deps"
fi
ok "Solidity deps installed"

# Check connectivity
cast block-number --rpc-url "$ANVIL_RPC" >/dev/null 2>&1 || fail "Cannot connect to Anvil at $ANVIL_RPC"
EVM_CHAIN_ID="$(cast chain-id --rpc-url "$ANVIL_RPC")" || fail "Cannot query Anvil chain ID"
ok "Anvil reachable"

DUSK_CHAIN_ID=$(curl -s -X POST \
    -H "Content-Type: application/octet-stream" \
    "${DUSK_RUES_URL}on/contracts:0100000000000000000000000000000000000000000000000000000000000000/chain_id" \
    --max-time 5 2>/dev/null | xxd -p 2>/dev/null) || true
[ -n "$DUSK_CHAIN_ID" ] || fail "Cannot connect to Dusk RUES at $DUSK_RUES_URL"
ok "Dusk RUES reachable"

validate_evm_contract() {
    local address="$1"
    local label="$2"
    local code

    [ -n "$address" ] && [ "$address" != "null" ] || fail "Saved $label address is missing"
    code="$(cast code "$address" --rpc-url "$ANVIL_RPC" 2>/dev/null)" \
        || fail "Cannot query saved $label at $address"
    [ "$code" != "0x" ] && [ -n "$code" ] \
        || fail "Saved $label is not deployed on the running EVM chain: $address"
}

query_evm_address() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local value

    value="$(cast call "$contract" "$method()(address)" --rpc-url "$ANVIL_RPC" 2>/dev/null)" \
        || fail "Saved $label is not queryable on the running EVM chain: $contract"
    value="${value,,}"
    [[ "$value" =~ ^0x[0-9a-f]{40}$ ]] \
        || fail "Saved $label returned a malformed address"
    printf '%s\n' "$value"
}

query_evm_u32() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local value

    value="$(cast call "$contract" "$method()(uint32)" --rpc-url "$ANVIL_RPC" 2>/dev/null)" \
        || fail "Saved $label is not queryable on the running EVM chain: $contract"
    value="${value%% *}"
    [[ "$value" =~ ^[0-9]+$ ]] \
        || fail "Saved $label returned a malformed uint32"
    printf '%s\n' "$value"
}

query_evm_router() {
    local contract="$1"
    local domain="$2"
    local label="$3"
    local value

    value="$(cast call "$contract" "routers(uint32)(bytes32)" "$domain" --rpc-url "$ANVIL_RPC" 2>/dev/null)" \
        || fail "Saved $label is not queryable on the running EVM chain: $contract"
    value="${value,,}"
    [[ "$value" =~ ^0x[0-9a-f]{64}$ ]] \
        || fail "Saved $label returned malformed bytes32"
    printf '%s\n' "$value"
}

validate_dusk_query() {
    local contract="$1"
    local method="$2"
    local return_type="$3"
    local label="$4"

    [ -n "$contract" ] && [ "$contract" != "null" ] || fail "Saved $label contract ID is missing"
    "$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type "$return_type" \
        >/dev/null 2>&1 \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
}

validate_dusk_state_version() {
    local contract="$1"
    local label="$2"
    local expected_version="${3:-1}"
    local response version

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method state_version \
        --return-type u32 2>/dev/null) \
        || fail "Saved $label predates state version $expected_version; redeploy"
    version=$(jq -er '.value | tonumber' <<<"$response") \
        || fail "Saved $label returned a malformed state version; redeploy"
    [ "$version" = "$expected_version" ] \
        || fail "Saved $label has unsupported state version $version; expected $expected_version"
}

query_dusk_bytes32() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type bytes32 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -er '.value | ascii_downcase' <<<"$response" \
        || fail "Saved $label returned a malformed bytes32 value"
}

query_dusk_bytes32_u32() {
    local contract="$1"
    local method="$2"
    local argument="$3"
    local label="$4"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type bytes32 \
        --arg-u32 "$argument" 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -er '.value | ascii_downcase' <<<"$response" \
        || fail "Saved $label returned a malformed bytes32 value"
}

query_dusk_option_bytes32() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type option-bytes32 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -er '.value | strings | ascii_downcase' <<<"$response" \
        || fail "Saved $label returned an empty or malformed owner"
}

query_dusk_contract_id_list() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type contract-id-list 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -ce '.value | map(ascii_downcase)' <<<"$response" \
        || fail "Saved $label returned a malformed contract-id list"
}

query_dusk_u64() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type u64 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -er '.value | tonumber' <<<"$response" \
        || fail "Saved $label returned a malformed u64 value"
}

query_dusk_u32() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type u32 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -er '.value | tonumber' <<<"$response" \
        || fail "Saved $label returned a malformed u32 value"
}

query_dusk_u8() {
    local contract="$1"
    local method="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method "$method" \
        --return-type u8 2>/dev/null) \
        || fail "Saved $label is not queryable on the running Dusk chain: $contract"
    jq -er '.value | tonumber' <<<"$response" \
        || fail "Saved $label returned a malformed u8 value"
}

query_dusk_domain_gas_config() {
    local contract="$1"
    local domain="$2"
    local label="$3"
    local response

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$contract" \
        --method domain_gas_config \
        --return-type domain-gas-config \
        --arg-u32 "$domain" 2>/dev/null) \
        || fail "Saved $label has no live gas configuration for domain $domain"
    jq -ce '.value | {
        gas_overhead: (.gas_overhead | tonumber),
        token_exchange_rate: (.token_exchange_rate | tonumber),
        gas_price: (.gas_price | tonumber)
    }' <<<"$response" \
        || fail "Saved $label returned a malformed domain gas configuration"
}

validate_saved_deployment() {
    local saved_evm_chain_id saved_dusk_chain_id saved_dusk_ism
    local evm_mailbox evm_token evm_native_token evm_collateral_token evm_ism evm_hook
    local evm_merkle evm_validator_announce evm_igp evm_recipient
    local dusk_mailbox dusk_test_mock dusk_ism_multisig dusk_default_ism
    local dusk_merkle dusk_warp dusk_warp_native dusk_warp_collateral
    local dusk_validator_announce dusk_igp dusk_protocol_fee dusk_aggregation_hook
    local dusk_test_recipient expected_default_ism live_default_ism
    local live_default_hook live_required_hook live_route_value
    local saved_igp_config saved_igp_domain live_igp_config
    local live_evm_value route expected_router
    local saved_account_h256 expected_children

    jq -e 'type == "object" and (.evm | type == "object") and (.dusk | type == "object")' \
        "$BRIDGE_STATE_FILE" >/dev/null \
        || fail "Saved deployment state is malformed: $BRIDGE_STATE_FILE"

    saved_evm_chain_id="$(jq -er '.evm_chain_id | tostring' "$BRIDGE_STATE_FILE")" \
        || fail "Saved deployment lacks evm_chain_id; redeploy instead of using --skip-deploy"
    saved_dusk_chain_id="$(jq -er '.dusk_chain_id | strings | select(length > 0)' "$BRIDGE_STATE_FILE")" \
        || fail "Saved deployment lacks dusk_chain_id; redeploy instead of using --skip-deploy"
    [ "$saved_evm_chain_id" = "$EVM_CHAIN_ID" ] \
        || fail "Saved EVM chain ID $saved_evm_chain_id does not match running chain $EVM_CHAIN_ID"
    [ "$saved_dusk_chain_id" = "$DUSK_CHAIN_ID" ] \
        || fail "Saved Dusk chain ID does not match the running chain"
    saved_dusk_ism="$(jq -er '.dusk_default_ism | strings | select(. == "testMock" or . == "messageIdMultisig")' "$BRIDGE_STATE_FILE")" \
        || fail "Saved deployment lacks a supported dusk_default_ism; redeploy"
    saved_account_h256="$(jq -er '.account_h256 | strings | ascii_downcase | select(test("^[0-9a-f]{64}$"))' "$BRIDGE_STATE_FILE")" \
        || fail "Saved deployment lacks the Dusk owner identity; redeploy"

    evm_mailbox="$(jq -er '.evm.mailbox' "$BRIDGE_STATE_FILE")"
    evm_token="$(jq -er '.evm.token' "$BRIDGE_STATE_FILE")"
    evm_native_token="$(jq -er '.evm.native_token' "$BRIDGE_STATE_FILE")"
    evm_collateral_token="$(jq -er '.evm.collateral_token' "$BRIDGE_STATE_FILE")"
    evm_ism="$(jq -er '.evm.ism' "$BRIDGE_STATE_FILE")"
    evm_hook="$(jq -er '.evm.hook' "$BRIDGE_STATE_FILE")"
    evm_merkle="$(jq -er '.evm.merkle_tree_hook' "$BRIDGE_STATE_FILE")"
    evm_validator_announce="$(jq -er '.evm.validator_announce' "$BRIDGE_STATE_FILE")"
    evm_igp="$(jq -er '.evm.igp' "$BRIDGE_STATE_FILE")"
    evm_recipient="$(jq -er '.evm.recipient' "$BRIDGE_STATE_FILE")"
    dusk_mailbox="$(jq -er '.dusk.mailbox' "$BRIDGE_STATE_FILE")"
    dusk_test_mock="$(jq -er '.dusk.test_mock' "$BRIDGE_STATE_FILE")"
    dusk_ism_multisig="$(jq -er '.dusk.ism_multisig // ""' "$BRIDGE_STATE_FILE")"
    dusk_default_ism="$(jq -er '.dusk.default_ism' "$BRIDGE_STATE_FILE")"
    dusk_merkle="$(jq -er '.dusk.merkle_tree_hook' "$BRIDGE_STATE_FILE")"
    dusk_warp="$(jq -er '.dusk.warp_drc20' "$BRIDGE_STATE_FILE")"
    dusk_warp_native="$(jq -er '.dusk.warp_native' "$BRIDGE_STATE_FILE")"
    dusk_warp_collateral="$(jq -er '.dusk.warp_drc20_collateral' "$BRIDGE_STATE_FILE")"
    dusk_validator_announce="$(jq -er '.dusk.validator_announce' "$BRIDGE_STATE_FILE")"
    dusk_igp="$(jq -er '.dusk.igp' "$BRIDGE_STATE_FILE")"
    dusk_protocol_fee="$(jq -er '.dusk.protocol_fee' "$BRIDGE_STATE_FILE")"
    dusk_aggregation_hook="$(jq -er '.dusk.aggregation_hook' "$BRIDGE_STATE_FILE")"
    dusk_test_recipient="$(jq -er '.dusk.test_recipient' "$BRIDGE_STATE_FILE")"
    saved_igp_config="$(jq -ce '.dusk.igp_domain_config | {
        gas_overhead: (.gas_overhead | tonumber),
        token_exchange_rate: (.token_exchange_rate | tonumber),
        gas_price: (.gas_price | tonumber)
    }' "$BRIDGE_STATE_FILE")" \
        || fail "Saved deployment lacks a valid Dusk IGP domain configuration; redeploy"
    saved_igp_domain="$(jq -er '.dusk.igp_domain_config.domain | tonumber' "$BRIDGE_STATE_FILE")" \
        || fail "Saved deployment lacks the Dusk IGP destination domain; redeploy"
    [ "$saved_igp_domain" = "$EVM_DOMAIN" ] \
        || fail "Saved Dusk IGP destination $saved_igp_domain does not match EVM domain $EVM_DOMAIN"

    validate_evm_contract "$evm_mailbox" "EVM Mailbox"
    validate_evm_contract "$evm_token" "EVM warp token"
    validate_evm_contract "$evm_native_token" "EVM native route token"
    validate_evm_contract "$evm_collateral_token" "EVM collateral route token"
    validate_evm_contract "$evm_ism" "EVM ISM"
    validate_evm_contract "$evm_hook" "EVM hook"
    validate_evm_contract "$evm_merkle" "EVM MerkleTreeHook"
    validate_evm_contract "$evm_validator_announce" "EVM ValidatorAnnounce"
    validate_evm_contract "$evm_igp" "EVM IGP"
    validate_evm_contract "$evm_recipient" "EVM test recipient"
    live_evm_value="$(query_evm_u32 "$evm_mailbox" localDomain "EVM Mailbox local domain")"
    [ "$live_evm_value" = "$EVM_DOMAIN" ] \
        || fail "Running EVM Mailbox local domain differs from the saved deployment; redeploy"
    live_evm_value="$(query_evm_address "$evm_mailbox" defaultIsm "EVM Mailbox default ISM")"
    [ "$live_evm_value" = "${evm_ism,,}" ] \
        || fail "Running EVM Mailbox default ISM differs from the saved deployment; redeploy"
    live_evm_value="$(query_evm_address "$evm_mailbox" defaultHook "EVM Mailbox default hook")"
    [ "$live_evm_value" = "${evm_hook,,}" ] \
        || fail "Running EVM Mailbox default hook differs from the saved deployment; redeploy"
    live_evm_value="$(query_evm_address "$evm_mailbox" requiredHook "EVM Mailbox required hook")"
    [ "$live_evm_value" = "${evm_merkle,,}" ] \
        || fail "Running EVM Mailbox required hook differs from the saved deployment; redeploy"
    live_evm_value="$(query_evm_address "$evm_mailbox" owner "EVM Mailbox owner")"
    [ "$live_evm_value" = "${ANVIL_DEPLOYER,,}" ] \
        || fail "Running EVM Mailbox owner differs from the deployment owner; redeploy"
    for dependency in "$evm_validator_announce:ValidatorAnnounce" "$evm_merkle:MerkleTreeHook"; do
        IFS=: read -r contract label <<<"$dependency"
        live_evm_value="$(query_evm_address "$contract" mailbox "EVM $label Mailbox")"
        [ "$live_evm_value" = "${evm_mailbox,,}" ] \
            || fail "Running EVM $label is bound to a different Mailbox; redeploy"
        live_evm_value="$(query_evm_u32 "$contract" localDomain "EVM $label local domain")"
        [ "$live_evm_value" = "$EVM_DOMAIN" ] \
            || fail "Running EVM $label local domain differs from the Mailbox; redeploy"
    done
    for route in "$evm_token" "$evm_native_token" "$evm_collateral_token"; do
        live_evm_value="$(query_evm_address "$route" mailbox "EVM warp route Mailbox")"
        [ "$live_evm_value" = "${evm_mailbox,,}" ] \
            || fail "Running EVM warp route is bound to a different Mailbox; redeploy"
        live_evm_value="$(query_evm_u32 "$route" localDomain "EVM warp route local domain")"
        [ "$live_evm_value" = "$EVM_DOMAIN" ] \
            || fail "Running EVM warp route local domain differs from the Mailbox; redeploy"
        live_evm_value="$(query_evm_address "$route" hook "EVM warp route hook")"
        [ "$live_evm_value" = "${evm_hook,,}" ] \
            || fail "Running EVM warp route hook differs from the saved deployment; redeploy"
        live_evm_value="$(query_evm_address "$route" interchainSecurityModule "EVM warp route ISM")"
        [ "$live_evm_value" = "${evm_ism,,}" ] \
            || fail "Running EVM warp route ISM differs from the saved deployment; redeploy"
        live_evm_value="$(query_evm_address "$route" owner "EVM warp route owner")"
        [ "$live_evm_value" = "${ANVIL_DEPLOYER,,}" ] \
            || fail "Running EVM warp route owner differs from the deployment owner; redeploy"
    done
    for route in \
        "$evm_token:$dusk_warp" \
        "$evm_native_token:$dusk_warp_native" \
        "$evm_collateral_token:$dusk_warp_collateral"; do
        IFS=: read -r contract expected_router <<<"$route"
        live_evm_value="$(query_evm_router "$contract" "$DUSK_DOMAIN" "EVM remote router")"
        [ "$live_evm_value" = "0x${expected_router,,}" ] \
            || fail "Running EVM warp route has the wrong Dusk router; redeploy"
    done
    for method in owner beneficiary; do
        live_evm_value="$(query_evm_address "$evm_igp" "$method" "EVM IGP $method")"
        [ "$live_evm_value" = "${ANVIL_DEPLOYER,,}" ] \
            || fail "Running EVM IGP $method differs from the deployment policy; redeploy"
    done
    validate_dusk_query "$dusk_mailbox" nonce u32 "Dusk Mailbox"
    validate_dusk_state_version "$dusk_mailbox" "Dusk Mailbox" 2
    [ "$(query_dusk_u32 "$dusk_mailbox" local_domain "Dusk Mailbox local domain")" = "$DUSK_DOMAIN" ] \
        || fail "Running Dusk Mailbox local domain differs from the saved deployment; redeploy"
    validate_dusk_state_version "$dusk_test_mock" "Dusk TestMock"
    [ "$(query_dusk_u8 "$dusk_test_mock" module_type "Dusk TestMock module type")" = "6" ] \
        || fail "Running Dusk TestMock has the wrong ISM module type; redeploy"
    if [ "$saved_dusk_ism" = "messageIdMultisig" ]; then
        [ -n "$dusk_ism_multisig" ] \
            || fail "Saved multisig deployment lacks its Dusk ISM contract ID; redeploy"
        validate_dusk_state_version "$dusk_ism_multisig" "Dusk multisig ISM"
        [ "$(query_dusk_u8 "$dusk_ism_multisig" module_type "Dusk multisig ISM module type")" = "5" ] \
            || fail "Running Dusk multisig ISM has the wrong module type; redeploy"
        expected_default_ism="$dusk_ism_multisig"
    else
        [ -z "$dusk_ism_multisig" ] \
            || fail "Saved TestMock deployment unexpectedly records a multisig ISM; redeploy"
        expected_default_ism="$dusk_test_mock"
    fi
    [ "${dusk_default_ism,,}" = "${expected_default_ism,,}" ] \
        || fail "Saved Dusk default ISM does not match saved dusk_default_ism policy; redeploy"
    live_default_ism="$(query_dusk_bytes32 "$dusk_mailbox" default_ism "Dusk Mailbox default ISM")"
    [ "$live_default_ism" = "${expected_default_ism,,}" ] \
        || fail "Running Dusk Mailbox default ISM does not match saved deployment policy; redeploy"
    live_default_hook="$(query_dusk_bytes32 "$dusk_mailbox" default_hook "Dusk Mailbox default hook")"
    [ "$live_default_hook" = "${dusk_igp,,}" ] \
        || fail "Running Dusk Mailbox default hook is not the saved IGP; redeploy"
    live_required_hook="$(query_dusk_bytes32 "$dusk_mailbox" required_hook "Dusk Mailbox required hook")"
    [ "$live_required_hook" = "${dusk_aggregation_hook,,}" ] \
        || fail "Running Dusk Mailbox required hook is not the saved aggregation hook; redeploy"
    live_route_value="$(query_dusk_option_bytes32 "$dusk_mailbox" owner "Dusk Mailbox owner")"
    [ "$live_route_value" = "$saved_account_h256" ] \
        || fail "Running Dusk Mailbox owner differs from the saved deployment owner; redeploy"
    validate_dusk_state_version "$dusk_merkle" "Dusk MerkleTreeHook"
    validate_dusk_state_version "$dusk_warp" "Dusk synthetic warp route" 4
    validate_dusk_state_version "$dusk_warp_native" "Dusk native warp route" 2
    validate_dusk_state_version "$dusk_warp_collateral" "Dusk collateral warp route" 3
    validate_dusk_state_version "$dusk_validator_announce" "Dusk ValidatorAnnounce"
    validate_dusk_state_version "$dusk_igp" "Dusk IGP" 2
    validate_dusk_state_version "$dusk_protocol_fee" "Dusk ProtocolFee"
    validate_dusk_state_version "$dusk_aggregation_hook" "Dusk AggregationHook"
    validate_dusk_state_version "$dusk_test_recipient" "Dusk test recipient"
    validate_dusk_query "$dusk_warp_collateral" mailbox bytes32 "Dusk collateral warp route"
    [ "$(query_dusk_u32 "$dusk_validator_announce" local_domain "Dusk ValidatorAnnounce local domain")" = "$DUSK_DOMAIN" ] \
        || fail "Running Dusk ValidatorAnnounce local domain differs from the Mailbox; redeploy"
    [ "$(query_dusk_u8 "$dusk_igp" hook_type "Dusk IGP hook type")" = "4" ] \
        || fail "Running Dusk IGP has the wrong hook type; redeploy"
    live_igp_config="$(query_dusk_domain_gas_config "$dusk_igp" "$saved_igp_domain" "Dusk IGP")"
    [ "$live_igp_config" = "$saved_igp_config" ] \
        || fail "Running Dusk IGP configuration does not match saved deployment policy; redeploy"
    [ "$(query_dusk_u8 "$dusk_protocol_fee" hook_type "Dusk ProtocolFee hook type")" = "6" ] \
        || fail "Running Dusk ProtocolFee has the wrong hook type; redeploy"
    [ "$(query_dusk_u8 "$dusk_aggregation_hook" hook_type "Dusk AggregationHook hook type")" = "2" ] \
        || fail "Running Dusk AggregationHook has the wrong hook type; redeploy"
    [ "$(query_dusk_u8 "$dusk_merkle" hook_type "Dusk MerkleTreeHook hook type")" = "3" ] \
        || fail "Running Dusk MerkleTreeHook has the wrong hook type; redeploy"
    validate_dusk_query "$dusk_test_recipient" handled_count u32 "Dusk test recipient"
    for route in "$dusk_warp" "$dusk_warp_native" "$dusk_warp_collateral"; do
        live_route_value="$(query_dusk_bytes32 "$route" mailbox "Dusk warp route Mailbox")"
        [ "$live_route_value" = "${dusk_mailbox,,}" ] \
            || fail "Running Dusk warp route is bound to a different Mailbox; redeploy"
        live_route_value="$(query_dusk_bytes32 "$route" hook "Dusk warp route hook")"
        [ "$live_route_value" = "$(printf '%064d' 0)" ] \
            || fail "Running Dusk warp route hook differs from the saved zero-override policy; redeploy"
        live_route_value="$(query_dusk_bytes32 "$route" interchain_security_module "Dusk warp route ISM")"
        [ "$live_route_value" = "$(printf '%064d' 0)" ] \
            || fail "Running Dusk warp route ISM differs from the saved zero-override policy; redeploy"
    done
    live_route_value="$(query_dusk_bytes32_u32 "$dusk_warp" enrolled_router "$EVM_DOMAIN" "Dusk synthetic remote router")"
    [ "$live_route_value" = "$(pad_evm_address "$evm_token")" ] \
        || fail "Running Dusk synthetic route has the wrong EVM router; redeploy"
    live_route_value="$(query_dusk_bytes32_u32 "$dusk_warp_native" enrolled_router "$EVM_DOMAIN" "Dusk native remote router")"
    [ "$live_route_value" = "$(pad_evm_address "$evm_native_token")" ] \
        || fail "Running Dusk native route has the wrong EVM router; redeploy"
    live_route_value="$(query_dusk_bytes32_u32 "$dusk_warp_collateral" enrolled_router "$EVM_DOMAIN" "Dusk collateral remote router")"
    [ "$live_route_value" = "$(pad_evm_address "$evm_collateral_token")" ] \
        || fail "Running Dusk collateral route has the wrong EVM router; redeploy"
    live_route_value="$(query_dusk_bytes32 "$dusk_warp_collateral" wrapped_token "Dusk collateral wrapped token")"
    [ "$live_route_value" = "${dusk_warp,,}" ] \
        || fail "Running Dusk collateral route wraps a different token; redeploy"
    for route in "$dusk_warp" "$dusk_warp_native" "$dusk_warp_collateral"; do
        live_route_value="$(query_dusk_option_bytes32 "$route" owner "Dusk warp route owner")"
        [ "$live_route_value" = "$saved_account_h256" ] \
            || fail "Running Dusk warp route owner differs from the saved deployment owner; redeploy"
    done
    for dependency in \
        "$dusk_validator_announce:mailbox:$dusk_mailbox:ValidatorAnnounce" \
        "$dusk_igp:mailbox:$dusk_mailbox:IGP" \
        "$dusk_aggregation_hook:mailbox:$dusk_mailbox:AggregationHook" \
        "$dusk_merkle:mailbox:$dusk_aggregation_hook:MerkleTreeHook" \
        "$dusk_protocol_fee:mailbox:$dusk_aggregation_hook:ProtocolFee"; do
        IFS=: read -r contract method expected label <<<"$dependency"
        live_route_value="$(query_dusk_bytes32 "$contract" "$method" "Dusk $label dependency")"
        [ "$live_route_value" = "${expected,,}" ] \
            || fail "Running Dusk $label dependency differs from the saved topology; redeploy"
    done
    for contract in "$dusk_igp" "$dusk_protocol_fee"; do
        live_route_value="$(query_dusk_option_bytes32 "$contract" owner "Dusk fee hook owner")"
        [ "$live_route_value" = "$saved_account_h256" ] \
            || fail "Running Dusk fee hook owner differs from the saved deployment owner; redeploy"
        live_route_value="$(query_dusk_bytes32 "$contract" beneficiary "Dusk fee hook beneficiary")"
        [ "$live_route_value" = "$saved_account_h256" ] \
            || fail "Running Dusk fee hook beneficiary differs from the saved deployment owner; redeploy"
    done
    if [ "$saved_dusk_ism" = "messageIdMultisig" ]; then
        live_route_value="$(query_dusk_option_bytes32 "$dusk_ism_multisig" owner "Dusk multisig ISM owner")"
        [ "$live_route_value" = "$saved_account_h256" ] \
            || fail "Running Dusk multisig ISM owner differs from the saved deployment owner; redeploy"
    fi
    [ "$(query_dusk_u64 "$dusk_protocol_fee" protocol_fee "Dusk protocol fee")" = "1000000" ] \
        || fail "Running Dusk protocol fee differs from the deployment policy; redeploy"
    [ "$(query_dusk_u64 "$dusk_protocol_fee" max_protocol_fee "Dusk maximum protocol fee")" = "100000000" ] \
        || fail "Running Dusk maximum protocol fee differs from the deployment policy; redeploy"
    expected_children="$(jq -cn --arg merkle "${dusk_merkle,,}" --arg fee "${dusk_protocol_fee,,}" '[$merkle, $fee]')"
    [ "$(query_dusk_contract_id_list "$dusk_aggregation_hook" hooks "Dusk aggregation hook children")" = "$expected_children" ] \
        || fail "Running Dusk aggregation hook children differ from the saved topology; redeploy"
}

# ── Check for existing deployment ────────────────────────────────────────────

if [ "$SKIP_DEPLOY" = true ] && [ -f "$BRIDGE_STATE_FILE" ]; then
    info "Validating existing deployment (--skip-deploy)"
    validate_saved_deployment
    ok "Saved deployment matches the running chains and contracts"
    echo ""
    info "State file: $BRIDGE_STATE_FILE"
    jq '.' "$BRIDGE_STATE_FILE"
    echo ""
    ok "Deployment loaded. Run 'bash demo/bridge.sh status' to check balances."
    exit 0
fi

if [ "$SKIP_DEPLOY" = true ]; then
    fail "--skip-deploy requires the combined deployment state at $BRIDGE_STATE_FILE; per-chain artifacts are not a trusted reuse boundary"
fi

# ── Deploy on EVM ────────────────────────────────────────────────────────────

header "Deploy Hyperlane on EVM (domain=$EVM_DOMAIN)"

EVM_DEPLOY_FILE="/tmp/hyperlane-demo-evm.json"

cd "$SOLIDITY_DIR"

    # Pre-compile so forge create doesn't mix compiler output with JSON
    step "Compiling Solidity contracts..."
    forge build --quiet || fail "Solidity compilation failed"
    ok "Contracts compiled"

    # Helper: deploy a contract and extract the address from forge JSON output
    forge_deploy() {
        local contract="$1"; shift
        local output
        # Put --broadcast --json before remaining args so --constructor-args (greedy) doesn't eat them
        output=$(forge create "$contract" --broadcast --json "$@" 2>&1) || true
        # Extract deployedTo — collapse to single line first for jq
        local addr
        addr=$(echo "$output" | tr '\n' ' ' | grep -o '{[^{]*"deployedTo"[^}]*}' | jq -r '.deployedTo' 2>/dev/null)
        if [ -z "$addr" ] || [ "$addr" = "null" ]; then
            echo "forge create output: $output" >&2
            return 1
        fi
        echo "$addr"
    }

    if [ "$DUSK_DEFAULT_ISM" = "messageIdMultisig" ]; then
        step "Deploying EVM StorageMessageIdMultisigIsm..."
        EVM_ISM=$(forge_deploy contracts/isms/multisig/StorageMultisigIsm.sol:StorageMessageIdMultisigIsm \
            --rpc-url "$ANVIL_RPC" \
            --private-key "$ANVIL_PRIVATE_KEY" \
            --constructor-args "[$ANVIL_DEPLOYER]" 1) \
            || fail "Failed to deploy EVM MessageIdMultisigIsm"
        ok "EVM MessageIdMultisigIsm: $EVM_ISM"
    else
        step "Deploying TestIsm..."
        EVM_ISM=$(forge_deploy contracts/test/TestIsm.sol:TestIsm \
            --rpc-url "$ANVIL_RPC" \
            --private-key "$ANVIL_PRIVATE_KEY") || fail "Failed to deploy TestIsm"
        ok "TestIsm: $EVM_ISM"
    fi

    step "Deploying TestPostDispatchHook..."
    EVM_HOOK=$(forge_deploy contracts/test/TestPostDispatchHook.sol:TestPostDispatchHook \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY") || fail "Failed to deploy Hook"
    ok "TestPostDispatchHook: $EVM_HOOK"

    step "Deploying Mailbox (domain=$EVM_DOMAIN)..."
    EVM_MAILBOX=$(forge_deploy contracts/Mailbox.sol:Mailbox \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --constructor-args "$EVM_DOMAIN") || fail "Failed to deploy Mailbox"
    ok "Mailbox: $EVM_MAILBOX"

    step "Deploying MerkleTreeHook..."
    EVM_MERKLE_TREE_HOOK=$(forge_deploy contracts/hooks/MerkleTreeHook.sol:MerkleTreeHook \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --constructor-args "$EVM_MAILBOX") || fail "Failed to deploy MerkleTreeHook"
    ok "MerkleTreeHook: $EVM_MERKLE_TREE_HOOK"

    step "Deploying ValidatorAnnounce..."
    EVM_VALIDATOR_ANNOUNCE=$(forge_deploy contracts/isms/multisig/ValidatorAnnounce.sol:ValidatorAnnounce \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --constructor-args "$EVM_MAILBOX") || fail "Failed to deploy ValidatorAnnounce"
    ok "ValidatorAnnounce: $EVM_VALIDATOR_ANNOUNCE"

    step "Deploying InterchainGasPaymaster..."
    EVM_IGP=$(forge_deploy contracts/hooks/igp/InterchainGasPaymaster.sol:InterchainGasPaymaster \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY") || fail "Failed to deploy InterchainGasPaymaster"
    ok "InterchainGasPaymaster: $EVM_IGP"

    step "Initializing InterchainGasPaymaster..."
    cast send "$EVM_IGP" \
        "initialize(address,address)" \
        "$ANVIL_DEPLOYER" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize InterchainGasPaymaster"
    ok "InterchainGasPaymaster initialized"

    step "Initializing Mailbox..."
    cast send "$EVM_MAILBOX" \
        "initialize(address,address,address,address)" \
        "$ANVIL_DEPLOYER" "$EVM_ISM" "$EVM_HOOK" "$EVM_MERKLE_TREE_HOOK" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize Mailbox"
    ok "Mailbox initialized"

    step "Deploying TestRecipient..."
    EVM_RECIPIENT=$(forge_deploy contracts/test/TestRecipient.sol:TestRecipient \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY") || fail "Failed to deploy TestRecipient"
    ok "TestRecipient: $EVM_RECIPIENT"

    step "Deploying HypERC20 ($TOKEN_SYMBOL)..."
    EVM_TOKEN=$(forge_deploy contracts/token/HypERC20.sol:HypERC20 \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --constructor-args "$TOKEN_DECIMALS" 1 1 "$EVM_MAILBOX") || fail "Failed to deploy HypERC20"
    ok "HypERC20: $EVM_TOKEN"

    step "Initializing HypERC20 (supply=$INITIAL_SUPPLY)..."
    cast send "$EVM_TOKEN" \
        "initialize(uint256,string,string,address,address,address)" \
        "$INITIAL_SUPPLY" "$TOKEN_NAME" "$TOKEN_SYMBOL" \
        "$EVM_HOOK" "$EVM_ISM" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize HypERC20"
    ok "HypERC20 initialized (10 $TOKEN_SYMBOL minted)"

    step "Deploying synthetic native-DUSK token..."
    EVM_NATIVE_TOKEN=$(forge_deploy contracts/token/HypERC20.sol:HypERC20 \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --constructor-args 9 1 1 "$EVM_MAILBOX") || fail "Failed to deploy native route token"
    cast send "$EVM_NATIVE_TOKEN" \
        "initialize(uint256,string,string,address,address,address)" \
        0 "Dusk" "DUSK" "$EVM_HOOK" "$EVM_ISM" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize native route token"
    ok "Native route token: $EVM_NATIVE_TOKEN"

    step "Deploying synthetic DRC20-collateral token..."
    EVM_COLLATERAL_TOKEN=$(forge_deploy contracts/token/HypERC20.sol:HypERC20 \
        --rpc-url "$ANVIL_RPC" \
        --private-key "$ANVIL_PRIVATE_KEY" \
        --constructor-args "$TOKEN_DECIMALS" 1 1 "$EVM_MAILBOX") || fail "Failed to deploy collateral route token"
    cast send "$EVM_COLLATERAL_TOKEN" \
        "initialize(uint256,string,string,address,address,address)" \
        0 "Collateral $TOKEN_NAME" "c$TOKEN_SYMBOL" \
        "$EVM_HOOK" "$EVM_ISM" "$ANVIL_DEPLOYER" \
        --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" \
        &>/dev/null || fail "Failed to initialize collateral route token"
    ok "Collateral route token: $EVM_COLLATERAL_TOKEN"

    cat > "$EVM_DEPLOY_FILE" <<EVMJSON
{
    "ism": "$EVM_ISM",
    "hook": "$EVM_HOOK",
    "merkle_tree_hook": "$EVM_MERKLE_TREE_HOOK",
    "validator_announce": "$EVM_VALIDATOR_ANNOUNCE",
    "igp": "$EVM_IGP",
    "mailbox": "$EVM_MAILBOX",
    "recipient": "$EVM_RECIPIENT",
    "token": "$EVM_TOKEN",
    "native_token": "$EVM_NATIVE_TOKEN",
    "collateral_token": "$EVM_COLLATERAL_TOKEN"
}
EVMJSON

# ── Deploy on Dusk ───────────────────────────────────────────────────────────

header "Deploy Hyperlane on Dusk (domain=$DUSK_DOMAIN)"

DUSK_DEPLOY_FILE="/tmp/hyperlane-demo-dusk-deploy.json"

step "Deploying Hyperlane contracts on Dusk..."
DUSK_DEPLOY_CMD=(
        "$DUSK_TX" deploy-hyperlane
        --rues-url "$DUSK_RUES_URL"
        --keys "$CONSENSUS_KEYS"
        --domain "$DUSK_DOMAIN"
        --wasm-dir "$WASM_DIR"
        --deploy-warp-drc20
        --deploy-warp-native
        --warp-collateral-token warp-drc20
        --warp-name "$TOKEN_NAME"
        --warp-symbol "$TOKEN_SYMBOL"
        --warp-decimals "$TOKEN_DECIMALS"
        --default-ism "$DUSK_DEFAULT_ISM"
        --igp-domain-config "$EVM_DOMAIN:$DUSK_IGP_GAS_OVERHEAD:$DUSK_IGP_TOKEN_EXCHANGE_RATE:$DUSK_IGP_GAS_PRICE"
)
if [ "$DUSK_DEFAULT_ISM" = "messageIdMultisig" ]; then
    DUSK_DEPLOY_CMD+=(
            --multisig-validators "$MULTISIG_VALIDATORS"
            --multisig-threshold "$MULTISIG_THRESHOLD"
    )
fi

DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "${DUSK_DEPLOY_CMD[@]}" > "$DUSK_DEPLOY_FILE" || {
        # Error JSON goes to stdout (captured in deploy file) — show it
        err=$(jq -r '.error // empty' "$DUSK_DEPLOY_FILE" 2>/dev/null || true)
        fail "Dusk deployment failed${err:+: $err}"
    }
ok "Dusk contracts deployed"

# Parse deployment output
DUSK_MAILBOX=$(jq -r '.contracts.mailbox' "$DUSK_DEPLOY_FILE")
DUSK_MERKLE=$(jq -r '.contracts.merkle_tree_hook' "$DUSK_DEPLOY_FILE")
DUSK_TEST_MOCK=$(jq -r '.contracts.test_mock' "$DUSK_DEPLOY_FILE")
DUSK_ISM_MULTISIG=$(jq -r '.contracts.ism_multisig // empty' "$DUSK_DEPLOY_FILE")
DUSK_VALIDATOR_ANNOUNCE=$(jq -r '.contracts.validator_announce' "$DUSK_DEPLOY_FILE")
DUSK_IGP=$(jq -r '.contracts.igp' "$DUSK_DEPLOY_FILE")
DUSK_PROTOCOL_FEE=$(jq -r '.contracts.protocol_fee' "$DUSK_DEPLOY_FILE")
DUSK_AGGREGATION_HOOK=$(jq -r '.contracts.aggregation_hook' "$DUSK_DEPLOY_FILE")
DUSK_WARP=$(jq -r '.contracts.warp_drc20' "$DUSK_DEPLOY_FILE")
DUSK_WARP_NATIVE=$(jq -r '.contracts.warp_native' "$DUSK_DEPLOY_FILE")
DUSK_WARP_COLLATERAL=$(jq -r '.contracts.warp_drc20_collateral' "$DUSK_DEPLOY_FILE")
DUSK_TEST_RECIPIENT=$(jq -r '.contracts.test_recipient' "$DUSK_DEPLOY_FILE")

if [ "$DUSK_DEFAULT_ISM" = "messageIdMultisig" ]; then
    DUSK_DEFAULT_ISM_ID="$DUSK_ISM_MULTISIG"
else
    DUSK_DEFAULT_ISM_ID="$DUSK_TEST_MOCK"
fi

info "  Mailbox:        $DUSK_MAILBOX"
info "  MerkleTreeHook: $DUSK_MERKLE"
if [ -n "${DUSK_ISM_MULTISIG:-}" ] && [ "$DUSK_ISM_MULTISIG" != "null" ]; then
    info "  MultisigISM:    $DUSK_ISM_MULTISIG"
fi
info "  ValidatorAnnounce: $DUSK_VALIDATOR_ANNOUNCE"
info "  IGP:            $DUSK_IGP"
info "  ProtocolFee:    $DUSK_PROTOCOL_FEE"
info "  AggregationHook: $DUSK_AGGREGATION_HOOK"
info "  WarpDrc20:      $DUSK_WARP"
info "  WarpNative:     $DUSK_WARP_NATIVE"
info "  WarpCollateral: $DUSK_WARP_COLLATERAL"
info "  TestRecipient:  $DUSK_TEST_RECIPIENT"

# ── Fund Dispatch Fees ───────────────────────────────────────────────────────

header "Fund Dusk Dispatch Fees"

ensure_dispatch_credit() {
    local route="$1"
    local label="$2"
    local response current deficit

    response=$("$DUSK_TX" query \
        --rues-url "$DUSK_RUES_URL" \
        --contract "$DUSK_MAILBOX" \
        --method fee_credit \
        --return-type u64 \
        --arg-bytes32 "$route" 2>/dev/null) \
        || fail "Cannot query $label dispatch fee credit"
    current=$(jq -er '.value | tonumber' <<<"$response") \
        || fail "$label returned malformed dispatch fee credit"
    if [ "$current" -ge "$DUSK_DISPATCH_FEE_CREDIT" ]; then
        ok "$label dispatch fee credit already satisfies target ($current LUX)"
        return 0
    fi

    deficit=$((DUSK_DISPATCH_FEE_CREDIT - current))
    step "Funding $label dispatch fee deficit ($deficit LUX)..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" fund-dispatch \
        --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" \
        --mailbox "$DUSK_MAILBOX" \
        --payer "$route" \
        --amount "$deficit" \
        >/dev/null || fail "Failed to fund $label dispatch fee deficit"
    ok "$label dispatch fee credit brought to target"
}

ensure_dispatch_credit "$DUSK_WARP" "WarpDrc20"
ensure_dispatch_credit "$DUSK_WARP_NATIVE" "WarpNative"
ensure_dispatch_credit "$DUSK_WARP_COLLATERAL" "WarpCollateral"

# ── Enroll Remote Routers ────────────────────────────────────────────────────

header "Enroll Remote Routers"

EVM_TOKEN_PAD32="0x$(pad_evm_address "$EVM_TOKEN")"
DUSK_WARP_PAD32="0x${DUSK_WARP}"

step "EVM: Enrolling Dusk WarpDrc20 (domain=$DUSK_DOMAIN)..."
cast send "$EVM_TOKEN" \
    "enrollRemoteRouter(uint32,bytes32)" \
    "$DUSK_DOMAIN" "$DUSK_WARP_PAD32" \
    --rpc-url "$ANVIL_RPC" \
    --private-key "$ANVIL_PRIVATE_KEY" \
    &>/dev/null || fail "Failed to enroll remote router on EVM"
ok "EVM router enrolled"

step "Dusk: Enrolling EVM HypERC20 (domain=$EVM_DOMAIN)..."
ENROLL_OUT=$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" enroll-router \
    --rues-url "$DUSK_RUES_URL" \
    --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP" \
    --domain "$EVM_DOMAIN" \
    --router "$(pad_evm_address "$EVM_TOKEN")" 2>&1) || {
        err=$(echo "$ENROLL_OUT" | jq -r '.error // empty' 2>/dev/null || true)
        fail "Failed to enroll remote router on Dusk${err:+: $err}"
    }
ok "Dusk router enrolled"

step "EVM: Enrolling Dusk WarpNative..."
cast send "$EVM_NATIVE_TOKEN" \
    "enrollRemoteRouter(uint32,bytes32)" \
    "$DUSK_DOMAIN" "0x${DUSK_WARP_NATIVE}" \
    --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" \
    &>/dev/null || fail "Failed to enroll WarpNative on EVM"
ok "EVM native router enrolled"

step "Dusk: Enrolling EVM native token..."
DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" enroll-router \
    --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP_NATIVE" --domain "$EVM_DOMAIN" \
    --router "$(pad_evm_address "$EVM_NATIVE_TOKEN")" \
    >/dev/null || fail "Failed to enroll EVM native token on Dusk"
ok "Dusk native router enrolled"

step "EVM: Enrolling Dusk WarpCollateral..."
cast send "$EVM_COLLATERAL_TOKEN" \
    "enrollRemoteRouter(uint32,bytes32)" \
    "$DUSK_DOMAIN" "0x${DUSK_WARP_COLLATERAL}" \
    --rpc-url "$ANVIL_RPC" --private-key "$ANVIL_PRIVATE_KEY" \
    &>/dev/null || fail "Failed to enroll WarpCollateral on EVM"
ok "EVM collateral router enrolled"

step "Dusk: Enrolling EVM collateral token..."
DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" enroll-router \
    --rues-url "$DUSK_RUES_URL" --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP_COLLATERAL" --domain "$EVM_DOMAIN" \
    --router "$(pad_evm_address "$EVM_COLLATERAL_TOKEN")" \
    >/dev/null || fail "Failed to enroll EVM collateral token on Dusk"
ok "Dusk collateral router enrolled"

# ── Register BLS Account ────────────────────────────────────────────────────

header "Register BLS Account on Warp Routes"

step "Registering deployer's BLS key on WarpDrc20..."
REGISTER_RESULT=$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" register-account \
    --rues-url "$DUSK_RUES_URL" \
    --keys "$CONSENSUS_KEYS" \
    --warp-contract "$DUSK_WARP") || {
        echo "$REGISTER_RESULT" >&2
        fail "Failed to register account on WarpDrc20"
    }
DUSK_ACCOUNT_H256=$(echo "$REGISTER_RESULT" | jq -r '.account_h256')
ok "WarpDrc20 account registered: ${DUSK_ACCOUNT_H256:0:16}..."

# Register on WarpDrc20Collateral if deployed
if [ -n "$DUSK_WARP_COLLATERAL" ] && [ "$DUSK_WARP_COLLATERAL" != "null" ]; then
    step "Registering deployer's BLS key on WarpDrc20Collateral..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" register-account \
        --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" \
        --warp-contract "$DUSK_WARP_COLLATERAL" \
        >/dev/null || fail "Failed to register on WarpDrc20Collateral"
    ok "WarpDrc20Collateral account registered"
fi

# Register on WarpNative if deployed
if [ -n "$DUSK_WARP_NATIVE" ] && [ "$DUSK_WARP_NATIVE" != "null" ]; then
    step "Registering deployer's BLS key on WarpNative..."
    DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" "$DUSK_TX" register-account \
        --rues-url "$DUSK_RUES_URL" \
        --keys "$CONSENSUS_KEYS" \
        --warp-contract "$DUSK_WARP_NATIVE" \
        >/dev/null || fail "Failed to register on WarpNative"
    ok "WarpNative account registered"
fi

# ── Write Combined State File ────────────────────────────────────────────────

header "Saving Deployment State"

cat > "$BRIDGE_STATE_FILE" <<STATEJSON
{
    "evm": {
        "mailbox": "$EVM_MAILBOX",
        "token": "$EVM_TOKEN",
        "native_token": "$EVM_NATIVE_TOKEN",
        "collateral_token": "$EVM_COLLATERAL_TOKEN",
        "ism": "$EVM_ISM",
        "hook": "$EVM_HOOK",
        "merkle_tree_hook": "${EVM_MERKLE_TREE_HOOK:-}",
        "validator_announce": "${EVM_VALIDATOR_ANNOUNCE:-}",
        "igp": "${EVM_IGP:-}",
        "recipient": "$EVM_RECIPIENT"
    },
    "dusk": {
        "mailbox": "$DUSK_MAILBOX",
        "test_mock": "$DUSK_TEST_MOCK",
        "ism_multisig": "${DUSK_ISM_MULTISIG:-}",
        "default_ism": "$DUSK_DEFAULT_ISM_ID",
        "warp_drc20": "$DUSK_WARP",
        "warp_native": "$DUSK_WARP_NATIVE",
        "warp_drc20_collateral": "$DUSK_WARP_COLLATERAL",
        "merkle_tree_hook": "$DUSK_MERKLE",
        "validator_announce": "$DUSK_VALIDATOR_ANNOUNCE",
        "igp": "$DUSK_IGP",
        "igp_domain_config": {
            "domain": $EVM_DOMAIN,
            "gas_overhead": $DUSK_IGP_GAS_OVERHEAD,
            "token_exchange_rate": $DUSK_IGP_TOKEN_EXCHANGE_RATE,
            "gas_price": $DUSK_IGP_GAS_PRICE
        },
        "protocol_fee": "$DUSK_PROTOCOL_FEE",
        "aggregation_hook": "$DUSK_AGGREGATION_HOOK",
        "test_recipient": "$DUSK_TEST_RECIPIENT"
    },
    "account_h256": "$DUSK_ACCOUNT_H256",
    "evm_domain": $EVM_DOMAIN,
    "dusk_domain": $DUSK_DOMAIN,
    "evm_chain_id": "$EVM_CHAIN_ID",
    "dusk_chain_id": "$DUSK_CHAIN_ID",
    "dusk_default_ism": "$DUSK_DEFAULT_ISM",
    "token_symbol": "$TOKEN_SYMBOL",
    "initial_supply": "$INITIAL_SUPPLY",
    "deployed_at": "$(date -Iseconds)"
}
STATEJSON

ok "State saved to: $BRIDGE_STATE_FILE"

# Update explorer .env with contract IDs (if explorer dir exists)
if [ -d "$EXPLORER_DIR" ] && [ -f "$EXPLORER_DIR/.env" ]; then
    info "Updating Dusk Explorer with contract IDs..."
    if grep -q "^VITE_HYPERLANE_WARP_DRC20_ID=" "$EXPLORER_DIR/.env"; then
        sed -i "s/^VITE_HYPERLANE_WARP_DRC20_ID=.*/VITE_HYPERLANE_WARP_DRC20_ID=\"${DUSK_WARP}\"/" "$EXPLORER_DIR/.env"
        sed -i "s/^VITE_HYPERLANE_MAILBOX_ID=.*/VITE_HYPERLANE_MAILBOX_ID=\"${DUSK_MAILBOX}\"/" "$EXPLORER_DIR/.env"
    else
        echo "VITE_HYPERLANE_WARP_DRC20_ID=\"${DUSK_WARP}\"" >> "$EXPLORER_DIR/.env"
        echo "VITE_HYPERLANE_MAILBOX_ID=\"${DUSK_MAILBOX}\"" >> "$EXPLORER_DIR/.env"
    fi
    ok "Explorer .env updated (restart explorer to pick up changes)"
fi
echo ""

# ── Summary ──────────────────────────────────────────────────────────────────

echo -e "${BOLD}Deployment Complete${NC}"
echo ""
echo -e "  ${GREEN}EVM (domain=$EVM_DOMAIN):${NC}"
echo "    Mailbox:  $EVM_MAILBOX"
echo "    HypERC20: $EVM_TOKEN ($TOKEN_SYMBOL)"
echo ""
echo -e "  ${GREEN}Dusk (domain=$DUSK_DOMAIN):${NC}"
echo "    Mailbox:   $DUSK_MAILBOX"
echo "    WarpDrc20: $DUSK_WARP ($TOKEN_SYMBOL)"
echo ""
echo -e "  ${GREEN}Deployer:${NC}"
echo "    EVM:  $ANVIL_DEPLOYER"
echo "    Dusk: ${DUSK_ACCOUNT_H256:0:16}... (BLS key registered)"
echo ""
echo "  Ready to bridge! Run:"
echo "    bash demo/bridge.sh status"
echo "    bash demo/bridge.sh to-dusk 3"
echo "    bash demo/bridge.sh to-evm 1"
echo ""
