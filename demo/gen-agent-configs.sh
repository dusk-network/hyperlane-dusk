#!/usr/bin/env bash
# =============================================================================
# Generate Hyperlane Agent Configs (Relayer + Validator) for the Demo Environment
# =============================================================================
#
# Reads the combined deployment state from /tmp/hyperlane-bridge-state.json and
# emits config JSON files for the Hyperlane relayer (and validator for multisig).
#
# Usage:
#   bash demo/gen-agent-configs.sh --ism testMock
#   bash demo/gen-agent-configs.sh --ism messageIdMultisig
#
# Output:
#   Prints a small JSON object with the generated config file paths.
#

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Helpers ──────────────────────────────────────────────────────────────────

fail() { echo "[FAIL] $*" >&2; exit 1; }

if [ "${AGENT_CONFIG_VALIDATE_ONLY:-0}" = "1" ]; then
    [ -n "${BRIDGE_STATE_FILE:-}" ] \
        || fail "BRIDGE_STATE_FILE is required for validation-only mode"
else
    [ -f "$SCRIPT_DIR/.env.bridge" ] \
        || fail "Environment file not found at $SCRIPT_DIR/.env.bridge"
    source "$SCRIPT_DIR/.env.bridge"
    command -v cast >/dev/null 2>&1 || fail "cast is required for validator identity checks"
fi

# ── Args ────────────────────────────────────────────────────────────────────

ISM="testMock"
RUN_ID=""

while [ "$#" -gt 0 ]; do
    case "$1" in
        --ism)
            ISM="${2:-}"; shift 2 ;;
        --run-id)
            RUN_ID="${2:-}"; shift 2 ;;
        *)
            fail "Unknown argument: $1" ;;
    esac
done

if [ "$ISM" != "testMock" ] && [ "$ISM" != "messageIdMultisig" ]; then
    fail "Invalid --ism '$ISM' (expected: testMock or messageIdMultisig)"
fi

STATE_FILE="$BRIDGE_STATE_FILE"
[ -f "$STATE_FILE" ] || fail "State file not found at $STATE_FILE (run: bash demo/deploy.sh)"

# Reserve one exclusive invocation directory before consuming deployment
# state. The validated snapshot, signer, configs, databases, and checkpoints
# all share this ownership boundary; a same-second or caller-chosen collision
# fails instead of reusing another run's paths.
if [ -n "$RUN_ID" ]; then
    [[ "$RUN_ID" =~ ^[A-Za-z0-9._-]+$ ]] || fail "Invalid --run-id '$RUN_ID'"
    RUN_DIR="/tmp/hyperlane-agent-${ISM}-${RUN_ID}"
    mkdir "$RUN_DIR" || fail "Agent run directory already exists: $RUN_DIR"
else
    RUN_DIR="$(mktemp -d -t "hyperlane-agent-${ISM}.XXXXXXXX")" \
        || fail "Cannot reserve an agent run directory"
    RUN_ID="${RUN_DIR##*.}"
fi
printf 'pid=%s\nism=%s\n' "$$" "$ISM" > "$RUN_DIR/.hyperlane-owner"

GENERATION_COMMITTED=0
cleanup_uncommitted_generation() {
    if [ "$GENERATION_COMMITTED" -eq 0 ]; then
        rm -rf -- "$RUN_DIR" 2>/dev/null || true
    fi
}
trap cleanup_uncommitted_generation EXIT

STATE_SNAPSHOT="$RUN_DIR/deployment.json"
source_hash_before="$(sha256sum "$STATE_FILE" | awk '{print $1}')" \
    || fail "Cannot hash deployment state before snapshot"
cp -- "$STATE_FILE" "$RUN_DIR/deployment.json.pending" \
    || fail "Cannot snapshot deployment state"
source_hash_after="$(sha256sum "$STATE_FILE" | awk '{print $1}')" \
    || fail "Cannot hash deployment state after snapshot"
snapshot_hash="$(sha256sum "$RUN_DIR/deployment.json.pending" | awk '{print $1}')" \
    || fail "Cannot hash deployment snapshot"
[ "$source_hash_before" = "$source_hash_after" ] && [ "$source_hash_after" = "$snapshot_hash" ] \
    || fail "Deployment state changed while it was being snapshotted"
mv -- "$RUN_DIR/deployment.json.pending" "$STATE_SNAPSHOT"
STATE_FILE="$STATE_SNAPSHOT"

# A saved manifest is not sufficient authority for live agent configuration:
# Mailbox policy and chain state can change after the file is written. Reuse
# the canonical, fail-closed deployment validator before reading any signer
# material or writing configuration. Validation-only mode remains hermetic for
# the repository's fixture tests and never emits operational config.
if [ "${AGENT_CONFIG_VALIDATE_ONLY:-0}" != "1" ]; then
    BRIDGE_STATE_FILE="$STATE_FILE" bash "$SCRIPT_DIR/deploy.sh" --skip-deploy >/dev/null \
        || fail "Live deployment validation failed; refusing to generate agent configuration"
fi

# ── Extract Deployment State ────────────────────────────────────────────────

EVM_MAILBOX="$(jq -r '.evm.mailbox' "$STATE_FILE")"
EVM_TOKEN="$(jq -r '.evm.token' "$STATE_FILE")"
EVM_IGP="$(jq -r '.evm.igp' "$STATE_FILE")"
EVM_VALIDATOR_ANNOUNCE="$(jq -r '.evm.validator_announce' "$STATE_FILE")"
EVM_MERKLE_TREE_HOOK="$(jq -r '.evm.merkle_tree_hook' "$STATE_FILE")"

DUSK_MAILBOX="$(jq -r '.dusk.mailbox' "$STATE_FILE")"
DUSK_IGP="$(jq -r '.dusk.igp' "$STATE_FILE")"
DUSK_VALIDATOR_ANNOUNCE="$(jq -r '.dusk.validator_announce' "$STATE_FILE")"
DUSK_MERKLE_TREE_HOOK="$(jq -r '.dusk.merkle_tree_hook' "$STATE_FILE")"

EVM_DOMAIN="$(jq -r '.evm_domain' "$STATE_FILE")"
DUSK_DOMAIN="$(jq -r '.dusk_domain' "$STATE_FILE")"
DUSK_DEFAULT_ISM="$(jq -er '.dusk_default_ism | strings' "$STATE_FILE")" \
    || fail "Deployment state lacks dusk_default_ism; redeploy"
[ "$DUSK_DEFAULT_ISM" = "$ISM" ] \
    || fail "Requested --ism $ISM does not match deployed Dusk Mailbox policy $DUSK_DEFAULT_ISM"

DUSK_TEST_MOCK="$(jq -er '.dusk.test_mock | strings | select(length == 64)' "$STATE_FILE")" \
    || fail "Deployment state lacks Dusk TestMock contract ID; redeploy"
DUSK_ISM_MULTISIG="$(jq -er '.dusk.ism_multisig // ""' "$STATE_FILE")"
DUSK_DEFAULT_ISM_ID="$(jq -er '.dusk.default_ism | strings | select(length == 64)' "$STATE_FILE")" \
    || fail "Deployment state lacks Dusk default ISM contract ID; redeploy"
if [ "$ISM" = "messageIdMultisig" ]; then
    [ -n "$DUSK_ISM_MULTISIG" ] && [ "$DUSK_ISM_MULTISIG" != "null" ] \
        || fail "Multisig agent mode requires a deployed Dusk multisig ISM"
    [ "${DUSK_DEFAULT_ISM_ID,,}" = "${DUSK_ISM_MULTISIG,,}" ] \
        || fail "Deployed Dusk Mailbox policy does not match the multisig ISM"
else
    [ -z "$DUSK_ISM_MULTISIG" ] || fail "TestMock deployment unexpectedly records a multisig ISM"
    [ "${DUSK_DEFAULT_ISM_ID,,}" = "${DUSK_TEST_MOCK,,}" ] \
        || fail "Deployed Dusk Mailbox policy does not match TestMock"
fi

# ChainId is persisted as the canonical one-byte Moonlight value in hex.
DUSK_CHAIN_ID_HEX="$(jq -er '.dusk_chain_id | strings | select(test("^[0-9A-Fa-f]{2}$"))' "$STATE_FILE")" \
    || fail "Deployment state has an invalid Dusk chain ID; redeploy"
DUSK_CHAIN_ID=$((16#$DUSK_CHAIN_ID_HEX))

if [ "${AGENT_CONFIG_VALIDATE_ONLY:-0}" = "1" ]; then
    jq -n \
        --arg ism "$ISM" \
        --arg default_ism "$DUSK_DEFAULT_ISM_ID" \
        --argjson chain_id "$DUSK_CHAIN_ID" \
        '{valid: true, ism: $ism, defaultIsm: $default_ism, chainId: $chain_id}'
    rm -rf -- "$RUN_DIR"
    GENERATION_COMMITTED=1
    exit 0
fi

# Gas settings for Dusk tx submission (passed through to dusk-tx). Calls
# only need a gas price above zero; 2000 is the minimum for deploys only.
# The relayer's `process` costs about 1.95M gas plus 0.22M per validator
# signature, so 5M covers the e2e thresholds (up to 5). A failed transaction
# pays its whole limit, for example when a message was already delivered, so
# the relayer's limit stays low. The validators' covers their announcement.
DUSK_RELAYER_GAS_LIMIT="${DUSK_RELAYER_GAS_LIMIT:-5000000}"
DUSK_GAS_LIMIT="${DUSK_GAS_LIMIT:-30000000}"
DUSK_GAS_PRICE="${DUSK_GAS_PRICE:-1}"
# How many deliveries to Dusk the relayer sends together, as consecutive
# transactions of its account that can share a block (maxBatchSize). At most
# 64, the most of an account's transactions Rusk queues ahead of its next
# nonce; deploy/testnet.env.example has why 32.
DUSK_RELAYER_MAX_BATCH_SIZE="${DUSK_RELAYER_MAX_BATCH_SIZE:-32}"
[[ "$DUSK_RELAYER_MAX_BATCH_SIZE" =~ ^[1-9][0-9]*$ ]] && [ "$DUSK_RELAYER_MAX_BATCH_SIZE" -le 64 ] \
    || fail "DUSK_RELAYER_MAX_BATCH_SIZE must be a whole number from 1 to 64, not '$DUSK_RELAYER_MAX_BATCH_SIZE'"

# The relayer relays any message once it is paid for, with the policies
# deploy/testnet.sh writes (docs/costs.md#fee-coverage): a Dusk-origin
# message's payment to the Dusk IGP must cover 2/3 of the EVM gas the relayer
# estimates for it (the default 50% margin), and an EVM-origin message must
# have paid the EVM IGP for the overhead demo/deploy.sh configured, as a
# fraction of the relayer's gas limit, which is hyperlane-dusk's estimate.
EVM_IGP_DUSK_GAS_OVERHEAD="$(jq -er '.evm.igp_dusk_gas_overhead | numbers' "$STATE_FILE")" \
    || fail "Deployment state lacks the EVM IGP's pricing for Dusk; redeploy"
[ "$EVM_IGP_DUSK_GAS_OVERHEAD" -le "$DUSK_RELAYER_GAS_LIMIT" ] \
    || fail "The EVM IGP's overhead for Dusk ($EVM_IGP_DUSK_GAS_OVERHEAD) is above DUSK_RELAYER_GAS_LIMIT"
DUSK_ORIGIN_GAS_FRACTION="2/3"
EVM_ORIGIN_GAS_FRACTION="$EVM_IGP_DUSK_GAS_OVERHEAD/$DUSK_RELAYER_GAS_LIMIT"

# Keep the EVM relayer submitter and validator identities independently
# configurable. Existing callers remain compatible, while the live E2E can
# isolate them from the operator account and from each other to avoid nonce
# races between concurrently running processes.
#
# ANVIL_VALIDATOR_PRIVATE_KEYS (comma-separated) runs a validator set: one
# validator per key for each origin chain. It defaults to the single
# ANVIL_VALIDATOR_PRIVATE_KEY.
ANVIL_RELAYER_PRIVATE_KEY="${ANVIL_RELAYER_PRIVATE_KEY:-$ANVIL_PRIVATE_KEY}"
ANVIL_VALIDATOR_PRIVATE_KEY="${ANVIL_VALIDATOR_PRIVATE_KEY:-$ANVIL_PRIVATE_KEY}"
ANVIL_VALIDATOR_PRIVATE_KEYS="${ANVIL_VALIDATOR_PRIVATE_KEYS:-$ANVIL_VALIDATOR_PRIVATE_KEY}"
IFS=',' read -r -a VALIDATOR_KEYS <<<"$ANVIL_VALIDATOR_PRIVATE_KEYS"
[ "${#VALIDATOR_KEYS[@]}" -ge 1 ] && [ "${#VALIDATOR_KEYS[@]}" -le 8 ] \
    || fail "ANVIL_VALIDATOR_PRIVATE_KEYS must list 1 to 8 keys"
if [ "$ISM" = "messageIdMultisig" ]; then
    DUSK_MULTISIG_POLICY="$(jq -ce '.dusk.multisig_policy | {
        validators: ([.validators[] | ascii_downcase] | sort),
        threshold: (.threshold | tonumber)
    }' "$STATE_FILE")" \
        || fail "Deployment state lacks the validated Dusk multisig policy"
    DUSK_MULTISIG_THRESHOLD="$(jq -er '.threshold' <<<"$DUSK_MULTISIG_POLICY")"
    [ "${#VALIDATOR_KEYS[@]}" -ge "$DUSK_MULTISIG_THRESHOLD" ] \
        || fail "Configured ${#VALIDATOR_KEYS[@]} validator key(s), fewer than the live Dusk multisig threshold $DUSK_MULTISIG_THRESHOLD"
    for key in "${VALIDATOR_KEYS[@]}"; do
        GENERATED_VALIDATOR_ADDRESS="$(cast wallet address --private-key "$key" | tr '[:upper:]' '[:lower:]')" \
            || fail "Cannot derive a configured validator address"
        jq -e --arg validator "$GENERATED_VALIDATOR_ADDRESS" \
            '.validators | index($validator) != null' <<<"$DUSK_MULTISIG_POLICY" >/dev/null \
            || fail "Configured validator $GENERATED_VALIDATOR_ADDRESS is not authorized by the live Dusk multisig policy"
    done
fi

# Multicall for Dusk, through which the agents batch their contract reads
# (batchContractAddress): the one demo/deploy.sh recorded, or the ID it has
# on every chain. Included only when it answers `version` with 1 on this
# chain, as deploy/testnet.sh checks it; otherwise the agents make single
# queries. DUSK_MULTICALL= leaves it out.
DUSK_MULTICALL="${DUSK_MULTICALL-$(jq -r '.dusk.multicall // "93de287b7d349d678a0038fe7772e8960e1a84a1b5da2d0b1f62f532616ce1d4"' "$STATE_FILE")}"
DUSK_MULTICALL_JSON=""
MULTICALL_ID="$(printf '%s' "${DUSK_MULTICALL#0x}" | tr '[:upper:]' '[:lower:]')"
if [ -n "$MULTICALL_ID" ]; then
    [[ "$MULTICALL_ID" =~ ^[0-9a-f]{64}$ ]] || fail "DUSK_MULTICALL must be a contract ID of 64 hex characters"
    MULTICALL_VERSION="$("$DUSK_TX" query --rues-url "$DUSK_RUES_URL" --contract "$MULTICALL_ID" \
        --method version --return-type u32 2>/dev/null | jq -r '.value // empty' 2>/dev/null)" || true
    if [ "$MULTICALL_VERSION" = 1 ]; then
        DUSK_MULTICALL_JSON="\"batchContractAddress\": \"0x${MULTICALL_ID}\","
    else
        echo "[WARN] Multicall for Dusk doesn't answer version 1 at $MULTICALL_ID on this chain: the agent configs leave batchContractAddress out, and the agents make single contract queries" >&2
    fi
fi

# ── Dusk Deployer Key (consensus.keys) ──────────────────────────────────────

[ -f "$CONSENSUS_KEYS" ] || fail "Consensus keys not found at $CONSENSUS_KEYS"
[ -x "$DUSK_TX" ] || fail "dusk-tx not found at $DUSK_TX (run: make dusk-tx)"

# ── Output Paths ────────────────────────────────────────────────────────────

RELAYER_DB="$RUN_DIR/db-relayer"
VALIDATOR_DB="$RUN_DIR/db-validator-anvil"
VALIDATOR_DUSK_DB="$RUN_DIR/db-validator-dusk"
CHECKPOINT_DIR="$RUN_DIR/checkpoints-anvil"
CHECKPOINT_DUSK_DIR="$RUN_DIR/checkpoints-dusk"
DUSK_RELAYER_EVENT_CURSOR_DIR="$RUN_DIR/event-cursors-relayer-dusk"
DUSK_VALIDATOR_EVENT_CURSOR_DIR="$RUN_DIR/event-cursors-validator-dusk"

RELAYER_CONFIG="$RUN_DIR/relayer.json"
VALIDATOR_CONFIG="$RUN_DIR/validator-anvil.json"
VALIDATOR_DUSK_CONFIG="$RUN_DIR/validator-dusk.json"
DUSK_SIGNER_KEY_FILE="$RUN_DIR/dusk-signer.key"

# dusk-tx decrypts the consensus keys and writes the agents' key file (mode
# 0600, never over an existing file), as deploy/local-agents.sh does. The
# password reaches it as data in its environment, as in the other demo
# scripts, never as code or an argument.
export_result="$(DUSK_CONSENSUS_PASSWORD="$CONSENSUS_PASSWORD" \
    "$DUSK_TX" export-signer-key --keys "$CONSENSUS_KEYS" --out "$DUSK_SIGNER_KEY_FILE")" \
    || fail "Cannot export the Dusk signer key from $CONSENSUS_KEYS: $(jq -r '.error // "dusk-tx failed"' <<<"$export_result" 2>/dev/null)"

# ── Write Relayer Config ────────────────────────────────────────────────────

cat > "$RELAYER_CONFIG" <<JSON
{
  "metricsPort": 19092,
  "log": { "level": "debug", "format": "pretty" },
  "db": "${RELAYER_DB}",
  "relayChains": "anvil,dusk",
  "allowLocalCheckpointSyncers": true,
  "txIdIndexingEnabled": false,
  "igpIndexingEnabled": true,
  "gasPaymentEnforcement": [
    { "type": "onChainFeeQuoting", "gasFraction": "${DUSK_ORIGIN_GAS_FRACTION}", "matchingList": [{ "originDomain": ${DUSK_DOMAIN} }] },
    { "type": "onChainFeeQuoting", "gasFraction": "${EVM_ORIGIN_GAS_FRACTION}", "matchingList": [{ "originDomain": ${EVM_DOMAIN} }] }
  ],
  "chains": {
    "anvil": {
      "name": "anvil",
      "domainId": ${EVM_DOMAIN},
      "chainId": ${ANVIL_CHAIN_ID},
      "protocol": "ethereum",
      "rpcUrls": [{ "http": "${ANVIL_RPC}" }],
      "blocks": { "reorgPeriod": 0, "estimateBlockTime": 1 },
      "mailbox": "${EVM_MAILBOX}",
      "interchainGasPaymaster": "${EVM_IGP}",
      "validatorAnnounce": "${EVM_VALIDATOR_ANNOUNCE}",
      "merkleTreeHook": "${EVM_MERKLE_TREE_HOOK}",
      "submitter": "Classic",
      "signer": { "type": "hexKey", "key": "${ANVIL_RELAYER_PRIVATE_KEY}" }
    },
    "dusk": {
      "name": "dusk",
      "domainId": ${DUSK_DOMAIN},
      "chainId": ${DUSK_CHAIN_ID},
      "eventCursorDir": "${DUSK_RELAYER_EVENT_CURSOR_DIR}",
      "protocol": "dusk",
      "rpcUrls": [{ "http": "${DUSK_RUES_URL}" }],
      "gasLimit": ${DUSK_RELAYER_GAS_LIMIT},
      "gasPrice": ${DUSK_GAS_PRICE},
      "maxBatchSize": ${DUSK_RELAYER_MAX_BATCH_SIZE},
      ${DUSK_MULTICALL_JSON}
      "mailbox": "0x${DUSK_MAILBOX}",
      "interchainGasPaymaster": "0x${DUSK_IGP}",
      "validatorAnnounce": "0x${DUSK_VALIDATOR_ANNOUNCE}",
      "merkleTreeHook": "0x${DUSK_MERKLE_TREE_HOOK}",
      "submitter": "Classic",
      "signer": { "type": "duskKey", "keyFile": "${DUSK_SIGNER_KEY_FILE}" }
    }
  }
}
JSON

# ── Write Validator Configs (only for MessageIdMultisig) ────────────────────
#
# One validator per key and origin chain. Validator 0 keeps the historical
# file names; validator i > 0 gets an "-<i>" suffix and its own database,
# checkpoint directory, event cursors and metrics port.

VALIDATOR_CONFIGS=()
VALIDATOR_DUSK_CONFIGS=()

if [ "$ISM" = "messageIdMultisig" ]; then
    for i in "${!VALIDATOR_KEYS[@]}"; do
        key="${VALIDATOR_KEYS[$i]}"
        sfx=""
        [ "$i" -eq 0 ] || sfx="-$i"
        anvil_db="$VALIDATOR_DB$sfx"
        dusk_db="$VALIDATOR_DUSK_DB$sfx"
        anvil_ckpt="$CHECKPOINT_DIR$sfx"
        dusk_ckpt="$CHECKPOINT_DUSK_DIR$sfx"
        dusk_cursors="$DUSK_VALIDATOR_EVENT_CURSOR_DIR$sfx"
        anvil_cfg="$RUN_DIR/validator-anvil$sfx.json"
        dusk_cfg="$RUN_DIR/validator-dusk$sfx.json"
        anvil_port=$((19093 + 2 * i))
        dusk_port=$((19094 + 2 * i))
        mkdir -p "$anvil_ckpt" "$dusk_ckpt"

        cat > "$anvil_cfg" <<JSON
{
  "metricsPort": ${anvil_port},
  "log": { "level": "debug", "format": "pretty" },
  "db": "${anvil_db}",
  "originChainName": "anvil",
  "validator": { "type": "hexKey", "key": "${key}" },
  "checkpointSyncer": { "type": "localStorage", "path": "${anvil_ckpt}" },
  "interval": 2,
  "chains": {
    "anvil": {
      "name": "anvil",
      "domainId": ${EVM_DOMAIN},
      "chainId": ${ANVIL_CHAIN_ID},
      "protocol": "ethereum",
      "rpcUrls": [{ "http": "${ANVIL_RPC}", "public": false }],
      "blocks": { "reorgPeriod": 0, "estimateBlockTime": 1 },
      "mailbox": "${EVM_MAILBOX}",
      "interchainGasPaymaster": "${EVM_IGP}",
      "validatorAnnounce": "${EVM_VALIDATOR_ANNOUNCE}",
      "merkleTreeHook": "${EVM_MERKLE_TREE_HOOK}",
      "submitter": "Classic",
      "signer": { "type": "hexKey", "key": "${key}" }
    }
  }
}
JSON

        cat > "$dusk_cfg" <<JSON
{
  "metricsPort": ${dusk_port},
  "log": { "level": "debug", "format": "pretty" },
  "db": "${dusk_db}",
  "originChainName": "dusk",
  "validator": { "type": "hexKey", "key": "${key}" },
  "checkpointSyncer": { "type": "localStorage", "path": "${dusk_ckpt}" },
  "interval": 2,
  "chains": {
    "dusk": {
      "name": "dusk",
      "domainId": ${DUSK_DOMAIN},
      "chainId": ${DUSK_CHAIN_ID},
      "eventCursorDir": "${dusk_cursors}",
      "protocol": "dusk",
      "rpcUrls": [{ "http": "${DUSK_RUES_URL}" }],
      "gasLimit": ${DUSK_GAS_LIMIT},
      "gasPrice": ${DUSK_GAS_PRICE},
      ${DUSK_MULTICALL_JSON}
      "mailbox": "0x${DUSK_MAILBOX}",
      "interchainGasPaymaster": "0x${DUSK_IGP}",
      "validatorAnnounce": "0x${DUSK_VALIDATOR_ANNOUNCE}",
      "merkleTreeHook": "0x${DUSK_MERKLE_TREE_HOOK}",
      "submitter": "Classic",
      "signer": { "type": "duskKey", "keyFile": "${DUSK_SIGNER_KEY_FILE}" }
    }
  }
}
JSON
        VALIDATOR_CONFIGS+=("$anvil_cfg")
        VALIDATOR_DUSK_CONFIGS+=("$dusk_cfg")
    done
fi

# ── Print Paths as JSON ─────────────────────────────────────────────────────

if [ "$ISM" = "messageIdMultisig" ]; then
    output_json="$(jq -n \
      --arg relayer "$RELAYER_CONFIG" \
      --arg validator "$VALIDATOR_CONFIG" \
      --arg dusk_validator "$VALIDATOR_DUSK_CONFIG" \
      --arg dusk_signer_key_file "$DUSK_SIGNER_KEY_FILE" \
      --arg run_dir "$RUN_DIR" \
      --arg deployment_snapshot "$STATE_SNAPSHOT" \
      --arg run_id "$RUN_ID" \
      --argjson validators "$(printf '%s\n' "${VALIDATOR_CONFIGS[@]}" | jq -R . | jq -s .)" \
      --argjson dusk_validators "$(printf '%s\n' "${VALIDATOR_DUSK_CONFIGS[@]}" | jq -R . | jq -s .)" \
      '{runId: $run_id, runDir: $run_dir, deploymentSnapshot: $deployment_snapshot, relayer: $relayer, validator: $validator, duskValidator: $dusk_validator, validators: $validators, duskValidators: $dusk_validators, duskSignerKeyFile: $dusk_signer_key_file}')"
else
    output_json="$(jq -n \
      --arg relayer "$RELAYER_CONFIG" \
      --arg dusk_signer_key_file "$DUSK_SIGNER_KEY_FILE" \
      --arg run_dir "$RUN_DIR" \
      --arg deployment_snapshot "$STATE_SNAPSHOT" \
      --arg run_id "$RUN_ID" \
      '{runId: $run_id, runDir: $run_dir, deploymentSnapshot: $deployment_snapshot, relayer: $relayer, duskSignerKeyFile: $dusk_signer_key_file}')"
fi

printf '%s\n' "$output_json"
GENERATION_COMMITTED=1
