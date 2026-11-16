#!/usr/bin/env bash
# =============================================================================
# Deploy Hyperlane to Dusk testnet and connect it to an EVM testnet
# =============================================================================
#
# Deploys the Dusk contract set plus a native DUSK warp route on Dusk, and on
# an EVM chain that already runs Hyperlane core contracts (Sepolia by
# default) deploys a synthetic DUSK token, the security module that checks
# Dusk-origin messages, and an InterchainGasPaymaster that charges for
# delivery to Dusk. Then enrolls the routes with each other and writes agent
# configs.
#
# Usage:
#   cp deploy/testnet.env.example deploy/testnet.env   # then edit
#   export DUSK_CONSENSUS_PASSWORD=... EVM_DEPLOYER_KEY=0x...
#   bash deploy/testnet.sh preflight       # read-only checks
#   bash deploy/testnet.sh deploy          # deploy and wire; safe to re-run
#   bash deploy/testnet.sh agent-configs   # relayer and validator configs
#
# Relaying fees (docs/costs.md#fee-coverage, docs/operations.md):
#   bash deploy/testnet.sh fees [--json]   # what the settings charge; no network
#   bash deploy/testnet.sh dusk-igp        # the Dusk IGP's pricing on chain against
#   bash deploy/testnet.sh evm-igp         #   the settings, and the EVM IGP's; both
#                                          #   dry runs: add --apply to send it
#   bash deploy/testnet.sh revenue         # fees waiting for their beneficiaries
#   bash deploy/testnet.sh claim           # claim them on both chains
#
# State goes to deploy/out/<DEPLOY_NAME>/deployment.json. Re-running `deploy`
# skips every step the state file records as done and re-checks it on chain.
# Once the Dusk contracts are deployed, every command checks that the ISM the
# route uses on chain is a MessageIdMultisig ISM with EVM_ORIGIN_VALIDATORS and
# EVM_ORIGIN_THRESHOLD, and that both IGPs charge what the settings say. See
# docs/testnet.md.

set -euo pipefail
umask 077

DUSK_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${TESTNET_ENV:-$DUSK_DIR/deploy/testnet.env}"
[ -f "$ENV_FILE" ] || { echo "error: $ENV_FILE not found (copy deploy/testnet.env.example)" >&2; exit 1; }
# shellcheck source=/dev/null
source "$ENV_FILE"

DUSK_TX="${DUSK_TX:-$DUSK_DIR/target/release/dusk-tx}"
AGENT_STATE_DIR="${AGENT_STATE_DIR:-/var/lib/hyperlane}"
DUSK_SIGNER_KEY_FILE="${DUSK_SIGNER_KEY_FILE:-/run/secrets/dusk-signer.key}"
DUSK_RELAYER_GAS_LIMIT="${DUSK_RELAYER_GAS_LIMIT:-5000000}"
DUSK_RELAYER_MAX_BATCH_SIZE="${DUSK_RELAYER_MAX_BATCH_SIZE:-16}"
DUSK_MULTICALL="${DUSK_MULTICALL-93de287b7d349d678a0038fe7772e8960e1a84a1b5da2d0b1f62f532616ce1d4}"
WASM_DIR="${WASM_DIR:-$DUSK_DIR/target/contract/wasm32-unknown-unknown/release}"
OUT_DIR="$DUSK_DIR/deploy/out/$DEPLOY_NAME"
STATE="$OUT_DIR/deployment.json"
ZERO_ADDRESS=0x0000000000000000000000000000000000000000
ZERO_ID=0000000000000000000000000000000000000000000000000000000000000000
TRANSFER_CONTRACT=0100000000000000000000000000000000000000000000000000000000000000
# The state_version of each core contract this checkout builds. Older ones
# keep storage anyone can fill, and this checkout's agents don't read them
# (docs/storage.md); an IGP before 4 can't be topped up (pay_for_gas), so a
# message that paid it too little is never relayed (docs/operations.md).
CORE_STATE_VERSIONS="mailbox=5 merkle_tree_hook=2 igp=4 validator_announce=2"
# The state_version of each warp route this checkout builds. Older routes may
# pay a sponsored escrow claim to the key as given, which a forged twin of the
# recipient's key can exploit (docs/security.md, CRITICAL-1), and don't
# register a recipient that claims its own escrow. WarpDrc20 7 keys balances
# by account ID (docs/storage.md). Older collateral routes can be re-entered
# from their token (MEDIUM-13).
ROUTE_STATE_VERSIONS="warp_native=4 warp_drc20=7 warp_drc20_collateral=6"
# Module types the Dusk ISMs report (Hyperlane's IsmType).
MESSAGE_ID_MULTISIG_ISM=5
TEST_MOCK_ISM=6
# Hook types the EVM hooks report (Hyperlane's IPostDispatchHook.HookTypes).
HOOK_ROUTING=1
HOOK_AGGREGATION=2
HOOK_MERKLE_TREE=3
HOOK_IGP=4
HOOK_FALLBACK_ROUTING=5
HOOK_PAUSABLE=7
HOOK_PROTOCOL_FEE=8
# How deep route_hook_plan follows hooks inside hooks.
MAX_HOOK_DEPTH=8
# The gas the Dusk IGP charges a message without hook metadata for, as the
# warp routes send none (types/src/metadata.rs, DEFAULT_GAS_USAGE).
DUSK_IGP_DEFAULT_GAS=50000
# What the EVM IGP's and the token's creation receipts must show: their
# ERC1967Proxy's Upgraded(address) and the implementation's
# Initialized(uint8). IMPLEMENTATION_SLOT is EIP-1967's.
UPGRADED_TOPIC=0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b
INITIALIZED_TOPIC=0x7f26b83ff96e1f2b6a682f133852f6798a09c465da95921460cefb3847402498
IMPLEMENTATION_SLOT=0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc

info() { echo "[INFO] $*" >&2; }
ok()   { echo "[OK]   $*" >&2; }
warn() { echo "[WARN] $*" >&2; }
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
# The chain ID transactions are signed for, from the transfer contract.
# /on/node/info reports null on nodes started without a network ID.
dusk_chain_id() {
    local id
    id="$(curl -sf -m 30 -X POST -H 'Content-Type: application/octet-stream' \
        "${DUSK_RUES_URL%/}/on/contracts:$TRANSFER_CONTRACT/chain_id" | od -An -tu1 | tr -d ' \n')"
    [[ "$id" =~ ^[0-9]+$ ]] && printf '%s' "$id"
}
dusk_query() { "$DUSK_TX" query --rues-url "$DUSK_RUES_URL" "$@"; }
# The value a Dusk contract query returns, as a string (JSON for a list or
# object). Stops the script if the query fails or returns nothing: a check
# that can't read the chain must not pass. `fail` in a command substitution
# only ends the substitution, so call it as `x="$(dusk_value ...)" || exit 1`.
dusk_value() {
    local label="$1" out value
    shift
    out="$(dusk_query "$@" 2>/dev/null)" \
        || fail "Can't read $label: $(jq -r '.error // "the query failed"' <<<"$out" 2>/dev/null)"
    value="$(jq -er '.value | select(. != null) | tostring' <<<"$out" 2>/dev/null)" \
        || fail "Can't read $label: the query returned no value"
    printf '%s' "$value"
}
dusk_tx() { DUSK_CONSENSUS_PASSWORD="$DUSK_CONSENSUS_PASSWORD" "$DUSK_TX" "$@"; }
evm_call() { cast call "$@" --rpc-url "$EVM_RPC"; }
evm_send() { cast send "$@" --rpc-url "$EVM_RPC" --private-key "$EVM_DEPLOYER_KEY" >/dev/null; }
# A number `cast call` returns, without the scientific form it adds.
evm_number() { local out; out="$(evm_call "$@")" || return 1; awk 'NR == 1 {print $1}' <<<"$out"; }
# Whether there is a contract at an address. A failed lookup stops the script
# rather than count as either.
evm_has_code() {
    local code
    code="$(cast code "$1" --rpc-url "$EVM_RPC")" || fail "Can't read the code at $1 on $EVM_CHAIN_NAME"
    [ "$code" != "0x" ]
}
evm_deployer() { cast wallet address --private-key "$EVM_DEPLOYER_KEY"; }
# Who deployed the EVM contracts: EVM_DEPLOYER_KEY's address, or, for a
# command run without the key, the deployer deploy recorded.
evm_creator() {
    if [ -n "${EVM_DEPLOYER_KEY:-}" ]; then
        evm_deployer
    elif [ -n "$(state_get '.evm.deployer')" ]; then
        state_get '.evm.deployer'
    else
        echo "[FAIL] Set EVM_DEPLOYER_KEY: $STATE doesn't record who deployed the EVM contracts" >&2
        return 1
    fi
}
# Deploy a contract from the Hyperlane Solidity sources; prints its address.
forge_create() {
    local contract="$1" address
    shift
    address="$(cd "$SOLIDITY_DIR" && forge create "$contract" --broadcast --json --rpc-url "$EVM_RPC" \
        --private-key "$EVM_DEPLOYER_KEY" "$@" | jq -r '.deployedTo')" || return 1
    [[ "$address" =~ ^0x[0-9a-fA-F]{40}$ ]] || return 1
    printf '%s' "$address"
}
# ERC1967Proxy's creation code, from the OpenZeppelin release Hyperlane's
# Solidity sources build with.
evm_proxy_creation_code() {
    local oz
    oz="$(cd "$SOLIDITY_DIR" && forge remappings 2>/dev/null | sed -n 's|^@openzeppelin/contracts/=||p' | head -1)"
    [ -n "$oz" ] || return 1
    (cd "$SOLIDITY_DIR" && forge inspect "${oz%/}/proxy/ERC1967/ERC1967Proxy.sol:ERC1967Proxy" bytecode 2>/dev/null)
}

# The EVM IGP and the route's token are initialized in the transaction that
# creates them. Each is an ERC1967Proxy whose constructor calls the
# implementation's initialize, which sets the owner and, for the IGP, the
# beneficiary. Deployed bare and initialized in a second transaction, as
# before, anyone could send initialize first: an IGP initialized with the
# deployer as owner and someone else as beneficiary passed every check, and
# one whose first owner planted a free standing quote overrode the oracle
# for good (docs/security.md, HIGH-8). The proxies have no admin, and
# neither implementation can upgrade itself, so they stay what they are.
#
# deploy_evm_proxied <key> <label> <implementation> <initialize calldata>
#     [forge create arguments for the implementation...]
# deploys the implementation, unless .evm.<key>_implementation records one,
# then the proxy, and records .evm.<key> and .evm.<key>_creation_tx.
deploy_evm_proxied() {
    local key="$1" label="$2" contract="$3" init="$4" impl code args receipt address tx
    shift 4
    [[ "$init" =~ ^0x[0-9a-fA-F]{8,}$ ]] || fail "No initialize call for the $label"
    impl="$(state_get ".evm.${key}_implementation")"
    if [ -z "$impl" ]; then
        info "Deploying the $label's implementation on $EVM_CHAIN_NAME..."
        impl="$(forge_create "$contract" "$@")" || fail "$label implementation deployment failed"
        state_set --arg k "${key}_implementation" --arg i "$impl" '.evm[$k] = $i'
    fi
    evm_has_code "$impl" || fail "No $label implementation at $impl, which $STATE records"
    code="$(evm_proxy_creation_code)" && [[ "$code" =~ ^0x[0-9a-fA-F]+$ ]] \
        || fail "Can't get ERC1967Proxy's creation code from $SOLIDITY_DIR"
    args="$(cast abi-encode "constructor(address,bytes)" "$impl" "$init")" \
        || fail "Can't encode the $label proxy's constructor arguments"
    info "Deploying the $label behind an ERC1967Proxy that initializes it in the same transaction..."
    receipt="$(cast send --rpc-url "$EVM_RPC" --private-key "$EVM_DEPLOYER_KEY" --json --create "$code${args#0x}")" \
        || fail "$label deployment failed"
    address="$(jq -r '.contractAddress // empty' <<<"$receipt")"
    tx="$(jq -r '.transactionHash // empty' <<<"$receipt")"
    [[ "$address" =~ ^0x[0-9a-fA-F]{40}$ ]] && [[ "$tx" =~ ^0x[0-9a-fA-F]{64}$ ]] \
        || fail "$label deployment returned no contract address and transaction hash"
    state_set --arg k "$key" --arg a "$address" --arg t "$tx" --arg d "$(evm_deployer)" \
        '.evm[$k] = $a | .evm[$k + "_creation_tx"] = $t | .evm.deployer = $d'
    check_evm_creation "$key" "$label"
}

# Prove from the receipt of the transaction that created the recorded
# contract that the deployer sent it, that it created the contract as a proxy
# of the recorded implementation, and that the contract was initialized in
# it, so nobody could have initialized it before. A contract recorded without
# that transaction was deployed by an older version of this script.
check_evm_creation() {
    local key="$1" label="$2" address tx impl creator receipt slot
    address="$(state_get ".evm.$key")"
    [ -n "$address" ] || return 0
    tx="$(state_get ".evm.${key}_creation_tx")" impl="$(state_get ".evm.${key}_implementation")"
    [ -n "$tx" ] && [ -n "$impl" ] \
        || fail "$STATE records the EVM $label $address without the transaction that created and initialized it. An older deploy/testnet.sh initialized it in a second transaction, which anyone could send first, and so choose its owner, its beneficiary or its pricing (docs/security.md, HIGH-8). Move $OUT_DIR aside and deploy again."
    creator="$(evm_creator)" || exit 1
    receipt="$(cast receipt "$tx" --json --rpc-url "$EVM_RPC")" \
        || fail "Can't read the receipt of $tx, which created the EVM $label $address"
    jq -e --arg a "$(lower "$address")" --arg c "$(lower "$creator")" --arg i "$(pad32 "$impl")" \
        --arg upgraded "$UPGRADED_TOPIC" --arg initialized "$INITIALIZED_TOPIC" '
        def mine: (.address | ascii_downcase) == $a;
        (.status | tostring | test("^(0x)?1$"))
        and ((.contractAddress // "") | ascii_downcase) == $a
        and ((.from // "") | ascii_downcase) == $c
        and any(.logs[]; mine and .topics[0] == $upgraded
                and ((.topics[1] // "") | ascii_downcase | ltrimstr("0x")) == $i)
        and any(.logs[]; mine and .topics[0] == $initialized)' <<<"$receipt" >/dev/null \
        || fail "The EVM $label $address wasn't initialized in the transaction that created it: $tx isn't the EVM deployer $creator creating it as a proxy of $impl and initializing it. Whoever initialized it chose its owner (docs/security.md, HIGH-8). Move $OUT_DIR aside and deploy again."
    slot="$(cast storage "$address" "$IMPLEMENTATION_SLOT" --rpc-url "$EVM_RPC")" \
        || fail "Can't read the implementation of the EVM $label $address"
    [ "$(lower "0x${slot: -40}")" = "$(lower "$impl")" ] \
        || fail "The EVM $label $address runs the implementation 0x${slot: -40}, not $impl, which it was created with"
}

# The EVM contracts deploy initializes: our IGP, once the state records its
# oracle (before, .evm.igp was Hyperlane's), and the token.
check_evm_creations() {
    [ -z "$(state_get '.evm.storage_gas_oracle')" ] || check_evm_creation igp IGP
    check_evm_creation token "DUSK token"
}

sorted_addresses() {
    tr ',' '\n' <<<"$1" | tr '[:upper:]' '[:lower:]' | sed 's/^ *//; s/ *$//' | grep -v '^$' | sort -u | paste -sd, -
}
count_addresses() { tr ',' '\n' <<<"$(sorted_addresses "$1")" | grep -c . || true; }

# ── Relaying fees ────────────────────────────────────────────────────────────
#
# Both IGPs price delivery the same way: a message pays for a gas amount on
# the destination (the gas limit in its hook metadata plus the destination's
# overhead) at
#
#   payment = gas x gas price x token exchange rate / 1e10
#
# in the origin chain's smallest unit. The gas price is in the destination's
# smallest unit, so the exchange rate converts one into the other, scaled by
# 1e10 and marked up by OPERATOR_MARGIN_BPS (neither IGP has a markup field):
#
#   Dusk IGP, LUX per wei: 1e10 x ETH_DUSK_PRICE_RATIO x 1e9 / 1e18 x (1 + margin)
#                        = 10 x ETH_DUSK_PRICE_RATIO x (1 + margin)
#   EVM IGP, wei per LUX:  1e10 x 1e18 / (ETH_DUSK_PRICE_RATIO x 1e9) x (1 + margin)
#                        = 1e19 / ETH_DUSK_PRICE_RATIO x (1 + margin)
#
# rounded up. docs/costs.md#fee-coverage has the derivation and what the
# defaults charge.

# Settings older versions of the env file had, which these replace.
check_fee_settings() {
    local name
    for name in DUSK_IGP_TOKEN_EXCHANGE_RATE DUSK_IGP_GAS_PRICE EVM_ROUTE_HOOK; do
        [ -z "${!name+x}" ] || fail "$name is no longer read: the IGPs' pricing comes from ETH_DUSK_PRICE_RATIO, EVM_GAS_PRICE_WEI and OPERATOR_MARGIN_BPS, and deploy sets the EVM route's hook. Remove it from $ENV_FILE (see deploy/testnet.env.example)."
    done
    [ -z "${EVM_IGP:-}" ] \
        || warn "EVM_IGP is no longer read: the relayer reads gas payments from the IGP deploy sets up on $EVM_CHAIN_NAME."
    require ETH_DUSK_PRICE_RATIO EVM_GAS_PRICE_WEI OPERATOR_MARGIN_BPS DUSK_IGP_GAS_OVERHEAD \
        EVM_IGP_GAS_OVERHEAD DUSK_AGENT_GAS_PRICE EVM_ORIGIN_THRESHOLD
}

# Derive both IGPs' pricing and the relayer's requirements from the settings.
# Prints JSON (`fees --json` shows it) or, with `text`, a summary. Exits 1,
# after printing, if a setting is invalid or a gas amount too low.
compute_fees() {
    python3 - "$1" \
        ETH_DUSK_PRICE_RATIO="$ETH_DUSK_PRICE_RATIO" OPERATOR_MARGIN_BPS="$OPERATOR_MARGIN_BPS" \
        EVM_GAS_PRICE_WEI="$EVM_GAS_PRICE_WEI" DUSK_AGENT_GAS_PRICE="$DUSK_AGENT_GAS_PRICE" \
        DUSK_IGP_GAS_OVERHEAD="$DUSK_IGP_GAS_OVERHEAD" EVM_IGP_GAS_OVERHEAD="$EVM_IGP_GAS_OVERHEAD" \
        DUSK_RELAYER_GAS_LIMIT="$DUSK_RELAYER_GAS_LIMIT" EVM_ORIGIN_THRESHOLD="$EVM_ORIGIN_THRESHOLD" \
        DUSK_ORIGIN_THRESHOLD="${DUSK_ORIGIN_THRESHOLD:-}" \
        DUSK_ORIGIN_VALIDATOR_COUNT="$(count_addresses "${DUSK_ORIGIN_VALIDATORS:-}")" \
        EVM_DOMAIN="$EVM_DOMAIN" DUSK_DOMAIN="$DUSK_DOMAIN" EVM_CHAIN_NAME="$EVM_CHAIN_NAME" <<'PY'
import json, re, sys
from fractions import Fraction
from math import ceil

mode = sys.argv[1]
s = dict(arg.split("=", 1) for arg in sys.argv[2:])
errors = []
U64, U96, U128 = 2**64 - 1, 2**96 - 1, 2**128 - 1
SCALE = 10**10                   # both IGPs' TOKEN_EXCHANGE_RATE_SCALE
LUX, WEI = 10**9, 10**18         # per DUSK, per ETH
DEFAULT_GAS = 50_000             # the Dusk IGP's gas for a message without hook metadata
IGP_MAX_GAS_LIMIT = 10**9        # the largest gas limit the Dusk IGP prices
# A route transfer's delivery on the EVM chain, Mailbox.process with an m-of-n
# MessageId multisig ISM and HypERC20.handle: at most this. The local
# rehearsal measured 140,569 gas at 2 of 3 (a StorageMessageIdMultisigIsm,
# minting to a new holder); each signature and validator adds at most this.
EVM_DELIVERY_BASE, EVM_DELIVERY_PER_SIGNATURE, EVM_DELIVERY_PER_VALIDATOR = 125_000, 7_500, 2_500
DUSK_PROCESS_BASE, DUSK_PROCESS_PER_SIGNATURE = 1_950_000, 220_000

def whole(name, low, high):
    raw = s.get(name, "")
    if not re.fullmatch(r"[0-9]+", raw):
        errors.append(f"{name} must be a whole number, not '{raw}'")
        return None
    if not low <= int(raw) <= high:
        errors.append(f"{name} must be between {low:,} and {high:,}, not {int(raw):,}")
        return None
    return int(raw)

raw_ratio = s["ETH_DUSK_PRICE_RATIO"]
ratio = Fraction(raw_ratio) if re.fullmatch(r"[0-9]+(\.[0-9]+)?", raw_ratio) else Fraction(0)
if ratio <= 0:
    errors.append(f"ETH_DUSK_PRICE_RATIO must be a positive number of DUSK per ETH, not '{raw_ratio}'")
margin = whole("OPERATOR_MARGIN_BPS", 0, 100_000)
evm_gas_price = whole("EVM_GAS_PRICE_WEI", 1, U64)
dusk_gas_price = whole("DUSK_AGENT_GAS_PRICE", 1, U64)
dusk_overhead = whole("DUSK_IGP_GAS_OVERHEAD", 0, U64 - IGP_MAX_GAS_LIMIT)
evm_overhead = whole("EVM_IGP_GAS_OVERHEAD", 1, U96)
relayer_limit = whole("DUSK_RELAYER_GAS_LIMIT", 1, U64)
evm_threshold = whole("EVM_ORIGIN_THRESHOLD", 1, 255)
validators = int(s["DUSK_ORIGIN_VALIDATOR_COUNT"] or 0)
dusk_threshold = int(s["DUSK_ORIGIN_THRESHOLD"]) if re.fullmatch(r"[0-9]+", s["DUSK_ORIGIN_THRESHOLD"]) else 0
evm_name = s["EVM_CHAIN_NAME"]

def finish(result):
    result["errors"] = errors
    if mode == "json":
        print(json.dumps(result, indent=2))
    else:
        print(summary(result) if "dusk_igp" in result else "")
    for error in errors:
        print(f"[FAIL] {error}", file=sys.stderr)
    sys.exit(1 if errors else 0)

if errors:
    finish({})

markup = Fraction(10_000 + margin, 10_000)
# LUX per wei and wei per LUX, scaled by 1e10, with the margin.
dusk_rate = ceil(ratio * LUX * SCALE * markup / WEI)
evm_rate = ceil(Fraction(WEI * SCALE) * markup / (ratio * LUX))
if dusk_rate > U64:
    errors.append(f"The Dusk IGP's token exchange rate {dusk_rate:,} doesn't fit a u64")
if evm_rate > U128:
    errors.append(f"The EVM IGP's token exchange rate {evm_rate:,} doesn't fit a uint128")

def dusk_quote(gas):
    return gas * evm_gas_price * dusk_rate // SCALE
def evm_quote(gas):
    return gas * dusk_gas_price * evm_rate // SCALE

# The checks the Dusk IGP makes before it accepts a pricing.
if dusk_quote(1 + dusk_overhead) == 0:
    errors.append("The Dusk IGP's pricing rounds the smallest payment to zero")
if dusk_quote(IGP_MAX_GAS_LIMIT + dusk_overhead) > U64:
    errors.append("The Dusk IGP's pricing makes the largest payment overflow a u64")

# Dusk -> EVM: a route transfer pays for DEFAULT_GAS + overhead, which must
# cover its delivery with DUSK_ORIGIN_THRESHOLD of the DUSK_ORIGIN_VALIDATORS.
dusk_to_evm_gas = DEFAULT_GAS + dusk_overhead
evm_delivery = None
if dusk_threshold and validators:
    evm_delivery = (EVM_DELIVERY_BASE + EVM_DELIVERY_PER_SIGNATURE * dusk_threshold
                    + EVM_DELIVERY_PER_VALIDATOR * validators)
    if dusk_to_evm_gas < evm_delivery:
        errors.append(
            f"DUSK_IGP_GAS_OVERHEAD is {dusk_overhead:,}: a transfer to {evm_name} would pay for "
            f"{dusk_to_evm_gas:,} gas, but delivering it with {dusk_threshold} of {validators} "
            f"signatures takes up to {evm_delivery:,} ({EVM_DELIVERY_BASE:,} plus "
            f"{EVM_DELIVERY_PER_SIGNATURE:,} per signature and {EVM_DELIVERY_PER_VALIDATOR:,} per "
            f"validator). Set it to at least {evm_delivery - DEFAULT_GAS:,}.")

# EVM -> Dusk: the route asks for no gas beyond the overhead, which must
# cover a relayed `process` at EVM_ORIGIN_THRESHOLD signatures, the route's
# handle included (docs/testnet.md, test_relayed_process_fits_the_relayer_gas_limit).
dusk_process = DUSK_PROCESS_BASE + DUSK_PROCESS_PER_SIGNATURE * evm_threshold
if evm_overhead < dusk_process:
    errors.append(
        f"EVM_IGP_GAS_OVERHEAD is {evm_overhead:,}, but a relayed process with "
        f"EVM_ORIGIN_THRESHOLD {evm_threshold} takes about {dusk_process:,} Dusk gas "
        f"({DUSK_PROCESS_BASE:,} plus {DUSK_PROCESS_PER_SIGNATURE:,} per signature). Raise it.")
if evm_overhead > relayer_limit:
    errors.append(
        f"EVM_IGP_GAS_OVERHEAD ({evm_overhead:,}) is above the relayer's gas limit "
        f"DUSK_RELAYER_GAS_LIMIT ({relayer_limit:,})")

def fraction(value):
    return f"{value.numerator}/{value.denominator}"

result = {
    "prices": {
        "eth_dusk_price_ratio": raw_ratio,
        "evm_gas_price_wei": evm_gas_price,
        "dusk_gas_price_lux": dusk_gas_price,
        "operator_margin_bps": margin,
    },
    # On-chain values and amounts are strings: jq before 1.7 rounds integers
    # above 2^53, and the EVM IGP's exchange rate can be that large.
    "dusk_igp": {
        "domain": int(s["EVM_DOMAIN"]),
        "gas_overhead": str(dusk_overhead),
        "token_exchange_rate": str(dusk_rate),
        "gas_price": str(evm_gas_price),
        "config": f"{s['EVM_DOMAIN']}:{dusk_overhead}:{dusk_rate}:{evm_gas_price}",
    },
    "evm_igp": {
        "domain": int(s["DUSK_DOMAIN"]),
        "gas_overhead": str(evm_overhead),
        "token_exchange_rate": str(evm_rate),
        "gas_price": str(dusk_gas_price),
    },
    "dusk_to_evm": {
        "gas": dusk_to_evm_gas,
        "quote_lux": str(dusk_quote(dusk_to_evm_gas)),
        # What the gas paid for costs at the prices, without the margin.
        "gas_cost_wei": str(dusk_to_evm_gas * evm_gas_price),
        "gas_cost_lux": str(int(dusk_to_evm_gas * evm_gas_price * ratio * LUX / WEI)),
        "delivery_gas": evm_delivery,
    },
    "evm_to_dusk": {
        "gas": evm_overhead,
        "quote_wei": str(evm_quote(evm_overhead)),
        "gas_cost_lux": str(evm_overhead * dusk_gas_price),
        "gas_cost_wei": str(int(evm_overhead * dusk_gas_price * WEI / (ratio * LUX))),
        "delivery_gas": dusk_process,
    },
    "relayer": {
        # Dusk-origin: what was paid for, priced with the margin, must cover
        # the relayer's own estimate of the EVM gas, priced without it.
        "dusk_origin_gas_fraction": fraction(Fraction(10_000, 10_000 + margin)),
        # EVM-origin: hyperlane-dusk estimates every delivery at the relayer's
        # gas limit, so require the overhead as a fraction of that.
        "evm_origin_gas_fraction": fraction(Fraction(evm_overhead, relayer_limit)),
        "evm_origin_required_gas": evm_overhead,
    },
}

def amount(value, decimals):
    whole_part, rest = divmod(value, 10**decimals)
    text = f"{whole_part:,}"
    if rest:
        text += "." + f"{rest:0{decimals}d}".rstrip("0")
    return text

def summary(r):
    d, e, rel = r["dusk_to_evm"], r["evm_to_dusk"], r["relayer"]
    lines = [
        f"Prices: 1 ETH = {raw_ratio} DUSK; {evm_name} gas at {amount(evm_gas_price, 9)} gwei; "
        f"Dusk gas at {dusk_gas_price} LUX; operator margin {margin / 100:g}% ({margin} bps)",
        "",
        f"Dusk -> {evm_name}: the Dusk IGP charges DUSK",
        f"  pricing for domain {s['EVM_DOMAIN']}: gas overhead {dusk_overhead:,}, "
        f"token exchange rate {dusk_rate:,}, gas price {evm_gas_price:,} wei",
        f"  a transfer pays for {d['gas']:,} {evm_name} gas: the {DEFAULT_GAS:,} the IGP charges "
        f"a message without hook metadata, plus the overhead",
        f"  quote {amount(int(d['quote_lux']), 9)} DUSK: that gas costs {amount(int(d['gas_cost_wei']), 18)} ETH, "
        f"{amount(int(d['gas_cost_lux']), 9)} DUSK, at these prices, plus the margin",
    ]
    if d["delivery_gas"]:
        lines.append(f"  delivering a transfer with {dusk_threshold} of {validators} signatures takes up to "
                     f"{d['delivery_gas']:,} gas")
    lines += [
        "",
        f"{evm_name} -> Dusk: the EVM IGP charges ETH",
        f"  pricing for domain {s['DUSK_DOMAIN']}: gas overhead {evm_overhead:,}, "
        f"token exchange rate {evm_rate:,}, gas price {dusk_gas_price} LUX",
        f"  a transfer pays for {e['gas']:,} Dusk gas: the overhead (the route adds none)",
        f"  quote {amount(int(e['quote_wei']), 18)} ETH ({int(e['quote_wei']):,} wei): that gas costs "
        f"{amount(int(e['gas_cost_lux']), 9)} DUSK, {amount(int(e['gas_cost_wei']), 18)} ETH, at these prices, "
        f"plus the margin",
        f"  a relayed process at EVM_ORIGIN_THRESHOLD {evm_threshold} takes up to about "
        f"{e['delivery_gas']:,} gas",
        "",
        "The relayer (agent-configs) relays any message, once it is paid for:",
        f"  from Dusk: its Dusk IGP payment covers {rel['dusk_origin_gas_fraction']} of the "
        f"{evm_name} gas the relayer estimates for delivering it",
        f"  from {evm_name}: it paid the EVM IGP for {rel['evm_origin_required_gas']:,} Dusk gas, "
        f"{rel['evm_origin_gas_fraction']} of the relayer's {relayer_limit:,} limit",
    ]
    return "\n".join(lines)

finish(result)
PY
}

# Compute the fees once, in this shell. `fee` reads them.
FEES=""
load_fees() {
    check_fee_settings
    FEES="$(compute_fees json)" || exit 1
}
fee() { jq -r "$1" <<<"$FEES"; }

cmd_fees() {
    check_fee_settings
    if [ "${1:-}" = "--json" ]; then
        compute_fees json
    else
        compute_fees text
    fi
}

# ── Chains and the recorded deployment ───────────────────────────────────────

# The node and the RPC must be on the chains the settings name before a
# command reads a price or signs anything. A transaction is signed for the
# endpoint's chain, and the IDs and addresses in $STATE can exist on another
# network: a Dusk contract ID comes from the bytecode, the deployer key and a
# nonce, an EVM address from the deployer and its nonce.
check_dusk_chain() {
    local chain_id
    require DUSK_RUES_URL DUSK_EXPECTED_CHAIN_ID
    chain_id="$(dusk_chain_id)" || fail "Dusk node at $DUSK_RUES_URL did not return the transfer contract's chain ID"
    [ "$chain_id" = "$DUSK_EXPECTED_CHAIN_ID" ] \
        || fail "Dusk node at $DUSK_RUES_URL is on chain ID $chain_id, expected DUSK_EXPECTED_CHAIN_ID $DUSK_EXPECTED_CHAIN_ID"
}
check_evm_chain() {
    local chain_id
    require EVM_RPC EVM_CHAIN_ID
    chain_id="$(cast chain-id --rpc-url "$EVM_RPC")" || fail "EVM RPC $EVM_RPC is unreachable"
    [ "$chain_id" = "$EVM_CHAIN_ID" ] \
        || fail "EVM RPC $EVM_RPC reports chain ID $chain_id, expected EVM_CHAIN_ID $EVM_CHAIN_ID"
}

# A Dusk hook takes calls only from the contract it was initialized with
# (its `mailbox`). dusk-tx deploy-hyperlane binds the IGP, the Mailbox's
# default hook, to the Mailbox, and the ProtocolFee to the aggregation hook
# that is the Mailbox's required hook and calls it. So a fee contract of
# this deployment takes its calls from the Mailbox, or from one of the
# Mailbox's hooks that takes its own from the Mailbox and calls it.
check_dusk_hook_binding() {
    local name="$1" contract="$2" mailbox="$3" bound method hook hook_bound hooks
    bound="$(dusk_value "the caller of the Dusk $name $contract" \
        --contract "$contract" --method mailbox --return-type bytes32)" || exit 1
    [ "$bound" != "$mailbox" ] || return 0
    for method in required_hook default_hook; do
        hook="$(dusk_value "the $method of the Dusk Mailbox $mailbox" \
            --contract "$mailbox" --method "$method" --return-type bytes32)" || exit 1
        [ "$bound" = "$hook" ] || continue
        hook_bound="$(dusk_value "the caller of the Dusk Mailbox's $method $hook" \
            --contract "$hook" --method mailbox --return-type bytes32)" || exit 1
        [ "$hook_bound" = "$mailbox" ] \
            || fail "The Dusk $name $contract takes its calls from the Mailbox's $method $hook, which takes its own from $hook_bound, not the Mailbox $mailbox recorded in $STATE"
        hooks="$(dusk_value "the hooks the Dusk Mailbox's $method $hook calls" \
            --contract "$hook" --method hooks --return-type contract-id-list)" || exit 1
        jq -e --arg c "$contract" 'any(.[]; . == $c)' <<<"$hooks" >/dev/null 2>&1 \
            || fail "The Dusk $name $contract takes its calls from the Mailbox's $method $hook, which doesn't call it (it calls $hooks)"
        return 0
    done
    fail "The Dusk $name $contract takes its calls from $bound, which is neither the Mailbox $mailbox recorded in $STATE nor one of its hooks"
}

# The Dusk contracts the fee commands use must be this deployment's: the
# recorded Mailbox on the expected chain at DUSK_DOMAIN, the contracts at
# this checkout's state versions, and the IGP and ProtocolFee taking their
# calls from that Mailbox or its hooks (check_dusk_hook_binding).
check_dusk_deployment() {
    local mailbox domain name contract
    check_dusk_chain
    mailbox="$(state_get '.dusk.mailbox')"
    [ -n "$mailbox" ] || fail "No Dusk deployment recorded in $STATE; run deploy first"
    [ "$(state_get '.dusk_domain')" = "$DUSK_DOMAIN" ] \
        || fail "$STATE was made for Dusk domain $(state_get '.dusk_domain'), not DUSK_DOMAIN $DUSK_DOMAIN"
    domain="$(dusk_value "the domain of the Dusk Mailbox $mailbox recorded in $STATE" \
        --contract "$mailbox" --method local_domain --return-type u32)" || exit 1
    [ "$domain" = "$DUSK_DOMAIN" ] \
        || fail "The Dusk Mailbox $mailbox recorded in $STATE has domain $domain, not DUSK_DOMAIN $DUSK_DOMAIN"
    check_state_versions
    for name in igp protocol_fee; do
        contract="$(state_get ".dusk.$name")"
        [ -n "$contract" ] || continue
        check_dusk_hook_binding "$name" "$contract" "$mailbox"
    done
}

# The same on the EVM side: the RPC on EVM_CHAIN_ID, the recorded Mailbox
# EVM_MAILBOX at EVM_DOMAIN, and the IGP and token this deployment created
# (check_evm_creations).
check_evm_deployment() {
    local domain
    check_evm_chain
    [ "$(lower "$(state_get '.evm.mailbox')")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "$STATE was made for EVM Mailbox $(state_get '.evm.mailbox'), not EVM_MAILBOX $EVM_MAILBOX"
    [ -z "$(state_get '.evm_domain')" ] || [ "$(state_get '.evm_domain')" = "$EVM_DOMAIN" ] \
        || fail "$STATE was made for EVM domain $(state_get '.evm_domain'), not EVM_DOMAIN $EVM_DOMAIN"
    domain="$(evm_number "$EVM_MAILBOX" "localDomain()(uint32)")" || fail "No Hyperlane Mailbox at $EVM_MAILBOX"
    [ "$domain" = "$EVM_DOMAIN" ] || fail "The EVM Mailbox $EVM_MAILBOX has domain $domain, not EVM_DOMAIN $EVM_DOMAIN"
    check_evm_creations
}

# ── Preflight ────────────────────────────────────────────────────────────────

preflight_dusk() {
    require DUSK_RUES_URL DUSK_EXPECTED_CHAIN_ID DUSK_DOMAIN
    local info height
    info="$(rues_post /on/node/info)" || fail "Dusk node at $DUSK_RUES_URL is unreachable"
    height="$(rues_post /on/graphql/query 'query { block(height: -1) { header { height } } }' \
        | jq -er '.block.header.height')" || fail "Dusk node did not report a block height"
    check_dusk_chain
    ok "Dusk node $(jq -r '.version_build' <<<"$info"), chain ID $DUSK_EXPECTED_CHAIN_ID, height $height"

    # Feature values are an activation height, or a list of [from, to) ranges.
    # Check them at the next block, where our transactions will run.
    python3 - "$height" "$(jq -c '.vm_config.features' <<<"$info")" <<'PY' || exit 1
import json, sys
height = int(sys.argv[1]) + 1
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
        errors.append(f"host query {name} is not active at the next block ({height})")
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
    local domain
    check_evm_chain
    domain="$(evm_call "$EVM_MAILBOX" "localDomain()(uint32)" | awk '{print $1}')" \
        || fail "No Hyperlane Mailbox at $EVM_MAILBOX"
    [ "$domain" = "$EVM_DOMAIN" ] || fail "EVM Mailbox domain is $domain, expected $EVM_DOMAIN"
    [ "$(lower "$(evm_call "$EVM_MERKLE_TREE_HOOK" "mailbox()(address)")")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "EVM_MERKLE_TREE_HOOK does not belong to EVM_MAILBOX"
    [ "$(lower "$(evm_call "$EVM_VALIDATOR_ANNOUNCE" "mailbox()(address)")")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "EVM_VALIDATOR_ANNOUNCE does not belong to EVM_MAILBOX"
    if [ -n "${EVM_ISM_FACTORY:-}" ]; then
        evm_has_code "$EVM_ISM_FACTORY" || fail "No contract at EVM_ISM_FACTORY $EVM_ISM_FACTORY"
    fi
    if [ -n "${EVM_AGGREGATION_HOOK_FACTORY:-}" ]; then
        evm_has_code "$EVM_AGGREGATION_HOOK_FACTORY" \
            || fail "No contract at EVM_AGGREGATION_HOOK_FACTORY $EVM_AGGREGATION_HOOK_FACTORY; leave it empty to deploy one"
    fi
    ok "EVM $EVM_CHAIN_NAME (chain $EVM_CHAIN_ID): Mailbox, Merkle hook and ValidatorAnnounce check out"
    route_hook_plan
}

# The route's hook replaces the Mailbox's default hook, and only the required
# hook runs as well. Validators sign EVM_MERKLE_TREE_HOOK's tree, and each
# message must be in it exactly once. So the route's hook is the IGP alone
# when the required hook already inserts a message to Dusk once, and
# aggregates the Merkle hook and the IGP when it doesn't. Sets
# ROUTE_HOOK_NEEDS_MERKLE.
ROUTE_HOOK_NEEDS_MERKLE=""
route_hook_plan() {
    local required count
    required="$(evm_call "$EVM_MAILBOX" "requiredHook()(address)")" || fail "Can't read the EVM Mailbox's required hook"
    count="$(merkle_insertions "$required")" || exit 1
    case "$count" in
        0)
            ROUTE_HOOK_NEEDS_MERKLE=1
            ok "The EVM Mailbox's required hook $required doesn't insert a message to Dusk into EVM_MERKLE_TREE_HOOK: the route's hook aggregates it and the IGP" ;;
        1)
            ROUTE_HOOK_NEEDS_MERKLE=0
            ok "The EVM Mailbox's required hook $required inserts a message to Dusk into EVM_MERKLE_TREE_HOOK once: the route's hook is the IGP" ;;
        *)
            fail "The EVM Mailbox's required hook $required inserts each message to Dusk into EVM_MERKLE_TREE_HOOK $count times, which no route's hook can undo" ;;
    esac
}

# How often a dispatch to Dusk through `hook` inserts the message into
# EVM_MERKLE_TREE_HOOK, following the hooks whose behaviour is known: an
# aggregation runs each of its hooks, a domain routing hook the one for Dusk
# (or its fallback), a Merkle tree hook inserts into its own tree, and an
# IGP, a ProtocolFee or a pausable hook inserts nothing. Any other kind, a
# routing hook with nothing for Dusk, or hooks nested deeper than
# MAX_HOOK_DEPTH stop the script: it can't tell how often a message would be
# inserted. Prints the count.
merkle_insertions() {
    local hook="$1" depth="${2:-0}" type children child route count=0 n
    [ "$depth" -lt "$MAX_HOOK_DEPTH" ] \
        || fail "The EVM Mailbox's required hook nests hooks more than $MAX_HOOK_DEPTH deep at $hook: deploy/testnet.sh can't tell how often it inserts a message to Dusk into EVM_MERKLE_TREE_HOOK"
    [ "$(lower "$hook")" != "$ZERO_ADDRESS" ] \
        || fail "The EVM Mailbox's required hook reaches the zero address: every dispatch would revert"
    type="$(evm_number "$hook" "hookType()(uint8)")" || fail "Can't read the hook type of $hook, in the EVM Mailbox's required hook"
    case "$type" in
        "$HOOK_MERKLE_TREE")
            [ "$(lower "$hook")" != "$(lower "$EVM_MERKLE_TREE_HOOK")" ] || count=1 ;;
        "$HOOK_AGGREGATION")
            children="$(evm_call "$hook" "hooks(bytes)(address[])" 0x)" \
                || fail "Can't read the hooks of the aggregation hook $hook, in the EVM Mailbox's required hook"
            for child in $(tr -d '[],' <<<"$children"); do
                n="$(merkle_insertions "$child" $((depth + 1)))" || exit 1
                count=$((count + n))
            done ;;
        "$HOOK_ROUTING"|"$HOOK_FALLBACK_ROUTING")
            route="$(evm_call "$hook" "hooks(uint32)(address)" "$DUSK_DOMAIN")" \
                || fail "Can't read where the routing hook $hook, in the EVM Mailbox's required hook, sends domain $DUSK_DOMAIN"
            if [ "$(lower "$route")" = "$ZERO_ADDRESS" ] && [ "$type" = "$HOOK_FALLBACK_ROUTING" ]; then
                route="$(evm_call "$hook" "fallbackHook()(address)")" || fail "Can't read the fallback hook of $hook"
            fi
            [ "$(lower "$route")" != "$ZERO_ADDRESS" ] \
                || fail "The routing hook $hook, in the EVM Mailbox's required hook, has no hook for domain $DUSK_DOMAIN: every dispatch to Dusk would revert"
            count="$(merkle_insertions "$route" $((depth + 1)))" || exit 1 ;;
        "$HOOK_IGP"|"$HOOK_PROTOCOL_FEE"|"$HOOK_PAUSABLE") ;;
        *)
            fail "The EVM Mailbox's required hook reaches $hook, of hook type $type, which deploy/testnet.sh can't follow: it can't tell how often a message to Dusk is inserted into EVM_MERKLE_TREE_HOOK" ;;
    esac
    printf '%s' "$count"
}

preflight_keys() {
    require DUSK_KEYS DUSK_CONSENSUS_PASSWORD EVM_DEPLOYER_KEY DUSK_ORIGIN_VALIDATORS DUSK_ORIGIN_THRESHOLD
    [ -f "$DUSK_KEYS" ] || fail "DUSK_KEYS file $DUSK_KEYS not found"
    [ -x "$DUSK_TX" ] || fail "dusk-tx not built at $DUSK_TX (run: make dusk-tx)"
    [ -f "$WASM_DIR/hyperlane_dusk_mailbox.wasm" ] || fail "contract WASMs not built (run: make all)"
    local deployer balance n account need
    account="$(dusk_tx account --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS")" \
        || fail "Can't read the Dusk deployer account: $(jq -r '.error // empty' <<<"$account" 2>/dev/null)"
    # Deploying the contracts costs about 300 DUSK, and each deploy needs
    # gas limit x gas price in the balance up front. Afterwards only cheap
    # calls remain.
    if [ -n "$(state_get '.dusk.mailbox')" ]; then need=1000000000; else need=$((400 * 1000000000)); fi
    balance="$(jq -r '.balance' <<<"$account")"
    [ "$balance" -ge "$need" ] \
        || fail "Dusk deployer $(jq -r '.address' <<<"$account") has $(jq -r '.balance_dusk' <<<"$account") DUSK, needs at least $((need / 1000000000))"
    ok "Dusk deployer $(jq -r '.address' <<<"$account") has $(jq -r '.balance_dusk' <<<"$account") DUSK"
    deployer="$(evm_deployer)" || fail "EVM_DEPLOYER_KEY is not a valid key"
    balance="$(cast balance "$deployer" --rpc-url "$EVM_RPC" --ether)"
    ok "EVM deployer $deployer has $balance ETH"
    if [ -n "${EVM_IGP_BENEFICIARY:-}" ]; then
        [[ "$EVM_IGP_BENEFICIARY" =~ ^0x[0-9a-fA-F]{40}$ ]] && [ "$(lower "$EVM_IGP_BENEFICIARY")" != "$ZERO_ADDRESS" ] \
            || fail "EVM_IGP_BENEFICIARY must be an EVM address"
    fi
    n="$(count_addresses "$DUSK_ORIGIN_VALIDATORS")"
    [ "$DUSK_ORIGIN_THRESHOLD" -ge 1 ] && [ "$DUSK_ORIGIN_THRESHOLD" -le "$n" ] \
        || fail "DUSK_ORIGIN_THRESHOLD must be between 1 and the $n Dusk-origin validators"
    check_evm_origin_policy
}

# The validators the Dusk side is meant to trust for EVM-origin messages.
check_evm_origin_policy() {
    require EVM_ORIGIN_VALIDATORS EVM_ORIGIN_THRESHOLD
    local n
    n="$(count_addresses "$EVM_ORIGIN_VALIDATORS")"
    [[ "$EVM_ORIGIN_THRESHOLD" =~ ^[1-9][0-9]*$ ]] && [ "$EVM_ORIGIN_THRESHOLD" -le "$n" ] \
        || fail "EVM_ORIGIN_THRESHOLD must be between 1 and the $n EVM-origin validators"
}

# A relayed `process` costs about 1.95M gas plus 0.22M per signature the
# Dusk ISM checks (test_relayed_process_fits_the_relayer_gas_limit). Below
# that every delivery runs out of gas and pays the whole limit, which the
# relayer's simulation can't catch, so insist on 1.5 times the cost.
# check_deployed_ism makes sure the deployed ISM's threshold is
# EVM_ORIGIN_THRESHOLD; the larger of the two is checked all the same.
check_relayer_gas_limit() {
    local threshold="$EVM_ORIGIN_THRESHOLD" basis="EVM_ORIGIN_THRESHOLD" need
    if [ -n "$DEPLOYED_ISM_THRESHOLD" ] && [ "$DEPLOYED_ISM_THRESHOLD" -gt "$threshold" ]; then
        threshold="$DEPLOYED_ISM_THRESHOLD" basis="the deployed ISM's threshold"
    fi
    need=$(((1950000 + 220000 * threshold) * 3 / 2))
    [ "$DUSK_RELAYER_GAS_LIMIT" -ge "$need" ] \
        || fail "DUSK_RELAYER_GAS_LIMIT is $DUSK_RELAYER_GAS_LIMIT; with $basis of $threshold it must be at least $need"
}

# How many deliveries to Dusk the relayer sends together, as consecutive
# transactions of its account (its maxBatchSize for Dusk): 1 to 64, the most
# of an account's transactions Rusk queues ahead of its next nonce
# (deploy/testnet.env.example has why 16, and which agents batches need).
check_relayer_batch_size() {
    [[ "$DUSK_RELAYER_MAX_BATCH_SIZE" =~ ^[1-9][0-9]*$ ]] && [ "$DUSK_RELAYER_MAX_BATCH_SIZE" -le 64 ] \
        || fail "DUSK_RELAYER_MAX_BATCH_SIZE must be a whole number from 1 to 64, the most of an account's transactions Rusk queues ahead of its next nonce, not '$DUSK_RELAYER_MAX_BATCH_SIZE'"
}

# Multicall for Dusk, through which the agents batch their contract reads
# (batchContractAddress). It must answer `version` with 1, the ABI the
# agents speak. On a chain without it, or with another version, the configs
# leave it out, with a warning, and the agents make single queries: an
# address that answers nothing would only make every batch fail first. Any
# other failure to ask stops the script. Sets DUSK_MULTICALL_ID.
DUSK_MULTICALL_ID=""
check_dusk_multicall() {
    local id out version
    DUSK_MULTICALL_ID=""
    id="$(lower "${DUSK_MULTICALL#0x}")"
    if [ -z "$id" ]; then
        info "DUSK_MULTICALL is empty: the agents make single contract queries"
        return 0
    fi
    [[ "$id" =~ ^[0-9a-f]{64}$ ]] || fail "DUSK_MULTICALL must be a contract ID of 64 hex characters, not '$DUSK_MULTICALL'"
    if ! out="$(dusk_query --contract "$id" --method version --return-type u32 2>/dev/null)"; then
        grep -qiE 'ContractDoesNotExist|contract does not exist|contract owner not found' <<<"$out" \
            || fail "Can't ask Multicall for Dusk at $id for its version: $(jq -r '.error // "the query failed"' <<<"$out" 2>/dev/null)"
        warn "Multicall for Dusk isn't on this chain at $id (DUSK_MULTICALL): the agent configs leave batchContractAddress out, and the agents make single contract queries"
        return 0
    fi
    version="$(jq -r '.value // empty' <<<"$out")"
    if [ "$version" != 1 ]; then
        warn "The contract at DUSK_MULTICALL $id answers version '$version', not 1, the Multicall for Dusk ABI the agents speak: the agent configs leave batchContractAddress out"
        return 0
    fi
    DUSK_MULTICALL_ID="$id"
    ok "Multicall for Dusk answers at $id (version 1): the agents batch their contract reads through it"
}

# Check the ISM that verifies messages to the deployed route: the route's
# own ISM if it has one, otherwise the Mailbox's default. It must be a
# MessageIdMultisig ISM whose validators and threshold are
# EVM_ORIGIN_VALIDATORS and EVM_ORIGIN_THRESHOLD, so that the agent configs,
# and the relayer's gas limit, are made for what the chain enforces. The owner
# can change either on chain after deploying. A TestMock ISM accepts every
# message; only a local rehearsal can allow it (allows_test_mock_ism). Every
# query must succeed. Sets DEPLOYED_ISM_THRESHOLD. Before the Dusk contracts
# are deployed there is nothing to check.
DEPLOYED_ISM_THRESHOLD=""
check_deployed_ism() {
    local route mailbox ism which type policy validators threshold expected
    DEPLOYED_ISM_THRESHOLD=""
    route="$(state_get '.dusk.warp_native')"
    mailbox="$(state_get '.dusk.mailbox')"
    [ -n "$route" ] && [ -n "$mailbox" ] || return 0
    check_evm_origin_policy
    ism="$(dusk_value "the ISM of the Dusk route $route recorded in $STATE (if the network was reset, move $OUT_DIR aside and deploy again)" \
        --contract "$route" --method interchain_security_module --return-type bytes32)" || exit 1
    which="its own ISM"
    if [ "$ism" = "$ZERO_ID" ]; then
        ism="$(dusk_value "the default ISM of the Dusk Mailbox $mailbox" \
            --contract "$mailbox" --method default_ism --return-type bytes32)" || exit 1
        which="the Mailbox's default ISM"
    fi
    [[ "$ism" =~ ^[0-9a-f]{64}$ ]] && [ "$ism" != "$ZERO_ID" ] \
        || fail "The Dusk route uses $which, which is not set ('$ism')"
    which="$which, $ism,"
    type="$(dusk_value "the module type of the Dusk ISM $ism" \
        --contract "$ism" --method module_type --return-type u8)" || exit 1
    case "$type" in
        "$MESSAGE_ID_MULTISIG_ISM") ;;
        "$TEST_MOCK_ISM")
            allows_test_mock_ism \
                || fail "The Dusk route uses $which a TestMock ISM that accepts every message. Point it at a MessageIdMultisig ISM."
            warn "The Dusk route uses $which a TestMock ISM that accepts every message. Allowed for this local rehearsal only."
            return 0 ;;
        *) fail "The Dusk route uses $which of module type $type; it must be a MessageIdMultisig ISM ($MESSAGE_ID_MULTISIG_ISM)" ;;
    esac
    policy="$(dusk_value "the validators and threshold of the Dusk ISM $ism" \
        --contract "$ism" --method validators_and_threshold --return-type validators-and-threshold)" || exit 1
    validators="$(jq -er '.validators | join(",")' <<<"$policy")" \
        && threshold="$(jq -er '.threshold' <<<"$policy")" && [[ "$threshold" =~ ^[0-9]+$ ]] \
        || fail "The Dusk ISM $ism returned a malformed validator set"
    validators="$(sorted_addresses "$validators")"
    expected="$(sorted_addresses "$EVM_ORIGIN_VALIDATORS")"
    [ "$validators" = "$expected" ] \
        || fail "The Dusk route uses $which which trusts [$validators], but EVM_ORIGIN_VALIDATORS is [$expected]. Set EVM_ORIGIN_VALIDATORS to what the chain has, or change the ISM."
    [ "$threshold" -eq "$EVM_ORIGIN_THRESHOLD" ] \
        || fail "The Dusk route uses $which which needs $threshold signatures, but EVM_ORIGIN_THRESHOLD is $EVM_ORIGIN_THRESHOLD. Set EVM_ORIGIN_THRESHOLD, and a DUSK_RELAYER_GAS_LIMIT that covers it, to what the chain has, or change the ISM."
    DEPLOYED_ISM_THRESHOLD="$threshold"
    ok "The Dusk route uses $which a MessageIdMultisig ISM: $threshold of [$validators]"
}

# LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1 lets a local rehearsal's route use a
# TestMock ISM. It counts only when both endpoints are local. The rehearsal
# deploys a MessageIdMultisig ISM, so it doesn't need this by default.
allows_test_mock_ism() {
    [ "${LOCAL_REHEARSAL_ALLOW_TEST_MOCK:-0}" = 1 ] || return 1
    is_local_url "$DUSK_RUES_URL" && is_local_url "$EVM_RPC" \
        || fail "LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1 only counts when DUSK_RUES_URL and EVM_RPC are both http://localhost or http://127.0.0.1"
}
# The host, and an optional port, must be followed by the path or nothing, so
# that userinfo such as http://localhost:8080@node.example/ doesn't count.
is_local_url() { [[ "$1" =~ ^http://(localhost|127\.0\.0\.1)(:[0-9]+)?(/|$) ]]; }

# ── Fee contracts on chain ───────────────────────────────────────────────────

# The Dusk IGP's pricing for the EVM domain, as JSON.
dusk_igp_pricing() {
    local pricing
    pricing="$(dusk_value "the Dusk IGP's pricing for domain $EVM_DOMAIN" --contract "$(state_get '.dusk.igp')" \
        --method domain_gas_config --return-type domain-gas-config --arg-u32 "$EVM_DOMAIN")" || exit 1
    jq -c '{gas_overhead, token_exchange_rate, gas_price} | map_values(tostring)' <<<"$pricing"
}

# The EVM IGP's pricing for the Dusk domain, as JSON: its oracle and
# overhead, and the oracle's exchange rate and gas price.
evm_igp_pricing() {
    local igp oracle config data
    igp="$(state_get '.evm.igp')"
    config="$(evm_call "$igp" "destinationGasConfigs(uint32)(address,uint96)" "$DUSK_DOMAIN")" \
        || fail "Can't read the EVM IGP $igp's configuration for domain $DUSK_DOMAIN"
    oracle="$(sed -n 1p <<<"$config")"
    data="0 0"
    if [ "$(lower "$oracle")" != "$ZERO_ADDRESS" ]; then
        data="$(evm_oracle_data "$oracle")" || exit 1
    fi
    jq -cn --arg oracle "$oracle" --arg overhead "$(sed -n 2p <<<"$config" | awk '{print $1}')" \
        --arg data "$data" '($data | split(" ")) as $d | {
          oracle: $oracle, gas_overhead: $overhead, token_exchange_rate: $d[0], gas_price: $d[1]}'
}

# A gas oracle's token exchange rate and gas price for Dusk, as "RATE PRICE".
evm_oracle_data() {
    local data
    data="$(evm_call "$1" "remoteGasData(uint32)(uint128,uint128)" "$DUSK_DOMAIN" | awk '{print $1}' | paste -sd' ')" \
        && [[ "$data" =~ ^[0-9]+\ [0-9]+$ ]] \
        || { echo "[FAIL] Can't read the gas oracle $1's data for domain $DUSK_DOMAIN" >&2; return 1; }
    printf '%s' "$data"
}

# The EVM IGP reads Dusk's pricing from the StorageGasOracle the state
# records, or, until deploy points it there, from none. Any other oracle got
# there through the owner key, and nothing here changes the IGP until that is
# looked at: pricing the recorded oracle and pointing the IGP back at it
# would replace whatever pricing is live with one nobody has checked.
check_evm_igp_oracle() {
    local active oracle
    active="$(lower "$(jq -r .oracle <<<"$1")")" oracle="$(state_get '.evm.storage_gas_oracle')"
    [ "$active" = "$ZERO_ADDRESS" ] || [ "$active" = "$(lower "$oracle")" ] \
        || fail "The EVM IGP $(state_get '.evm.igp') reads domain $DUSK_DOMAIN's pricing from $active, not the StorageGasOracle $oracle recorded in $STATE. Nothing was sent: find out who changed it, then point it back with setDestinationGasConfigs, or record that oracle in $STATE."
}

# Price the StorageGasOracle the state records for Dusk, then point the EVM
# IGP at it with the settings' overhead, in that order: the IGP only ever
# reads from an oracle that has just been read back with the settings'
# pricing. An oracle without pricing for Dusk prices it at zero, and the
# relayer would deliver every message to Dusk for nothing. In `deploy` mode
# only what isn't set yet is set: a pricing already on chain stays, and
# preflight reports how it differs; `apply` sets everything the settings say.
configure_evm_igp_pricing() {
    local mode="$1" deployer igp oracle current active data wanted overhead
    deployer="$(evm_deployer)"
    igp="$(state_get '.evm.igp')" oracle="$(state_get '.evm.storage_gas_oracle')"
    current="$(evm_igp_pricing)" || exit 1
    check_evm_igp_oracle "$current"
    [ "$(lower "$(evm_call "$oracle" "owner()(address)")")" = "$(lower "$deployer")" ] \
        || fail "The StorageGasOracle $oracle isn't owned by the EVM deployer $deployer"
    wanted="$(fee .evm_igp.token_exchange_rate) $(fee .evm_igp.gas_price)"
    overhead="$(fee .evm_igp.gas_overhead)"
    active="$(lower "$(jq -r .oracle <<<"$current")")"
    data="$(evm_oracle_data "$oracle")" || exit 1
    # Until the IGP reads from the oracle, nothing quotes from it.
    if [ "$data" != "$wanted" ] && { [ "$mode" = apply ] || [ "$data" = "0 0" ] || [ "$active" = "$ZERO_ADDRESS" ]; }; then
        info "Pricing domain $DUSK_DOMAIN in the StorageGasOracle $oracle..."
        evm_send "$oracle" "setRemoteGasDataConfigs((uint32,uint128,uint128)[])" "[($DUSK_DOMAIN,${wanted/ /,})]"
        data="$(evm_oracle_data "$oracle")" || exit 1
        [ "$data" = "$wanted" ] \
            || fail "The StorageGasOracle $oracle prices domain $DUSK_DOMAIN at $data after the update, not $wanted"
    fi
    if [ "$active" = "$ZERO_ADDRESS" ] \
        || { [ "$mode" = apply ] && [ "$(jq -r .gas_overhead <<<"$current")" != "$overhead" ]; }; then
        [ "$data" = "$wanted" ] \
            || fail "Not pointing the EVM IGP $igp at the StorageGasOracle $oracle, which prices domain $DUSK_DOMAIN at $data, not the settings' $wanted. Price it with: bash deploy/testnet.sh evm-igp --apply"
        info "Pointing the EVM IGP at the oracle for domain $DUSK_DOMAIN, with a gas overhead of $overhead..."
        evm_send "$igp" "setDestinationGasConfigs((uint32,(address,uint96))[])" "[($DUSK_DOMAIN,($oracle,$overhead))]"
    fi
}

# The Dusk IGP must charge what the settings say: agent-configs sets the
# relayer's requirements from them. dusk-igp shows and applies a difference.
check_dusk_igp_pricing() {
    local igp current wanted
    igp="$(state_get '.dusk.igp')"
    [ -n "$igp" ] || return 0
    check_dusk_igp_is_default_hook
    current="$(dusk_igp_pricing)" || exit 1
    wanted="$(fee '.dusk_igp | {gas_overhead, token_exchange_rate, gas_price}')"
    [ "$(jq -cS . <<<"$current")" = "$(jq -cS . <<<"$wanted")" ] \
        || fail "The Dusk IGP $igp prices domain $EVM_DOMAIN at $(jq -c . <<<"$current"), but the settings say $(jq -c . <<<"$wanted"). Compare them with: bash deploy/testnet.sh dusk-igp; apply the settings with --apply."
    ok "The Dusk IGP charges $(fee '.dusk_to_evm.quote_lux') LUX for a transfer to $EVM_CHAIN_NAME"
}

# Every dispatch pays the Dusk Mailbox's default hook, so that is the IGP
# whose pricing counts, and whose payments the relayer reads.
check_dusk_igp_is_default_hook() {
    local igp mailbox hook
    igp="$(state_get '.dusk.igp')" mailbox="$(state_get '.dusk.mailbox')"
    hook="$(dusk_value "the default hook of the Dusk Mailbox $mailbox" \
        --contract "$mailbox" --method default_hook --return-type bytes32)" || exit 1
    [ "$hook" = "$igp" ] \
        || fail "The Dusk Mailbox $mailbox's default hook is $hook, not the IGP $igp recorded in $STATE: dispatches pay that one"
}

# Before `deploy`, an IGP it hasn't finished configuring only warns: deploy
# configures it.
check_evm_igp_pricing() {
    local when="${1:-}" igp current wanted
    igp="$(state_get '.evm.igp')"
    [ -n "$igp" ] && [ -n "$(state_get '.evm.storage_gas_oracle')" ] || return 0
    current="$(evm_igp_pricing)" || exit 1
    if [ "$when" = before-deploy ] \
        && jq -e '.oracle == "0x0000000000000000000000000000000000000000" or .gas_price == "0"' <<<"$current" >/dev/null; then
        warn "The EVM IGP $igp isn't priced for Dusk yet; deploy prices it"
        return 0
    fi
    wanted="$(fee '.evm_igp | {gas_overhead, token_exchange_rate, gas_price}')"
    [ "$(lower "$(jq -r .oracle <<<"$current")")" = "$(lower "$(state_get '.evm.storage_gas_oracle')")" ] \
        || fail "The EVM IGP $igp reads domain $DUSK_DOMAIN's pricing from $(jq -r .oracle <<<"$current"), not the StorageGasOracle $(state_get '.evm.storage_gas_oracle') recorded in $STATE"
    [ "$(jq -cS 'del(.oracle)' <<<"$current")" = "$(jq -cS . <<<"$wanted")" ] \
        || fail "The EVM IGP $igp prices domain $DUSK_DOMAIN at $(jq -c 'del(.oracle)' <<<"$current"), but the settings say $(jq -c . <<<"$wanted"). Compare them with: bash deploy/testnet.sh evm-igp; apply the settings with --apply."
    check_evm_igp_quotes "$current"
    ok "The EVM IGP charges $(fee '.evm_to_dusk.quote_wei') wei for a transfer to Dusk"
}

# Signed offchain quotes come before the oracle: a quote signer the owner
# adds can price delivery to Dusk at anything, for one sender or for all,
# and a standing quote outlives the signer's removal until it expires. This
# deployment uses none. The IGP resolves quotes by the caller, so ask it what
# a transfer pays as the route, whose messages it prices, and as any other
# sender, against what the pricing `current` (evm_igp_pricing) comes to.
check_evm_igp_quotes() {
    local current="$1" igp signers gas expected sender quote
    igp="$(state_get '.evm.igp')"
    signers="$(evm_call "$igp" "quoteSigners()(address[])")" || fail "Can't read the EVM IGP $igp's quote signers"
    [ -z "$(tr -d '[] ' <<<"$signers")" ] \
        || fail "The EVM IGP $igp has quote signers $signers: offchain quotes they sign override its StorageGasOracle's pricing, which the relayer's requirements follow. Remove them (removeQuoteSigner), and wait for their standing quotes to expire."
    gas="$(fee .evm_igp.gas_overhead)"
    expected="$(igp_cost "$gas" "$(jq -r .gas_price <<<"$current")" "$(jq -r .token_exchange_rate <<<"$current")")"
    for sender in "$(state_get '.evm.token')" "$ZERO_ADDRESS"; do
        [ -n "$sender" ] || continue
        quote="$(evm_number "$igp" "quoteGasPayment(uint32,uint256)(uint256)" "$DUSK_DOMAIN" "$gas" --from "$sender")" \
            || fail "Can't read the EVM IGP $igp's quote for a message from $sender"
        [ "$quote" = "$expected" ] \
            || fail "The EVM IGP $igp quotes $quote wei for $gas Dusk gas to a message from $sender, not the $expected its oracle's pricing comes to: a signed offchain quote overrides the oracle (docs/security.md, HIGH-8)."
    done
}
# What gas costs at a gas price and token exchange rate, as the IGPs compute
# it. Bash's integers are too small.
igp_cost() { python3 -c 'import sys; g, p, r = map(int, sys.argv[1:]); print(g * p * r // 10**10)' "$@"; }

# The EVM route must dispatch through its recorded hook, and that hook must
# insert into the Merkle tree validators sign, unless the required hook
# does, and pay our IGP. Before `deploy`, a route that doesn't yet only
# warns: deploy sets its hook.
check_evm_route_hook() {
    local when="${1:-}" token hook igp current children expected
    token="$(state_get '.evm.token')" hook="$(state_get '.evm.route_hook')" igp="$(state_get '.evm.igp')"
    [ -n "$token" ] || return 0
    if [ -z "$hook" ] || [ -z "$(state_get '.evm.storage_gas_oracle')" ]; then
        [ "$when" = before-deploy ] || fail "The EVM route $token has no hook that pays an IGP of this deployment: run deploy"
        warn "The EVM route $token has no hook that pays an IGP of this deployment yet; deploy sets one"
        return 0
    fi
    current="$(evm_call "$token" "hook()(address)")" || fail "Can't read the hook of the EVM route $token"
    if [ "$(lower "$current")" != "$(lower "$hook")" ]; then
        [ "$when" = before-deploy ] || fail "The EVM route $token dispatches through hook $current, not $hook recorded in $STATE: run deploy"
        warn "The EVM route $token dispatches through hook $current; deploy sets $hook"
        return 0
    fi
    if [ "$(lower "$hook")" = "$(lower "$igp")" ]; then
        [ "$ROUTE_HOOK_NEEDS_MERKLE" = 0 ] \
            || fail "The EVM route's hook is the IGP alone, so its messages never reach the Merkle tree validators sign"
        ok "The EVM route dispatches through the IGP $igp; the Mailbox's required hook inserts into $EVM_MERKLE_TREE_HOOK"
        return 0
    fi
    [ "$ROUTE_HOOK_NEEDS_MERKLE" = 1 ] \
        || fail "The EVM Mailbox's required hook now inserts into $EVM_MERKLE_TREE_HOOK, and so does the route's hook $hook: every message would be inserted twice. Point the route's hook at the IGP $igp."
    children="$(evm_call "$hook" "hooks(bytes)(address[])" 0x)" || fail "Can't read the hooks of $hook"
    children="$(tr -d '[] ' <<<"$children" | tr '[:upper:]' '[:lower:]')"
    expected="$(lower "$EVM_MERKLE_TREE_HOOK,$igp")"
    [ "$children" = "$expected" ] \
        || fail "The EVM route's hook $hook aggregates [$children], not [$expected] (the Merkle hook validators sign, then the IGP)"
    ok "The EVM route dispatches through $hook: Merkle tree $EVM_MERKLE_TREE_HOOK, then IGP $igp"
}

cmd_preflight() {
    for tool in jq cast forge curl python3; do
        command -v "$tool" >/dev/null || fail "$tool is required"
    done
    load_fees
    check_relayer_batch_size
    preflight_dusk
    check_dusk_multicall
    preflight_evm
    preflight_keys
    check_state_matches_chain
    check_deployed_ism
    check_relayer_gas_limit
    check_dusk_igp_pricing
    check_evm_igp_pricing before-deploy
    check_evm_route_hook before-deploy
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
        --igp-domain-config "$(fee '.dusk_igp.config')" \
        --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID" \
        > "$out" || fail "Dusk deployment failed: $(jq -r '.error // empty' "$out" 2>/dev/null)"
    state_set --slurpfile d "$out" --arg domain "$DUSK_DOMAIN" \
        '.dusk = $d[0].contracts | .dusk_domain = ($domain | tonumber)'
    ok "Dusk Mailbox $(state_get '.dusk.mailbox'), WarpNative $(state_get '.dusk.warp_native')"
}

# Who can claim the fees on Dusk: DUSK_FEE_BENEFICIARY, or the deployer that
# deploy-hyperlane makes the beneficiary.
configure_dusk_beneficiary() {
    [ -n "${DUSK_FEE_BENEFICIARY:-}" ] || return 0
    local name contract out
    for name in igp protocol_fee; do
        contract="$(state_get ".dusk.$name")"
        out="$(dusk_tx set-beneficiary --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
            --contract "$contract" --beneficiary "$DUSK_FEE_BENEFICIARY" \
            --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
            || fail "Can't set the beneficiary of the Dusk $name $contract: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
    done
    ok "The Dusk IGP's and ProtocolFee's fees go to $DUSK_FEE_BENEFICIARY"
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
        if ! evm_has_code "$ism"; then
            info "Deploying MessageIdMultisigIsm ($DUSK_ORIGIN_THRESHOLD of [$validators]) through the factory..."
            evm_send "$EVM_ISM_FACTORY" "deploy(address[],uint8)" "[$validators]" "$DUSK_ORIGIN_THRESHOLD"
        fi
    else
        info "Deploying StorageMessageIdMultisigIsm ($DUSK_ORIGIN_THRESHOLD of [$validators])..."
        ism="$(forge_create contracts/isms/multisig/StorageMultisigIsm.sol:StorageMessageIdMultisigIsm \
            --constructor-args "[$validators]" "$DUSK_ORIGIN_THRESHOLD")" || fail "EVM security module deployment failed"
    fi
    evm_has_code "$ism" || fail "EVM security module deployment failed"
    state_set --arg ism "$ism" --arg v "$validators" --argjson t "$DUSK_ORIGIN_THRESHOLD" \
        '.evm.ism = $ism | .dusk_origin_validators = ($v | split(",")) | .dusk_origin_threshold = $t'
    ok "EVM security module $ism"
}

# The first block the agents index on the EVM chain: before our first
# contract there, so they see the IGP's payments and the route's dispatches.
record_index_from() {
    [ -z "$(state_get '.evm.index_from')" ] || return 0
    state_set --argjson b "$(cast block-number --rpc-url "$EVM_RPC")" '.evm.index_from = $b'
}

# An older version of this script recorded Hyperlane's own IGP, which has
# no pricing for Dusk, as .evm.igp.
forget_hyperlane_igp() {
    if [ -n "$(state_get '.evm.igp')" ] && [ -z "$(state_get '.evm.storage_gas_oracle')" ]; then
        state_set '.evm.hyperlane_igp = .evm.igp | del(.evm.igp)'
    fi
}

# Our IGP on the EVM chain, priced for Dusk through our own StorageGasOracle.
# The EVM deployer owns both: the oracle's constructor makes it the owner,
# and the IGP is initialized in the transaction that creates it
# (deploy_evm_proxied). EVM_IGP_BENEFICIARY, or the deployer, can claim the
# payments. A configuration already on chain is left as it is: preflight
# stops when it differs from the settings, and evm-igp --apply changes it.
deploy_evm_igp() {
    local deployer beneficiary oracle igp owner init
    deployer="$(evm_deployer)"
    beneficiary="${EVM_IGP_BENEFICIARY:-$deployer}"
    forget_hyperlane_igp
    oracle="$(state_get '.evm.storage_gas_oracle')"
    if [ -z "$oracle" ]; then
        record_index_from
        info "Deploying a StorageGasOracle on $EVM_CHAIN_NAME..."
        oracle="$(forge_create contracts/hooks/igp/StorageGasOracle.sol:StorageGasOracle)" \
            || fail "StorageGasOracle deployment failed"
        state_set --arg o "$oracle" '.evm.storage_gas_oracle = $o'
    fi
    [ "$(lower "$(evm_call "$oracle" "owner()(address)")")" = "$(lower "$deployer")" ] \
        || fail "The StorageGasOracle $oracle isn't owned by the EVM deployer $deployer"
    if [ -z "$(state_get '.evm.igp')" ]; then
        record_index_from
        init="$(cast calldata "initialize(address,address)" "$deployer" "$beneficiary")" \
            || fail "Can't encode the EVM IGP's initialize call"
        deploy_evm_proxied igp IGP contracts/hooks/igp/InterchainGasPaymaster.sol:InterchainGasPaymaster "$init"
    fi
    igp="$(state_get '.evm.igp')"
    check_evm_creation igp IGP
    # Only its owner, the deployer, could have handed it to someone else.
    owner="$(evm_call "$igp" "owner()(address)")" || fail "Can't read the owner of the EVM IGP $igp"
    [ "$(lower "$owner")" = "$(lower "$deployer")" ] \
        || fail "The EVM IGP $igp is owned by $owner, not the EVM deployer $deployer"
    # EVM_IGP_BENEFICIARY, when set, wins over what the IGP has; when it isn't,
    # the beneficiary stays what it is, as on Dusk.
    if [ -n "${EVM_IGP_BENEFICIARY:-}" ] \
        && [ "$(lower "$(evm_call "$igp" "beneficiary()(address)")")" != "$(lower "$EVM_IGP_BENEFICIARY")" ]; then
        info "Making $EVM_IGP_BENEFICIARY the EVM IGP's beneficiary..."
        evm_send "$igp" "setBeneficiary(address)" "$EVM_IGP_BENEFICIARY"
    fi
    beneficiary="$(evm_call "$igp" "beneficiary()(address)")" || fail "Can't read the EVM IGP's beneficiary"
    state_set --arg b "$beneficiary" '.evm.igp_beneficiary = $b'

    configure_evm_igp_pricing deploy
    check_evm_igp_pricing
    ok "EVM IGP $igp (owner $deployer, beneficiary $beneficiary)"
}

# The EVM route's hook: our IGP, aggregated with EVM_MERKLE_TREE_HOOK when
# the Mailbox's required hook doesn't insert into it (route_hook_plan), as on
# Sepolia, where the required hook is a ProtocolFee and only the default hook,
# which the route's hook replaces, inserts. The aggregation hook comes from
# EVM_AGGREGATION_HOOK_FACTORY, Hyperlane's StaticAggregationHookFactory, or a
# factory this script deploys when that is empty.
deploy_evm_route_hook() {
    local igp hook factory
    igp="$(state_get '.evm.igp')"
    if [ "$ROUTE_HOOK_NEEDS_MERKLE" = 0 ]; then
        hook="$igp"
    else
        factory="${EVM_AGGREGATION_HOOK_FACTORY:-$(state_get '.evm.aggregation_hook_factory')}"
        if [ -z "$factory" ]; then
            info "Deploying a StaticAggregationHookFactory on $EVM_CHAIN_NAME..."
            factory="$(forge_create contracts/hooks/aggregation/StaticAggregationHookFactory.sol:StaticAggregationHookFactory)" \
                || fail "StaticAggregationHookFactory deployment failed"
            state_set --arg f "$factory" '.evm.aggregation_hook_factory = $f'
        fi
        hook="$(evm_call "$factory" "getAddress(address[])(address)" "[$EVM_MERKLE_TREE_HOOK,$igp]")" \
            || fail "Can't get the aggregation hook's address from the factory $factory"
        if ! evm_has_code "$hook"; then
            info "Deploying the route's hook, aggregating the Merkle hook $EVM_MERKLE_TREE_HOOK and the IGP $igp..."
            evm_send "$factory" "deploy(address[])" "[$EVM_MERKLE_TREE_HOOK,$igp]"
        fi
        evm_has_code "$hook" || fail "The aggregation hook deployment failed"
        [ "$(evm_number "$hook" "hookType()(uint8)")" = "$HOOK_AGGREGATION" ] \
            || fail "$hook is not an aggregation hook"
    fi
    state_set --arg h "$hook" '.evm.route_hook = $h'
}

deploy_evm_token() {
    if [ -n "$(state_get '.evm.token')" ]; then
        ok "EVM DUSK token already deployed ($(state_get '.evm.token'))"
        return
    fi
    local deployer init
    deployer="$(evm_deployer)"
    record_index_from
    # Initialized in the transaction that creates it (deploy_evm_proxied):
    # before, anyone could initialize it first with their own ISM, owner and
    # supply, and deploy stopped.
    init="$(cast calldata "initialize(uint256,string,string,address,address,address)" \
        0 "Dusk" "DUSK" "$(state_get '.evm.route_hook')" "$(state_get '.evm.ism')" "$deployer")" \
        || fail "Can't encode the EVM token's initialize call"
    info "Deploying the synthetic DUSK token on $EVM_CHAIN_NAME (9 decimals, matching LUX)..."
    deploy_evm_proxied token "DUSK token" contracts/token/HypERC20.sol:HypERC20 "$init" \
        --constructor-args 9 1 1 "$EVM_MAILBOX"
    state_set --arg o "$deployer" '.evm.owner = $o'
    ok "EVM DUSK token $(state_get '.evm.token')"
}

# A token deployed before the route had its own hook dispatches through the
# Mailbox's default hook, which pays nothing to our IGP.
set_evm_route_hook() {
    local token hook
    token="$(state_get '.evm.token')" hook="$(state_get '.evm.route_hook')"
    if [ "$(lower "$(evm_call "$token" "hook()(address)")")" != "$(lower "$hook")" ]; then
        info "Setting the EVM route's hook to $hook..."
        evm_send "$token" "setHook(address)" "$hook"
    fi
    check_evm_route_hook
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
            --warp-contract "$route" --domain "$EVM_DOMAIN" --router "$(pad32 "$token")" \
            --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID" >/dev/null \
            || fail "Dusk router enrollment failed"
    fi
    enrolled="$(dusk_query --contract "$route" --method enrolled_router --return-type bytes32 \
        --arg-u32 "$EVM_DOMAIN" | jq -r '.value')"
    [ "$(lower "$enrolled")" = "$(pad32 "$token")" ] || fail "Dusk route does not have the EVM token enrolled"
    ok "Routes enrolled with each other"
}

# A state file from another network, or from before a network reset, would
# make `deploy` skip steps whose contracts don't exist. One whose routes are
# older than this checkout's would reuse contracts with known bugs.
check_state_matches_chain() {
    local mailbox name contract domain
    [ -f "$STATE" ] || return 0
    [ "$(lower "$(state_get '.evm.mailbox')")" = "" ] || [ "$(lower "$(state_get '.evm.mailbox')")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "$STATE was made for EVM Mailbox $(state_get '.evm.mailbox'), not $EVM_MAILBOX. Use another DEPLOY_NAME or move it aside."
    mailbox="$(state_get '.dusk.mailbox')"
    if [ -n "$mailbox" ]; then
        domain="$(dusk_query --contract "$mailbox" --method local_domain --return-type u32 2>/dev/null | jq -r '.value // empty')" || true
        [ "$domain" = "$DUSK_DOMAIN" ] \
            || fail "$STATE records Dusk Mailbox $mailbox, but it isn't on this chain with domain $DUSK_DOMAIN. The network was reset or the state is from elsewhere; move it aside and redeploy."
    fi
    check_state_versions
    for name in token storage_gas_oracle route_hook aggregation_hook_factory; do
        contract="$(state_get ".evm.$name")"
        [ -z "$contract" ] || evm_has_code "$contract" \
            || fail "$STATE records EVM $name $contract, but there is no contract there; move the state aside and redeploy."
    done
    contract="$(state_get '.evm.igp')"
    if [ -n "$contract" ] && [ -n "$(state_get '.evm.storage_gas_oracle')" ]; then
        evm_has_code "$contract" \
            || fail "$STATE records EVM igp $contract, but there is no contract there; move the state aside and redeploy."
    fi
    check_evm_creations
}

# Refuse a deployment whose core contracts or warp routes report another
# state_version than this checkout's (CORE_STATE_VERSIONS,
# ROUTE_STATE_VERSIONS).
check_state_versions() {
    local entry name expected contract version kind older
    for entry in $CORE_STATE_VERSIONS $ROUTE_STATE_VERSIONS; do
        name="${entry%%=*}" expected="${entry#*=}"
        contract="$(state_get ".dusk.$name")"
        [ -n "$contract" ] || continue
        if [[ " $ROUTE_STATE_VERSIONS " == *" $entry "* ]]; then
            kind="route"
            older="Older routes have bugs this checkout fixes (docs/security.md, CRITICAL-1 and MEDIUM-13)."
        elif [ "$name" = igp ]; then
            kind="contract"
            older="An IGP before it takes no top-ups (pay_for_gas), so a message that paid it too little is never relayed (docs/operations.md)."
        else
            kind="contract"
            older="Contracts before it keep storage anyone can fill, and this checkout's agents don't read them (docs/storage.md)."
        fi
        version="$(dusk_query --contract "$contract" --method state_version --return-type u32 2>/dev/null \
            | jq -r '.value // empty')" || true
        [[ "$version" =~ ^[0-9]+$ ]] \
            || fail "Can't read the state version of the Dusk $kind ($name) $contract recorded in $STATE. If the network was reset, move $OUT_DIR aside and deploy again."
        [ "$version" -ge "$expected" ] \
            || fail "$STATE records a Dusk $kind ($name) at state version $version; this checkout's is $expected. $older Move $OUT_DIR aside and redeploy."
        [ "$version" -eq "$expected" ] \
            || fail "$STATE records a Dusk $kind ($name) at state version $version, newer than this checkout's $expected. Update the checkout."
    done
}

cmd_deploy() {
    cmd_preflight
    state_set --arg n "$EVM_CHAIN_NAME" --argjson d "$EVM_DOMAIN" --arg m "$EVM_MAILBOX" \
        --arg h "$EVM_MERKLE_TREE_HOOK" --arg va "$EVM_VALIDATOR_ANNOUNCE" \
        '.evm_chain = $n | .evm_domain = $d | .evm.mailbox = $m | .evm.merkle_tree_hook = $h
         | .evm.validator_announce = $va'
    deploy_dusk
    configure_dusk_beneficiary
    check_dusk_igp_pricing
    deploy_evm_ism
    deploy_evm_igp
    deploy_evm_route_hook
    deploy_evm_token
    set_evm_route_hook
    enroll_routers
    check_deployed_ism
    ok "Deployment complete: $STATE"
    info "Next: bash deploy/testnet.sh agent-configs"
}

# ── Pricing updates ──────────────────────────────────────────────────────────

# Print a pricing row: label, overhead, rate, gas price, quote.
pricing_row() { printf '  %-5s %14s %24s %16s   %s\n' "$@" >&2; }

# The Dusk IGP's pricing for the EVM chain on chain against the settings,
# with what a transfer pays under each. With --apply, the owner (DUSK_KEYS)
# sets the settings' pricing.
cmd_dusk_igp() {
    local apply="${1:-}" igp current chain_quote new out
    [ -z "$apply" ] || [ "$apply" = --apply ] || fail "usage: bash deploy/testnet.sh dusk-igp [--apply]"
    load_fees
    igp="$(state_get '.dusk.igp')"
    [ -n "$igp" ] || fail "No Dusk IGP recorded in $STATE; run deploy first"
    check_dusk_deployment
    check_dusk_igp_is_default_hook
    current="$(dusk_igp_pricing)" || exit 1
    chain_quote="none"
    if [ "$(jq '.gas_price != "0"' <<<"$current")" = true ]; then
        chain_quote="$(dusk_value "the Dusk IGP's quote" --contract "$igp" --method quote_gas_payment \
            --return-type u64 --arg-u32 "$EVM_DOMAIN" --arg-u64 "$DUSK_IGP_DEFAULT_GAS")" || exit 1
    fi
    new="$(fee '.dusk_igp | {gas_overhead, token_exchange_rate, gas_price}')"
    info "Dusk IGP $igp, pricing for $EVM_CHAIN_NAME (domain $EVM_DOMAIN), and what a transfer pays ($DUSK_IGP_DEFAULT_GAS gas plus the overhead):"
    pricing_row "" "gas overhead" "token exchange rate" "gas price (wei)" "quote (LUX)"
    pricing_row now "$(jq -r .gas_overhead <<<"$current")" "$(jq -r .token_exchange_rate <<<"$current")" \
        "$(jq -r .gas_price <<<"$current")" "$chain_quote"
    pricing_row new "$(fee .dusk_igp.gas_overhead)" "$(fee .dusk_igp.token_exchange_rate)" \
        "$(fee .dusk_igp.gas_price)" "$(fee .dusk_to_evm.quote_lux)"
    if [ "$(jq -cS . <<<"$current")" = "$(jq -cS . <<<"$new")" ]; then
        ok "The Dusk IGP already charges what the settings say"
        return
    fi
    if [ "$apply" != --apply ]; then
        info "Dry run: nothing was sent. To set the new pricing: bash deploy/testnet.sh dusk-igp --apply"
        return
    fi
    require DUSK_KEYS DUSK_CONSENSUS_PASSWORD
    out="$(dusk_tx set-igp-config --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
        --igp "$igp" --igp-domain-config "$(fee '.dusk_igp.config')" \
        --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
        || fail "Can't set the Dusk IGP's pricing: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
    info "Sent $(jq -r .tx_id <<<"$out"), included at height $(jq -r .block_height <<<"$out")"
    chain_quote="$(dusk_value "the Dusk IGP's quote" --contract "$igp" --method quote_gas_payment \
        --return-type u64 --arg-u32 "$EVM_DOMAIN" --arg-u64 "$DUSK_IGP_DEFAULT_GAS")" || exit 1
    [ "$chain_quote" = "$(fee .dusk_to_evm.quote_lux)" ] \
        || fail "The Dusk IGP now quotes $chain_quote LUX, not $(fee .dusk_to_evm.quote_lux)"
    check_dusk_igp_pricing
    info "Regenerate the agent configs (agent-configs) if the margin changed: the relayer's requirement follows it."
}

# The EVM IGP's pricing for Dusk on chain against the settings. With --apply,
# the owner (EVM_DEPLOYER_KEY) sets the oracle's data and the IGP's overhead.
cmd_evm_igp() {
    local apply="${1:-}" igp oracle current chain_quote new token route_quote
    [ -z "$apply" ] || [ "$apply" = --apply ] || fail "usage: bash deploy/testnet.sh evm-igp [--apply]"
    load_fees
    igp="$(state_get '.evm.igp')" oracle="$(state_get '.evm.storage_gas_oracle')"
    [ -n "$igp" ] && [ -n "$oracle" ] || fail "No EVM IGP recorded in $STATE; run deploy first"
    check_evm_deployment
    current="$(evm_igp_pricing)" || exit 1
    check_evm_igp_oracle "$current"
    chain_quote="none"
    if [ "$(jq '.gas_price != "0" and .token_exchange_rate != "0"' <<<"$current")" = true ] \
        && [ "$(lower "$(jq -r .oracle <<<"$current")")" != "$ZERO_ADDRESS" ]; then
        # The settings can't undo a quote that overrides the oracle.
        check_evm_igp_quotes "$current"
        chain_quote="$(evm_number "$igp" "quoteGasPayment(uint32,uint256)(uint256)" "$DUSK_DOMAIN" \
            "$(jq -r .gas_overhead <<<"$current")")" || fail "Can't read the EVM IGP's quote"
    fi
    new="$(fee '.evm_igp | {gas_overhead, token_exchange_rate, gas_price}')"
    info "EVM IGP $igp, pricing for Dusk (domain $DUSK_DOMAIN) through oracle $oracle, and what a transfer pays (the overhead's gas):"
    pricing_row "" "gas overhead" "token exchange rate" "gas price (LUX)" "quote (wei)"
    pricing_row now "$(jq -r .gas_overhead <<<"$current")" "$(jq -r .token_exchange_rate <<<"$current")" \
        "$(jq -r .gas_price <<<"$current")" "$chain_quote"
    pricing_row new "$(fee .evm_igp.gas_overhead)" "$(fee .evm_igp.token_exchange_rate)" \
        "$(fee .evm_igp.gas_price)" "$(fee .evm_to_dusk.quote_wei)"
    if [ "$(jq -cS 'del(.oracle)' <<<"$current")" = "$(jq -cS . <<<"$new")" ] \
        && [ "$(lower "$(jq -r .oracle <<<"$current")")" = "$(lower "$oracle")" ]; then
        ok "The EVM IGP already charges what the settings say"
        return
    fi
    if [ "$apply" != --apply ]; then
        info "Dry run: nothing was sent. To set the new pricing: bash deploy/testnet.sh evm-igp --apply"
        return
    fi
    require EVM_DEPLOYER_KEY
    configure_evm_igp_pricing apply
    check_evm_igp_pricing
    token="$(state_get '.evm.token')"
    if [ -n "$token" ]; then
        route_quote="$(evm_number "$token" "quoteGasPayment(uint32)(uint256)" "$DUSK_DOMAIN")" \
            || fail "Can't read the EVM route's quote"
        ok "A transfer to Dusk now pays $route_quote wei (quoteGasPayment on the route, the required hook's fee included)"
    fi
    info "Regenerate the agent configs (agent-configs) if the overhead changed: the relayer's requirement follows it."
}

# ── Revenue ──────────────────────────────────────────────────────────────────

# What the fee contracts hold for their beneficiaries.
cmd_revenue() {
    local name contract claimable beneficiary igp balance
    [ -f "$STATE" ] || fail "Run deploy first"
    igp="$(state_get '.evm.igp')"
    [ -z "$igp" ] || [ -z "$(state_get '.evm.storage_gas_oracle')" ] || check_evm_chain
    check_dusk_chain
    for name in igp protocol_fee; do
        contract="$(state_get ".dusk.$name")"
        [ -n "$contract" ] || continue
        claimable="$(dusk_value "the Dusk $name's claimable fees" --contract "$contract" \
            --method claimable_fees --return-type u64)" || exit 1
        beneficiary="$(dusk_value "the Dusk $name's beneficiary" --contract "$contract" \
            --method beneficiary --return-type bytes32)" || exit 1
        info "Dusk $name $contract holds $claimable LUX for the account with hash $beneficiary"
    done
    if [ -n "$igp" ] && [ -n "$(state_get '.evm.storage_gas_oracle')" ]; then
        balance="$(cast balance "$igp" --rpc-url "$EVM_RPC")"
        beneficiary="$(evm_call "$igp" "beneficiary()(address)")"
        info "EVM IGP $igp holds $balance wei for $beneficiary"
    fi
    info "Claim with: bash deploy/testnet.sh claim (docs/operations.md, Collecting the fees)"
}

# Claim on both chains. On the EVM chain anyone may call claim(), which pays
# the IGP's balance to its beneficiary. On Dusk only the beneficiary can
# claim, by signing the claim: DUSK_KEYS here, unless DUSK_FEE_BENEFICIARY is
# someone else, who then runs the dusk-tx command printed. Both chains and
# the recorded contracts are checked before anything is sent on either.
cmd_claim() {
    local name contract claimable beneficiary signer out igp balance
    [ -f "$STATE" ] || fail "Run deploy first"
    igp="$(state_get '.evm.igp')"
    [ -z "$igp" ] || [ -z "$(state_get '.evm.storage_gas_oracle')" ] || check_evm_deployment
    check_dusk_deployment
    if [ -n "$igp" ] && [ -n "$(state_get '.evm.storage_gas_oracle')" ]; then
        balance="$(cast balance "$igp" --rpc-url "$EVM_RPC")"
        if [ "$balance" != 0 ]; then
            require EVM_DEPLOYER_KEY
            evm_send "$igp" "claim()"
            ok "Claimed $balance wei from the EVM IGP $igp for $(evm_call "$igp" "beneficiary()(address)")"
        else
            info "The EVM IGP $igp holds nothing"
        fi
    fi
    require DUSK_KEYS DUSK_CONSENSUS_PASSWORD
    signer="$(dusk_tx account --offline --keys "$DUSK_KEYS" | jq -r '.account_h256')" \
        || fail "Can't read DUSK_KEYS"
    for name in igp protocol_fee; do
        contract="$(state_get ".dusk.$name")"
        [ -n "$contract" ] || continue
        claimable="$(dusk_value "the Dusk $name's claimable fees" --contract "$contract" \
            --method claimable_fees --return-type u64)" || exit 1
        if [ "$claimable" = 0 ]; then
            info "The Dusk $name $contract holds nothing"
            continue
        fi
        beneficiary="$(dusk_value "the Dusk $name's beneficiary" --contract "$contract" \
            --method beneficiary --return-type bytes32)" || exit 1
        if [ "$beneficiary" != "$signer" ]; then
            warn "The Dusk $name $contract holds $claimable LUX for the account with hash $beneficiary, not DUSK_KEYS. Its beneficiary claims with: dusk-tx claim-fees --rues-url $DUSK_RUES_URL --keys <beneficiary keys> --contract $contract --expected-chain-id $DUSK_EXPECTED_CHAIN_ID"
            continue
        fi
        out="$(dusk_tx claim-fees --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" --contract "$contract" \
            --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
            || fail "Can't claim from the Dusk $name: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
        ok "Claimed $(jq -r .claimed_dusk <<<"$out") DUSK from the Dusk $name $contract ($(jq -r .tx_id <<<"$out"))"
    done
}

# ── Agent configs ────────────────────────────────────────────────────────────

# The Dusk chain of an agent config. The relayer's has a maxBatchSize: it
# sends that many deliveries to Dusk together, as consecutive transactions
# of its account, with a dusk-tx that takes `call --nonce --no-wait`. Both
# batch their contract reads through Multicall for Dusk when it answers
# (check_dusk_multicall).
dusk_chain_json() {
    local cursor_dir="$1" gas_limit="$2" max_batch_size="${3:-null}"
    jq -n --arg url "$DUSK_RUES_URL" --argjson domain "$DUSK_DOMAIN" \
        --argjson chain "$DUSK_EXPECTED_CHAIN_ID" --arg cursors "$cursor_dir" \
        --argjson gl "$gas_limit" --argjson gp "$DUSK_AGENT_GAS_PRICE" \
        --argjson batch "$max_batch_size" --arg multicall "$DUSK_MULTICALL_ID" \
        --arg key_file "$DUSK_SIGNER_KEY_FILE" --slurpfile s "$STATE" '{
          name: "dusk", protocol: "dusk", domainId: $domain, chainId: $chain,
          rpcUrls: [{http: $url}], eventCursorDir: $cursors,
          gasLimit: $gl, gasPrice: $gp,
          mailbox: ("0x" + $s[0].dusk.mailbox),
          interchainGasPaymaster: ("0x" + $s[0].dusk.igp),
          validatorAnnounce: ("0x" + $s[0].dusk.validator_announce),
          merkleTreeHook: ("0x" + $s[0].dusk.merkle_tree_hook),
          submitter: "Classic",
          signer: {type: "duskKey", keyFile: $key_file}
        } + (if $batch == null then {} else {maxBatchSize: $batch} end)
          + (if $multicall == "" then {} else {batchContractAddress: ("0x" + $multicall)} end)'
}

cmd_agent_configs() {
    [ -f "$STATE" ] && [ -n "$(state_get '.evm.token')" ] || fail "Run deploy first"
    [ -x "$DUSK_TX" ] || fail "dusk-tx not built at $DUSK_TX (run: make dusk-tx)"
    load_fees
    check_relayer_batch_size
    check_dusk_deployment
    check_evm_deployment
    check_dusk_multicall
    check_deployed_ism
    check_relayer_gas_limit
    route_hook_plan
    check_dusk_igp_pricing
    [ -n "$(state_get '.evm.storage_gas_oracle')" ] \
        || fail "No EVM IGP for Dusk recorded in $STATE: run deploy, which deploys one"
    check_evm_igp_pricing
    check_evm_route_hook
    local evm_chain
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

    # The relayer relays any message between the two chains (no whitelist),
    # once it is paid for: a message from each origin must have paid that
    # chain's IGP, the one this deployment owns, for enough gas
    # (onChainFeeQuoting; docs/costs.md#fee-coverage). A message from another
    # origin matches no policy and is not relayed.
    jq -n --arg evm "$EVM_CHAIN_NAME" --argjson evm_chain "$evm_chain" \
        --argjson dusk_chain "$(dusk_chain_json "$AGENT_STATE_DIR/relayer-dusk-events" "$DUSK_RELAYER_GAS_LIMIT" \
            "$DUSK_RELAYER_MAX_BATCH_SIZE")" \
        --argjson ed "$EVM_DOMAIN" --argjson dd "$DUSK_DOMAIN" \
        --arg dusk_fraction "$(fee .relayer.dusk_origin_gas_fraction)" \
        --arg evm_fraction "$(fee .relayer.evm_origin_gas_fraction)" \
        --arg db "$AGENT_STATE_DIR/relayer-db" '{
          db: $db,
          relayChains: ("dusk," + $evm),
          gasPaymentEnforcement: [
            {type: "onChainFeeQuoting", gasFraction: $dusk_fraction, matchingList: [{originDomain: $dd}]},
            {type: "onChainFeeQuoting", gasFraction: $evm_fraction, matchingList: [{originDomain: $ed}]}
          ],
          igpIndexingEnabled: true,
          chains: {($evm): $evm_chain, dusk: $dusk_chain},
          metricsPort: 9090
        }' > "$OUT_DIR/relayer.json"

    # One Dusk-origin validator. Each validator operator copies this, sets its
    # own signing key and checkpoint bucket, and runs it on its own host.
    jq -n --argjson dusk_chain "$(dusk_chain_json "$AGENT_STATE_DIR/validator-dusk-events" "$DUSK_AGENT_GAS_LIMIT")" \
        --arg db "$AGENT_STATE_DIR/validator-db" '{
          db: $db,
          originChainName: "dusk",
          interval: 5,
          checkpointSyncer: {type: "s3", bucket: "<bucket>", region: "<region>"},
          chains: {dusk: $dusk_chain},
          metricsPort: 9091
        }' > "$OUT_DIR/validator-dusk.json"

    ok "Wrote $OUT_DIR/relayer.json and $OUT_DIR/validator-dusk.json"
    ok "The relayer sends up to $DUSK_RELAYER_MAX_BATCH_SIZE deliveries to Dusk together, as consecutive transactions (maxBatchSize)"
    ok "The relayer relays a Dusk-origin message once its Dusk IGP payment covers $(fee .relayer.dusk_origin_gas_fraction) of the gas it estimates for $EVM_CHAIN_NAME, and an $EVM_CHAIN_NAME-origin one once it paid the EVM IGP $(state_get '.evm.igp') for $(fee .relayer.evm_origin_required_gas) Dusk gas"
    cat >&2 <<EOF
Secrets are not in these files. Provide them when starting the agents:
  relayer:   HYP_CHAINS_$(tr '[:lower:]' '[:upper:]' <<<"$EVM_CHAIN_NAME")_SIGNER_KEY=0x...  (funded on $EVM_CHAIN_NAME)
             a Dusk key file at $DUSK_SIGNER_KEY_FILE (funded on Dusk)
  validator: HYP_VALIDATOR_KEY=0x...  (one of DUSK_ORIGIN_VALIDATORS)
             a Dusk key file at $DUSK_SIGNER_KEY_FILE (pays for its announcement)
             AWS credentials for the checkpoint bucket
Write a Dusk key file from a rusk-wallet export with:
  dusk-tx export-signer-key --keys <consensus.keys> --out $DUSK_SIGNER_KEY_FILE
Run the agents from hyperlane-monorepo/rust/main with DUSK_TX_BIN pointing at
dusk-tx. See docs/testnet.md.
EOF
}

case "${1:-}" in
    preflight) cmd_preflight ;;
    deploy) cmd_deploy ;;
    agent-configs) cmd_agent_configs ;;
    fees) cmd_fees "${2:-}" ;;
    dusk-igp) cmd_dusk_igp "${2:-}" ;;
    evm-igp) cmd_evm_igp "${2:-}" ;;
    revenue) cmd_revenue ;;
    claim) cmd_claim ;;
    *) fail "usage: bash deploy/testnet.sh preflight|deploy|agent-configs|fees [--json]|dusk-igp [--apply]|evm-igp [--apply]|revenue|claim" ;;
esac
