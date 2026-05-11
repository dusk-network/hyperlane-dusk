#!/usr/bin/env bash
# =============================================================================
# Start the Hyperlane Dusk <-> EVM Bridge Environment
# =============================================================================
#
# Starts:
#   1. Rusk (Dusk node) — from local fork at ~/projects/rusk
#   2. Anvil (EVM local node) — Foundry
#   3. Otterscan (EVM block explorer) — Docker
#   4. Dusk Explorer (Dusk block explorer) — SvelteKit dev server
#
# Usage: bash start-env.sh

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
NC='\033[0m'

info()   { echo -e "${BLUE}[INFO]${NC}  $*"; }
ok()     { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()   { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()   { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }
header() { echo -e "\n${BOLD}${GREEN}  $*${NC}\n"; }

record_pid() {
    echo "$1:$2" >> "$PID_FILE"
}

port_in_use() {
    lsof -i ":$1" &>/dev/null 2>&1
}

wait_for_url() {
    local url="$1" max_wait="${2:-30}" i=0
    while [ "$i" -lt "$max_wait" ]; do
        # Rusk RUES endpoints require a POST and a version header.
        if [[ "$url" == *"/on/"* ]]; then
            if curl -sf --max-time 2 -X POST \
                -H "Content-Type: application/octet-stream" \
                -H "rusk-version: 1.0.0-rc.0" \
                "$url" >/dev/null 2>&1; then
                return 0
            fi
        else
            if curl -sf --max-time 2 "$url" >/dev/null 2>&1; then
                return 0
            fi
        fi
        sleep 1
        i=$((i + 1))
    done
    return 1
}

# ── Pre-flight Checks ───────────────────────────────────────────────────────

header "Pre-flight Checks"

# Check if already running
if [ -f "$PID_FILE" ]; then
    warn "PID file exists at $PID_FILE — environment may already be running."
    info "Run 'bash demo/stop-env.sh' first, or delete $PID_FILE if stale."
    exit 1
fi

# Rusk binary — package is dusk-rusk but binary is named 'rusk'
RUSK_BIN="$RUSK_DIR/target/release/rusk"
if [ ! -f "$RUSK_BIN" ]; then
    fail "Rusk binary not found at $RUSK_BIN

  Build it first (one-time):
    cd $RUSK_DIR
    make prepare-dev
    cargo build --release -p dusk-rusk --features archive"
fi
ok "Rusk binary: $RUSK_BIN"

# Rusk genesis state (gzip file created by 'make prepare-dev')
RUSK_STATE="/tmp/example.state"
if [ ! -e "$RUSK_STATE" ]; then
    fail "Rusk genesis state not found at $RUSK_STATE

  Generate it first (one-time):
    cd $RUSK_DIR
    make prepare-dev"
fi
ok "Rusk genesis state: $RUSK_STATE"

# dusk-tx binary
if [ ! -f "$DUSK_TX" ]; then
    info "dusk-tx not found, building..."
    (cd "$DUSK_DIR" && cargo build -p dusk-tx --release) || fail "Failed to build dusk-tx"
fi
ok "dusk-tx binary: $DUSK_TX"

# Contract WASMs
if [ ! -f "$WASM_DIR/hyperlane_dusk_mailbox.wasm" ]; then
    info "Contract WASMs not found, building..."
    (cd "$DUSK_DIR" && make all) || fail "Failed to build WASMs"
fi
ok "Contract WASMs: $WASM_DIR"

# Data-driver WASM (for explorer integration)
DATA_DRIVER_WASM="$DUSK_DIR/target/data-driver/wasm32-unknown-unknown/release/hyperlane_dusk_data_driver.wasm"
if [ ! -f "$DATA_DRIVER_WASM" ]; then
    info "Data-driver WASM not found, building..."
    (cd "$DUSK_DIR" && make data-driver) || fail "Failed to build data-driver"
fi
# Copy to explorer assets if explorer exists
if [ -d "$EXPLORER_DIR/src/lib/assets" ]; then
    cp "$DATA_DRIVER_WASM" "$EXPLORER_DIR/src/lib/assets/"
    ok "Data-driver WASM copied to explorer"
else
    ok "Data-driver WASM: $DATA_DRIVER_WASM"
fi

# Docker
if [ "${SKIP_OTTERSCAN:-false}" = "true" ]; then
    warn "Skipping Otterscan (SKIP_OTTERSCAN=true)"
    SKIP_OTTERSCAN=true
elif ! command -v docker &>/dev/null; then
    warn "Docker not found — Otterscan (EVM explorer) will be skipped"
    SKIP_OTTERSCAN=true
else
    SKIP_OTTERSCAN=false
    ok "Docker available"
fi

# Node.js / npm
if [ "${SKIP_DUSK_EXPLORER:-false}" = "true" ]; then
    warn "Skipping Dusk Explorer (SKIP_DUSK_EXPLORER=true)"
    SKIP_DUSK_EXPLORER=true
elif ! command -v npm &>/dev/null; then
    warn "npm not found — Dusk Explorer will be skipped"
    SKIP_DUSK_EXPLORER=true
else
    SKIP_DUSK_EXPLORER=false
    ok "npm available"
fi

# Foundry
for tool in anvil forge cast; do
    command -v "$tool" &>/dev/null || fail "$tool not found. Install: curl -L https://foundry.paradigm.xyz | bash && foundryup"
done
ok "Foundry tools: anvil, forge, cast"

# jq
command -v jq &>/dev/null || fail "jq not found. Install: sudo apt-get install jq"
ok "jq available"

# Initialize PID file
> "$PID_FILE"

# ── Start Rusk ───────────────────────────────────────────────────────────────

header "Starting Rusk (Dusk Node) on port $RUSK_HTTP_PORT"

if port_in_use "$RUSK_HTTP_PORT"; then
    info "Port $RUSK_HTTP_PORT already in use — assuming Rusk is running"
    echo "rusk:external" >> "$PID_FILE"
else
    info "Starting Rusk from $RUSK_DIR..."
    RUSK_LOG="/tmp/rusk-dev.log"
    (
        cd "$RUSK_DIR"
        DUSK_CONSENSUS_KEYS_PASS="$CONSENSUS_PASSWORD" \
            "$RUSK_BIN" -s "$RUSK_STATE" \
            --http-listen-addr "0.0.0.0:${RUSK_HTTP_PORT}" \
            > "$RUSK_LOG" 2>&1
    ) &
    RUSK_PID=$!
    # Some rusk builds may fork; record the actual listening PID if available.
    sleep 1
    RUSK_LISTEN_PID="$(lsof -ti \":${RUSK_HTTP_PORT}\" -sTCP:LISTEN 2>/dev/null | head -n 1 || true)"
    if [ -n "$RUSK_LISTEN_PID" ]; then
        record_pid "rusk" "$RUSK_LISTEN_PID"
        info "Rusk starting (PID: $RUSK_LISTEN_PID, log: $RUSK_LOG)"
    else
        record_pid "rusk" "$RUSK_PID"
        info "Rusk starting (PID: $RUSK_PID, log: $RUSK_LOG)"
    fi

    # Wait for RUES endpoint
    info "Waiting for Rusk RUES endpoint..."
    RUES_TEST_URL="${DUSK_RUES_URL}on/contracts:0100000000000000000000000000000000000000000000000000000000000000/chain_id"
    if wait_for_url "$RUES_TEST_URL" 60; then
        ok "Rusk is ready"
    else
        warn "Rusk did not respond within 60s — check $RUSK_LOG"
        info "Continuing anyway; it may still be starting up..."
    fi
fi

# ── Start Anvil ──────────────────────────────────────────────────────────────

header "Starting Anvil (EVM Node) on port $ANVIL_PORT"

if port_in_use "$ANVIL_PORT"; then
    info "Port $ANVIL_PORT already in use — assuming Anvil is running"
    echo "anvil:external" >> "$PID_FILE"
else
    anvil --port "$ANVIL_PORT" --chain-id "$ANVIL_CHAIN_ID" --host 0.0.0.0 > /tmp/anvil.log 2>&1 &
    ANVIL_PID=$!
    record_pid "anvil" "$ANVIL_PID"
    sleep 2

    if ! kill -0 "$ANVIL_PID" 2>/dev/null; then
        fail "Anvil failed to start — check /tmp/anvil.log"
    fi
    ok "Anvil started (PID: $ANVIL_PID)"
fi

# Verify
cast block-number --rpc-url "$ANVIL_RPC" >/dev/null 2>&1 || fail "Cannot connect to Anvil at $ANVIL_RPC"
ok "Anvil connected"

# ── Start Otterscan (EVM Explorer) ───────────────────────────────────────────

header "Starting Otterscan (EVM Explorer) on port $EVM_EXPLORER_PORT"

if [ "$SKIP_OTTERSCAN" = true ]; then
    warn "Skipping Otterscan (Docker not available)"
else
    # Remove any existing container
    docker rm -f "$OTTERSCAN_CONTAINER" >/dev/null 2>&1 || true

    # Pull image first (may take a while on first run)
    if ! docker image inspect otterscan/otterscan:latest >/dev/null 2>&1; then
        info "Pulling Otterscan Docker image (first time only)..."
        docker pull otterscan/otterscan:latest || fail "Failed to pull Otterscan image"
    fi

    docker run -d \
        --name "$OTTERSCAN_CONTAINER" \
        -p "${EVM_EXPLORER_PORT}:80" \
        --add-host=host.docker.internal:host-gateway \
        -e ERIGON_URL="http://localhost:${ANVIL_PORT}" \
        otterscan/otterscan:latest \
        >/dev/null 2>&1 || fail "Failed to start Otterscan"

    echo "otterscan:container:$OTTERSCAN_CONTAINER" >> "$PID_FILE"

    # Wait for it to be ready
    info "Waiting for Otterscan..."
    if wait_for_url "http://localhost:${EVM_EXPLORER_PORT}" 15; then
        ok "Otterscan ready at http://localhost:${EVM_EXPLORER_PORT}"
    else
        warn "Otterscan may still be starting — check 'docker logs $OTTERSCAN_CONTAINER'"
    fi
fi

# ── Start Dusk Explorer ──────────────────────────────────────────────────────

header "Starting Dusk Explorer on port $DUSK_EXPLORER_PORT"

if [ "$SKIP_DUSK_EXPLORER" = true ]; then
    warn "Skipping Dusk Explorer (npm not available)"
else
    if [ ! -d "$EXPLORER_DIR" ]; then
        warn "Dusk Explorer not found at $EXPLORER_DIR — skipping"
    else
        # Back up existing .env and write our config
        if [ -f "$EXPLORER_DIR/.env" ]; then
            cp "$EXPLORER_DIR/.env" "$EXPLORER_DIR/.env.backup.$(date +%s)"
        fi
        cat > "$EXPLORER_DIR/.env" <<'ENVEOF'
VITE_RUSK_PATH="/rusk"
VITE_REFETCH_INTERVAL=5000
VITE_STATS_REFETCH_INTERVAL=3000
VITE_BLOCKS_LIST_ENTRIES=15
VITE_CHAIN_INFO_ENTRIES=15
VITE_TRANSACTIONS_LIST_ENTRIES=15
VITE_FEATURE_TOKENS=true
VITE_HYPERLANE_WARP_DRC20_ID=""
VITE_HYPERLANE_MAILBOX_ID=""
ENVEOF

        # Install deps if needed
        if [ ! -d "$EXPLORER_DIR/node_modules" ]; then
            info "Installing Dusk Explorer dependencies..."
            (cd "$EXPLORER_DIR" && npm install) || warn "npm install failed"
        fi

        # Kill any existing process on our port
        if port_in_use "$DUSK_EXPLORER_PORT"; then
            info "Port $DUSK_EXPLORER_PORT in use — killing existing process..."
            fuser -k "${DUSK_EXPLORER_PORT}/tcp" 2>/dev/null || true
            sleep 1
        fi

        # Start dev server with --strictPort (fail if port taken instead of silently switching)
        # Use npx vite dev directly to avoid npm subshell PID issues
        # --host binds to 0.0.0.0 (needed for WSL2 access from Windows browser)
        (cd "$EXPLORER_DIR" && npx vite dev --port "$DUSK_EXPLORER_PORT" --strictPort --host > /tmp/dusk-explorer.log 2>&1) &
        EXPLORER_PID=$!
        record_pid "dusk-explorer" "$EXPLORER_PID"

        # Wait for the dev server to become reachable
        info "Waiting for Dusk Explorer..."
        if wait_for_url "http://localhost:${DUSK_EXPLORER_PORT}" 15; then
            ok "Dusk Explorer ready at http://localhost:${DUSK_EXPLORER_PORT}"
        else
            warn "Dusk Explorer may not have started — check /tmp/dusk-explorer.log"
        fi
    fi
fi

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo -e "${BOLD}${GREEN}Environment Ready${NC}"
echo ""
printf "  %-20s %-8s %s\n" "Service" "Port" "URL"
printf "  %-20s %-8s %s\n" "-------" "----" "---"
printf "  %-20s %-8s %s\n" "Rusk (Dusk)" "$RUSK_HTTP_PORT" "$DUSK_RUES_URL"
printf "  %-20s %-8s %s\n" "Anvil (EVM)" "$ANVIL_PORT" "$ANVIL_RPC"

if [ "$SKIP_OTTERSCAN" = false ]; then
    printf "  %-20s %-8s %s\n" "EVM Explorer" "$EVM_EXPLORER_PORT" "$EVM_EXPLORER_URL"
fi
if [ "$SKIP_DUSK_EXPLORER" = false ] && [ -d "${EXPLORER_DIR:-}" ]; then
    printf "  %-20s %-8s %s\n" "Dusk Explorer" "$DUSK_EXPLORER_PORT" "$DUSK_EXPLORER_URL"
fi

echo ""
echo -e "${BOLD}Next steps:${NC}"
echo "  1. Deploy contracts:  bash demo/deploy.sh"
echo "  2. Check status:      bash demo/bridge.sh status"
echo "  3. Bridge to Dusk:    bash demo/bridge.sh to-dusk 3"
echo "  4. Bridge to EVM:     bash demo/bridge.sh to-evm 1"
echo "  5. Stop everything:   bash demo/stop-env.sh"
echo ""
echo -e "  PID file: ${CYAN}$PID_FILE${NC}"
echo ""
