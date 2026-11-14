#!/usr/bin/env bash
# =============================================================================
# Deploy Hyperlane to Dusk testnet and connect it to an EVM testnet
# =============================================================================
#
# Deploys the Dusk contract set plus a native DUSK warp route on Dusk, and on
# an EVM chain that already runs Hyperlane core contracts (Sepolia by
# default) deploys a synthetic DUSK token and the security module that checks
# Dusk-origin messages. Then enrolls the routes with each other, funds the
# route's dispatch credit and writes agent configs.
#
# Usage:
#   cp deploy/testnet.env.example deploy/testnet.env   # then edit
#   export DUSK_CONSENSUS_PASSWORD=... EVM_DEPLOYER_KEY=0x...
#   bash deploy/testnet.sh preflight       # read-only checks
#   bash deploy/testnet.sh deploy          # deploy and wire; safe to re-run
#   bash deploy/testnet.sh agent-configs   # relayer and validator configs
#
# State goes to deploy/out/<DEPLOY_NAME>/deployment.json. Re-running `deploy`
# skips every step the state file records as done and re-checks it on chain.
# See docs/testnet.md.

set -euo pipefail
umask 077

DUSK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${TESTNET_ENV:-$DUSK_DIR/deploy/testnet.env}"
[ -f "$ENV_FILE" ] || { echo "error: $ENV_FILE not found (copy deploy/testnet.env.example)" >&2; exit 1; }
# shellcheck source=/dev/null
source "$ENV_FILE"

DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
WASM_DIR="${WASM_DIR:-$DUSK_DIR/target/contract/wasm32-unknown-unknown/release}"
OUT_DIR="$DUSK_DIR/deploy/out/$DEPLOY_NAME"
STATE="$OUT_DIR/deployment.json"
ZERO_ADDRESS=0x0000000000000000000000000000000000000000
TRANSFER_CONTRACT=0100000000000000000000000000000000000000000000000000000000000000

info() { echo "[INFO] $*" >&2; }
ok()   { echo "[OK]   $*" >&2; }
fail() { echo "[FAIL] $*" >&2; exit 1; }

require() {
    local name
    for name in "$@"; do
        [ -n "${!name:-}" ] || fail "$name is not set (see $ENV_FILE)"
    done
}

pad32() { local a="${1#0x}"; printf '000000000000000000000000%s' "${a,,}"; }
lower() { printf '%s' "${1,,}"; }

state_get() { [ -f "$STATE" ] && jq -r "$1 // empty" "$STATE" || true; }
state_set() {
    local tmp
    mkdir -p "$OUT_DIR"
    [ -f "$STATE" ] || echo '{}' > "$STATE"
    tmp="$(mktemp "$OUT_DIR/.state.XXXXXX")"
    jq "$@" "$STATE" > "$tmp" && mv "$tmp" "$STATE"
}

rues_post() { curl -sf -m 30 -X POST "${DUSK_RUES_URL%/}$1" ${2:+--data "$2"}; }
dusk_query() { "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" "$@"; }
dusk_tx() { DUSK_CONSENSUS_PASSWORD="$DUSK_CONSENSUS_PASSWORD" "$DUSK_TX" "$@"; }
evm_call() { cast call "$@" --rpc-url "$EVM_RPC"; }
evm_send() { cast send "$@" --rpc-url "$EVM_RPC" --private-key "$EVM_DEPLOYER_KEY" >/dev/null; }

sorted_addresses() {
    tr ',' '\n' <<<"$1" | tr '[:upper:]' '[:lower:]' | sed 's/^ *//; s/ *$//' | grep -v '^$' | sort -u | paste -sd, -
}

# ── Preflight ────────────────────────────────────────────────────────────────

preflight_dusk() {
    require DUSK_RUES_URL DUSK_EXPECTED_CHAIN_ID DUSK_DOMAIN
    local info height
    info="$(rues_post /on/node/info)" || fail "Dusk node at $DUSK_RUES_URL is unreachable"
    height="$(rues_post /on/graphql/query 'query { block(height: -1) { header { height } } }' \
        | jq -er '.block.header.height')" || fail "Dusk node did not report a block height"
    [ "$(jq -r '.chain_id' <<<"$info")" = "$DUSK_EXPECTED_CHAIN_ID" ] \
        || fail "Dusk node reports chain ID $(jq -r '.chain_id' <<<"$info"), expected $DUSK_EXPECTED_CHAIN_ID"
    ok "Dusk node $(jq -r '.version_build' <<<"$info"), chain ID $DUSK_EXPECTED_CHAIN_ID, height $height"

    # Feature values are an activation height, or a list of [from, to) ranges.
    python3 - "$height" "$(jq -c '.vm_config.features' <<<"$info")" <<'PY' || exit 1
import json, sys
height = int(sys.argv[1])
features = json.loads(sys.argv[2])
def active(name):
    value = features.get(name)
    if value is None:
        return False
    if isinstance(value, int):
        return height >= value
    return any(start <= height < end for start, end in value)
errors = []
for name in ("HQ_KECCAK256", "HQ_SECP256K1_RECOVER"):
    if not active(name):
        errors.append(f"host query {name} is not active at height {height}")
for name in ("DISABLE_3RD_PARTY", "DISABLE_WASM32"):
    if active(name):
        errors.append(f"{name} is active: third-party wasm32 contracts can't be deployed")
for error in errors:
    print(f"[FAIL] {error}", file=sys.stderr)
if errors:
    sys.exit(1)
print("[OK]   keccak256 and secp256k1_recover active; third-party wasm32 deploys allowed", file=sys.stderr)
PY
    rues_post /on/graphql/query "query { finalizedEvents(contractId: \"$TRANSFER_CONTRACT\", limit: 1) { json } }" \
        | jq -e '.finalizedEvents.json.events' >/dev/null \
        || fail "Dusk node does not serve archive finalizedEvents; agents need an archive node"
    ok "Dusk node serves the archive queries agents use"
}

preflight_evm() {
    require EVM_RPC EVM_CHAIN_ID EVM_DOMAIN EVM_MAILBOX EVM_MERKLE_TREE_HOOK EVM_VALIDATOR_ANNOUNCE
    local chain_id domain
    chain_id="$(cast chain-id --rpc-url "$EVM_RPC")" || fail "EVM RPC $EVM_RPC is unreachable"
    [ "$chain_id" = "$EVM_CHAIN_ID" ] || fail "EVM RPC reports chain ID $chain_id, expected $EVM_CHAIN_ID"
    domain="$(evm_call "$EVM_MAILBOX" "localDomain()(uint32)" | awk '{print $1}')" \
        || fail "No Hyperlane Mailbox at $EVM_MAILBOX"
    [ "$domain" = "$EVM_DOMAIN" ] || fail "EVM Mailbox domain is $domain, expected $EVM_DOMAIN"
    [ "$(lower "$(evm_call "$EVM_MERKLE_TREE_HOOK" "mailbox()(address)")")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "EVM_MERKLE_TREE_HOOK does not belong to EVM_MAILBOX"
    [ "$(lower "$(evm_call "$EVM_VALIDATOR_ANNOUNCE" "mailbox()(address)")")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "EVM_VALIDATOR_ANNOUNCE does not belong to EVM_MAILBOX"
    if [ -n "${EVM_ISM_FACTORY:-}" ]; then
        [ "$(cast code "$EVM_ISM_FACTORY" --rpc-url "$EVM_RPC")" != "0x" ] \
            || fail "No contract at EVM_ISM_FACTORY $EVM_ISM_FACTORY"
    fi
    # A dispatch to Dusk must be quotable through the route's hook, or every
    # transferRemote would revert.
    local quote
    if [ "$(lower "$EVM_ROUTE_HOOK")" = "$ZERO_ADDRESS" ]; then
        quote="$(evm_call "$EVM_MAILBOX" "quoteDispatch(uint32,bytes32,bytes)(uint256)" \
            "$DUSK_DOMAIN" 0x0000000000000000000000000000000000000000000000000000000000000000 0x)" \
            || fail "The EVM Mailbox's default hook can't quote a dispatch to domain $DUSK_DOMAIN; set EVM_ROUTE_HOOK"
    else
        quote="$(evm_call "$EVM_MAILBOX" "quoteDispatch(uint32,bytes32,bytes,bytes,address)(uint256)" \
            "$DUSK_DOMAIN" 0x0000000000000000000000000000000000000000000000000000000000000000 0x 0x "$EVM_ROUTE_HOOK")" \
            || fail "EVM_ROUTE_HOOK can't quote a dispatch to domain $DUSK_DOMAIN"
    fi
    ok "EVM $EVM_CHAIN_NAME (chain $chain_id): Mailbox, Merkle hook and ValidatorAnnounce check out; dispatch to Dusk quotes ${quote%% *} wei"
}

preflight_keys() {
    require DUSK_KEYS DUSK_CONSENSUS_PASSWORD EVM_DEPLOYER_KEY DUSK_ORIGIN_VALIDATORS DUSK_ORIGIN_THRESHOLD
    [ -f "$DUSK_KEYS" ] || fail "DUSK_KEYS file $DUSK_KEYS not found"
    local deployer balance n
    deployer="$(cast wallet address --private-key "$EVM_DEPLOYER_KEY")" || fail "EVM_DEPLOYER_KEY is not a valid key"
    balance="$(cast balance "$deployer" --rpc-url "$EVM_RPC" --ether)"
    ok "EVM deployer $deployer has $balance ETH"
    n="$(tr ',' '\n' <<<"$(sorted_addresses "$DUSK_ORIGIN_VALIDATORS")" | grep -c .)"
    [ "$DUSK_ORIGIN_THRESHOLD" -ge 1 ] && [ "$DUSK_ORIGIN_THRESHOLD" -le "$n" ] \
        || fail "DUSK_ORIGIN_THRESHOLD must be between 1 and the $n Dusk-origin validators"
    n="$(tr ',' '\n' <<<"$(sorted_addresses "$EVM_ORIGIN_VALIDATORS")" | grep -c .)"
    [ "$EVM_ORIGIN_THRESHOLD" -ge 1 ] && [ "$EVM_ORIGIN_THRESHOLD" -le "$n" ] \
        || fail "EVM_ORIGIN_THRESHOLD must be between 1 and the $n EVM-origin validators"
    [ -x "$DUSK_TX" ] || fail "dusk-tx not built at $DUSK_TX (run: make dusk-tx)"
    [ -f "$WASM_DIR/hyperlane_dusk_mailbox.wasm" ] || fail "contract WASMs not built (run: make all)"
}

cmd_preflight() {
    for tool in jq cast forge curl python3; do
        command -v "$tool" >/dev/null || fail "$tool is required"
    done
    preflight_dusk
    preflight_evm
    preflight_keys
}

# ── Deploy steps ─────────────────────────────────────────────────────────────

deploy_dusk() {
    if [ -n "$(state_get '.dusk.mailbox')" ]; then
        ok "Dusk contracts already deployed (Mailbox $(state_get '.dusk.mailbox'))"
        return
    fi
    info "Deploying the Dusk contracts (this takes a few minutes)..."
    local out="$OUT_DIR/dusk-deploy.json"
    mkdir -p "$OUT_DIR"
    dusk_tx deploy-hyperlane \
        --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
        --domain "$DUSK_DOMAIN" --wasm-dir "$WASM_DIR" \
        --gas-limit "$DUSK_GAS_LIMIT" --gas-price "$DUSK_GAS_PRICE" \
        --deploy-warp-native \
        --default-ism messageIdMultisig \
        --multisig-validators "$(sorted_addresses "$EVM_ORIGIN_VALIDATORS")" \
        --multisig-threshold "$EVM_ORIGIN_THRESHOLD" \
        --igp-domain-config "$EVM_DOMAIN:$DUSK_IGP_GAS_OVERHEAD:$DUSK_IGP_TOKEN_EXCHANGE_RATE:$DUSK_IGP_GAS_PRICE" \
        > "$out" || fail "Dusk deployment failed: $(jq -r '.error // empty' "$out" 2>/dev/null)"
    state_set --slurpfile d "$out" --arg domain "$DUSK_DOMAIN" \
        '.dusk = $d[0].contracts | .dusk_domain = ($domain | tonumber)'
    ok "Dusk Mailbox $(state_get '.dusk.mailbox'), WarpNative $(state_get '.dusk.warp_native')"
}

deploy_evm_ism() {
    if [ -n "$(state_get '.evm.ism')" ]; then
        ok "EVM security module already deployed ($(state_get '.evm.ism'))"
        return
    fi
    local validators ism
    validators="$(sorted_addresses "$DUSK_ORIGIN_VALIDATORS")"
    if [ -n "${EVM_ISM_FACTORY:-}" ]; then
        ism="$(evm_call "$EVM_ISM_FACTORY" "getAddress(address[],uint8)(address)" "[$validators]" "$DUSK_ORIGIN_THRESHOLD")"
        if [ "$(cast code "$ism" --rpc-url "$EVM_RPC")" = "0x" ]; then
            info "Deploying MessageIdMultisigIsm ($DUSK_ORIGIN_THRESHOLD of [$validators]) through the factory..."
            evm_send "$EVM_ISM_FACTORY" "deploy(address[],uint8)" "[$validators]" "$DUSK_ORIGIN_THRESHOLD"
        fi
    else
        info "Deploying StorageMessageIdMultisigIsm ($DUSK_ORIGIN_THRESHOLD of [$validators])..."
        ism="$(cd "$SOLIDITY_DIR" && forge create contracts/isms/multisig/StorageMultisigIsm.sol:StorageMessageIdMultisigIsm \
            --broadcast --json --rpc-url "$EVM_RPC" --private-key "$EVM_DEPLOYER_KEY" \
            --constructor-args "[$validators]" "$DUSK_ORIGIN_THRESHOLD" | jq -r '.deployedTo')"
    fi
    [ "$(cast code "$ism" --rpc-url "$EVM_RPC")" != "0x" ] || fail "EVM security module deployment failed"
    state_set --arg ism "$ism" --arg v "$validators" --argjson t "$DUSK_ORIGIN_THRESHOLD" \
        '.evm.ism = $ism | .dusk_origin_validators = ($v | split(",")) | .dusk_origin_threshold = $t'
    ok "EVM security module $ism"
}

deploy_evm_token() {
    if [ -n "$(state_get '.evm.token')" ]; then
        ok "EVM DUSK token already deployed ($(state_get '.evm.token'))"
        return
    fi
    local deployer from_block token
    deployer="$(cast wallet address --private-key "$EVM_DEPLOYER_KEY")"
    from_block="$(cast block-number --rpc-url "$EVM_RPC")"
    info "Deploying the synthetic DUSK token on $EVM_CHAIN_NAME (9 decimals, matching LUX)..."
    token="$(cd "$SOLIDITY_DIR" && forge create contracts/token/HypERC20.sol:HypERC20 \
        --broadcast --json --rpc-url "$EVM_RPC" --private-key "$EVM_DEPLOYER_KEY" \
        --constructor-args 9 1 1 "$EVM_MAILBOX" | jq -r '.deployedTo')"
    [ -n "$token" ] && [ "$token" != "null" ] || fail "HypERC20 deployment failed"
    evm_send "$token" "initialize(uint256,string,string,address,address,address)" \
        0 "Dusk" "DUSK" "$EVM_ROUTE_HOOK" "$(state_get '.evm.ism')" "$deployer"
    state_set --arg t "$token" --arg o "$deployer" --argjson b "$from_block" \
        '.evm.token = $t | .evm.owner = $o | .evm.index_from = $b'
    ok "EVM DUSK token $token"
}

enroll_routers() {
    local token route enrolled
    token="$(state_get '.evm.token')"
    route="$(state_get '.dusk.warp_native')"
    enrolled="$(evm_call "$token" "routers(uint32)(bytes32)" "$DUSK_DOMAIN")"
    if [ "$(lower "$enrolled")" != "0x$route" ]; then
        info "Enrolling the Dusk route on $EVM_CHAIN_NAME..."
        evm_send "$token" "enrollRemoteRouter(uint32,bytes32)" "$DUSK_DOMAIN" "0x$route"
    fi
    [ "$(lower "$(evm_call "$token" "routers(uint32)(bytes32)" "$DUSK_DOMAIN")")" = "0x$route" ] \
        || fail "EVM token does not have the Dusk route enrolled"
    enrolled="$(dusk_query --contract "$route" --method enrolled_router --return-type bytes32 \
        --arg-u32 "$EVM_DOMAIN" 2>/dev/null | jq -r '.value // empty')"
    if [ "$(lower "$enrolled")" != "$(pad32 "$token")" ]; then
        info "Enrolling the $EVM_CHAIN_NAME token on the Dusk route..."
        dusk_tx enroll-router --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
            --warp-contract "$route" --domain "$EVM_DOMAIN" --router "$(pad32 "$token")" >/dev/null \
            || fail "Dusk router enrollment failed"
    fi
    enrolled="$(dusk_query --contract "$route" --method enrolled_router --return-type bytes32 \
        --arg-u32 "$EVM_DOMAIN" | jq -r '.value')"
    [ "$(lower "$enrolled")" = "$(pad32 "$token")" ] || fail "Dusk route does not have the EVM token enrolled"
    ok "Routes enrolled with each other"
}

fund_dispatch_credit() {
    local mailbox route credit
    mailbox="$(state_get '.dusk.mailbox')"
    route="$(state_get '.dusk.warp_native')"
    credit="$(dusk_query --contract "$mailbox" --method fee_credit --return-type u64 \
        --arg-bytes32 "$route" | jq -r '.value')"
    if [ "$credit" -lt "$DUSK_DISPATCH_CREDIT" ]; then
        info "Funding $((DUSK_DISPATCH_CREDIT - credit)) LUX of dispatch credit for the native route..."
        dusk_tx fund-dispatch --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
            --mailbox "$mailbox" --payer "$route" --amount "$((DUSK_DISPATCH_CREDIT - credit))" >/dev/null \
            || fail "Funding dispatch credit failed"
        credit="$(dusk_query --contract "$mailbox" --method fee_credit --return-type u64 \
            --arg-bytes32 "$route" | jq -r '.value')"
    fi
    ok "Native route dispatch credit: $credit LUX"
}

cmd_deploy() {
    cmd_preflight
    state_set --arg n "$EVM_CHAIN_NAME" --argjson d "$EVM_DOMAIN" --arg m "$EVM_MAILBOX" \
        --arg h "$EVM_MERKLE_TREE_HOOK" --arg va "$EVM_VALIDATOR_ANNOUNCE" --arg igp "${EVM_IGP:-}" \
        '.evm_chain = $n | .evm_domain = $d | .evm.mailbox = $m | .evm.merkle_tree_hook = $h
         | .evm.validator_announce = $va | .evm.igp = $igp'
    deploy_dusk
    deploy_evm_ism
    deploy_evm_token
    enroll_routers
    fund_dispatch_credit
    ok "Deployment complete: $STATE"
    info "Next: bash deploy/testnet.sh agent-configs"
}

# ── Agent configs ────────────────────────────────────────────────────────────

dusk_chain_json() {
    local cursor_dir="$1"
    jq -n --arg url "$DUSK_RUES_URL" --argjson domain "$DUSK_DOMAIN" \
        --argjson chain "$DUSK_EXPECTED_CHAIN_ID" --arg cursors "$cursor_dir" \
        --argjson gl "$DUSK_AGENT_GAS_LIMIT" --argjson gp "$DUSK_AGENT_GAS_PRICE" \
        --slurpfile s "$STATE" '{
          name: "dusk", protocol: "dusk", domainId: $domain, chainId: $chain,
          rpcUrls: [{http: $url}], eventCursorDir: $cursors,
          gasLimit: $gl, gasPrice: $gp,
          mailbox: ("0x" + $s[0].dusk.mailbox),
          interchainGasPaymaster: ("0x" + $s[0].dusk.igp),
          validatorAnnounce: ("0x" + $s[0].dusk.validator_announce),
          merkleTreeHook: ("0x" + $s[0].dusk.merkle_tree_hook),
          submitter: "Classic",
          signer: {type: "duskKey", keyFile: "/run/secrets/dusk-signer.key"}
        }'
}

cmd_agent_configs() {
    [ -f "$STATE" ] && [ -n "$(state_get '.evm.token')" ] || fail "Run deploy first"
    local evm_chain token route
    token="$(state_get '.evm.token')"
    route="$(state_get '.dusk.warp_native')"
    evm_chain="$(jq -n --arg name "$EVM_CHAIN_NAME" --argjson domain "$EVM_DOMAIN" \
        --argjson chain "$EVM_CHAIN_ID" --arg rpc "$EVM_RPC" \
        --argjson reorg "$EVM_REORG_PERIOD" --argjson bt "$EVM_BLOCK_TIME" \
        --slurpfile s "$STATE" '{
          name: $name, protocol: "ethereum", domainId: $domain, chainId: $chain,
          rpcUrls: [{http: $rpc}],
          blocks: {reorgPeriod: $reorg, estimateBlockTime: $bt},
          index: {from: $s[0].evm.index_from, chunk: 1999},
          mailbox: $s[0].evm.mailbox,
          interchainGasPaymaster: $s[0].evm.igp,
          validatorAnnounce: $s[0].evm.validator_announce,
          merkleTreeHook: $s[0].evm.merkle_tree_hook
        }')"

    # The relayer only relays this deployment's route, in both directions, and
    # does not require gas payments (the IGPs are not priced for production).
    jq -n --arg evm "$EVM_CHAIN_NAME" --argjson evm_chain "$evm_chain" \
        --argjson dusk_chain "$(dusk_chain_json /var/lib/hyperlane/relayer-dusk-events)" \
        --argjson ed "$EVM_DOMAIN" --argjson dd "$DUSK_DOMAIN" \
        --arg token "0x$(pad32 "$token")" --arg route "0x$route" '{
          db: "/var/lib/hyperlane/relayer-db",
          relayChains: ("dusk," + $evm),
          gasPaymentEnforcement: [{type: "none"}],
          whitelist: [
            {originDomain: $ed, senderAddress: $token, destinationDomain: $dd, recipientAddress: $route},
            {originDomain: $dd, senderAddress: $route, destinationDomain: $ed, recipientAddress: $token}
          ],
          chains: {($evm): $evm_chain, dusk: $dusk_chain},
          metricsPort: 9090
        }' > "$OUT_DIR/relayer.json"

    # One Dusk-origin validator. Each validator operator copies this, sets its
    # own signing key and checkpoint bucket, and runs it on its own host.
    jq -n --argjson dusk_chain "$(dusk_chain_json /var/lib/hyperlane/validator-dusk-events)" '{
          db: "/var/lib/hyperlane/validator-db",
          originChainName: "dusk",
          interval: 5,
          checkpointSyncer: {type: "s3", bucket: "<bucket>", region: "<region>"},
          chains: {dusk: $dusk_chain},
          metricsPort: 9091
        }' > "$OUT_DIR/validator-dusk.json"

    ok "Wrote $OUT_DIR/relayer.json and $OUT_DIR/validator-dusk.json"
    cat >&2 <<EOF
Secrets are not in these files. Provide them when starting the agents:
  relayer:   HYP_CHAINS_$(tr '[:lower:]' '[:upper:]' <<<"$EVM_CHAIN_NAME")_SIGNER_KEY=0x...  (funded on $EVM_CHAIN_NAME)
             a Dusk key file at /run/secrets/dusk-signer.key (funded on Dusk testnet)
  validator: HYP_VALIDATOR_KEY=0x...  (one of DUSK_ORIGIN_VALIDATORS)
             a Dusk key file at /run/secrets/dusk-signer.key (pays for its announcement)
             AWS credentials for the checkpoint bucket
Both need DUSK_TX_BIN pointing at dusk-tx. See docs/testnet.md.
EOF
}

case "${1:-}" in
    preflight) cmd_preflight ;;
    deploy) cmd_deploy ;;
    agent-configs) cmd_agent_configs ;;
    *) fail "usage: bash deploy/testnet.sh preflight|deploy|agent-configs" ;;
esac
