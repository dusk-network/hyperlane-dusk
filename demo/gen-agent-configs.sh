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
source "$SCRIPT_DIR/.env.bridge"

# ── Helpers ──────────────────────────────────────────────────────────────────

fail() { echo "[FAIL] $*" >&2; exit 1; }

# ── Args ────────────────────────────────────────────────────────────────────

ISM="testMock"
RUN_ID="$(date +%s)"

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

# ChainId for Dusk is an 8-bit value on Moonlight; pull from the Dusk deploy output if present.
DUSK_CHAIN_ID="$(jq -r '.chain_id // 0' /tmp/hyperlane-demo-dusk-deploy.json 2>/dev/null || echo 0)"
if [ -z "$DUSK_CHAIN_ID" ] || [ "$DUSK_CHAIN_ID" = "null" ]; then
    DUSK_CHAIN_ID="0"
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

RELAYER_DB="/tmp/hyperlane-db-relayer-${ISM}-${RUN_ID}"
VALIDATOR_DB="/tmp/hyperlane-db-validator-anvil-${ISM}-${RUN_ID}"
CHECKPOINT_DIR="/tmp/hyperlane-checkpoints-anvil-${ISM}-${RUN_ID}"

RELAYER_CONFIG="/tmp/hyperlane-relayer-${ISM}-${RUN_ID}.json"
VALIDATOR_CONFIG="/tmp/hyperlane-validator-anvil-${ISM}-${RUN_ID}.json"

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
      "signer": { "type": "duskKey", "key": "0x${DUSK_SECRET_KEY_HEX}" }
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
    jq -n \
      --arg relayer "$RELAYER_CONFIG" \
      --arg validator "$VALIDATOR_CONFIG" \
      --arg run_id "$RUN_ID" \
      '{run_id: $run_id, relayer: $relayer, validator: $validator}'
else
    jq -n \
      --arg relayer "$RELAYER_CONFIG" \
      --arg run_id "$RUN_ID" \
      '{run_id: $run_id, relayer: $relayer}'
fi

