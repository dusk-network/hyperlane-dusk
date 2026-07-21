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

# Gas settings for Dusk tx submission (passed through to dusk-tx).
DUSK_GAS_LIMIT="${DUSK_GAS_LIMIT:-30000000}"
DUSK_GAS_PRICE="${DUSK_GAS_PRICE:-2000}"

# ── Decrypt Dusk Deployer Key (consensus.keys) ──────────────────────────────

if [ ! -f "$CONSENSUS_KEYS" ]; then
    fail "Consensus keys not found at $CONSENSUS_KEYS"
fi

DUSK_SECRET_KEY_HEX="$(
python3 - <<PY
import base64, json
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

path = ${CONSENSUS_KEYS@Q}
password = ${CONSENSUS_PASSWORD@Q}

enc = json.load(open(path, "r", encoding="utf-8"))
salt = base64.b64decode(enc["salt"])
iv = base64.b64decode(enc["iv"])
key_pair = bytes(enc["key_pair"])

kdf = PBKDF2HMAC(algorithm=hashes.SHA256(), length=32, salt=salt, iterations=10000)
aes_key = kdf.derive(password.encode("utf-8"))
pt = AESGCM(aes_key).decrypt(iv, key_pair, None)
kp = json.loads(pt)
sk = base64.b64decode(kp["secret_key_bls"])
if len(sk) != 32:
    raise SystemExit(f"Expected 32-byte BLS secret key, got {len(sk)} bytes")
print(sk.hex())
PY
)"

# ── Output Paths ────────────────────────────────────────────────────────────

RELAYER_DB="$RUN_DIR/db-relayer"
VALIDATOR_DB="$RUN_DIR/db-validator-anvil"
CHECKPOINT_DIR="$RUN_DIR/checkpoints-anvil"

RELAYER_CONFIG="$RUN_DIR/relayer.json"
VALIDATOR_CONFIG="$RUN_DIR/validator-anvil.json"
DUSK_SIGNER_KEY_FILE="$RUN_DIR/dusk-signer.key"

printf '0x%s\n' "$DUSK_SECRET_KEY_HEX" > "$DUSK_SIGNER_KEY_FILE"

# ── Write Relayer Config ────────────────────────────────────────────────────

cat > "$RELAYER_CONFIG" <<JSON
{
  "metricsPort": 19092,
  "log": { "level": "debug", "format": "pretty" },
  "db": "${RELAYER_DB}",
  "relayChains": "anvil,dusk",
  "allowLocalCheckpointSyncers": true,
  "txIdIndexingEnabled": false,
  "igpIndexingEnabled": false,
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
      "signer": { "type": "hexKey", "key": "${ANVIL_PRIVATE_KEY}" }
    },
    "dusk": {
      "name": "dusk",
      "domainId": ${DUSK_DOMAIN},
      "chainId": ${DUSK_CHAIN_ID},
      "protocol": "dusk",
      "rpcUrls": [{ "http": "${DUSK_RUES_URL}" }],
      "gasLimit": ${DUSK_GAS_LIMIT},
      "gasPrice": ${DUSK_GAS_PRICE},
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

# ── Write Validator Config (only for MessageIdMultisig) ─────────────────────

if [ "$ISM" = "messageIdMultisig" ]; then
    mkdir -p "$CHECKPOINT_DIR"

    cat > "$VALIDATOR_CONFIG" <<JSON
{
  "metricsPort": 19093,
  "log": { "level": "debug", "format": "pretty" },
  "db": "${VALIDATOR_DB}",
  "originChainName": "anvil",
  "validator": { "type": "hexKey", "key": "${ANVIL_PRIVATE_KEY}" },
  "checkpointSyncer": { "type": "localStorage", "path": "${CHECKPOINT_DIR}" },
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
      "signer": { "type": "hexKey", "key": "${ANVIL_PRIVATE_KEY}" }
    }
  }
}
JSON
fi

# ── Print Paths as JSON ─────────────────────────────────────────────────────

if [ "$ISM" = "messageIdMultisig" ]; then
    output_json="$(jq -n \
      --arg relayer "$RELAYER_CONFIG" \
      --arg validator "$VALIDATOR_CONFIG" \
      --arg dusk_signer_key_file "$DUSK_SIGNER_KEY_FILE" \
      --arg run_dir "$RUN_DIR" \
      --arg deployment_snapshot "$STATE_SNAPSHOT" \
      --arg run_id "$RUN_ID" \
      '{runId: $run_id, runDir: $run_dir, deploymentSnapshot: $deployment_snapshot, relayer: $relayer, validator: $validator, duskSignerKeyFile: $dusk_signer_key_file}')"
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
