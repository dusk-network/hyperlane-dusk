#!/usr/bin/env bash
# =============================================================================
# Stop the Hyperlane Dusk <-> EVM Bridge Environment
# =============================================================================
#
# Stops all services started by start-env.sh.
#
# Usage: bash stop-env.sh [--force]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/.env.bridge"
RUSK_STATE="${RUSK_STATE:-/tmp/example.state}"

# ── Helpers ──────────────────────────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()   { echo -e "${BLUE}[INFO]${NC}  $*"; }
ok()     { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()   { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()   { echo -e "${RED}[FAIL]${NC}  $*"; exit 1; }

FORCE=false
if [ "${1:-}" = "--force" ]; then
    FORCE=true
fi

ensure_rusk_stopped() {
    # Rusk may fork, so killing the recorded PID isn't always sufficient.
    # Only stop rusk processes using this demo's exact state archive.
    local pid arg matches_state
    while read -r pid; do
        [ -r "/proc/$pid/cmdline" ] || continue
        matches_state=false
        while IFS= read -r -d '' arg; do
            if [ "$arg" = "$RUSK_STATE" ]; then
                matches_state=true
                break
            fi
        done < "/proc/$pid/cmdline"
        if [ "$matches_state" = true ]; then
            kill "$pid" 2>/dev/null || true
        fi
    done < <(pgrep -x rusk 2>/dev/null || true)
    # Give the listener a moment to release the port.
    for _ in 1 2 3 4 5; do
        lsof -ti ":${RUSK_HTTP_PORT}" -sTCP:LISTEN >/dev/null 2>&1 || return 0
        sleep 1
    done
    # Force kill any matching stragglers.
    while read -r pid; do
        [ -r "/proc/$pid/cmdline" ] || continue
        matches_state=false
        while IFS= read -r -d '' arg; do
            if [ "$arg" = "$RUSK_STATE" ]; then
                matches_state=true
                break
            fi
        done < "/proc/$pid/cmdline"
        if [ "$matches_state" = true ]; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    done < <(pgrep -x rusk 2>/dev/null || true)
}

# ── Stop Services ────────────────────────────────────────────────────────────

if [ ! -f "$PID_FILE" ]; then
    info "No PID file found at $PID_FILE — nothing to stop."
    exit 0
fi

echo ""
info "Stopping bridge environment..."
echo ""

while IFS=: read -r name type rest; do
    case "$type" in
        container)
            # Docker container
            container_name="$rest"
            info "Stopping Docker container: $container_name"
            docker stop "$container_name" >/dev/null 2>&1 && docker rm "$container_name" >/dev/null 2>&1 \
                && ok "Stopped: $name ($container_name)" \
                || warn "Container $container_name was not running"
            ;;
        external)
            # Service was already running when start-env.sh ran
            if [ "$name" = "rusk" ]; then
                # Always kill Rusk — it was started for this demo environment
                info "Stopping $name (was running before start-env.sh)..."
                ensure_rusk_stopped
                ok "Stopped: $name"
            else
                info "Skipping $name (was already running externally)"
            fi
            ;;
        *)
            # PID-based process (format is name:pid)
            pid="$type"
            if kill -0 "$pid" 2>/dev/null; then
                info "Stopping $name (PID: $pid)..."
                # Kill the process group (handles npm/node child processes)
                kill -- -"$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true

                if [ "$FORCE" = false ]; then
                    # Wait up to 5 seconds for graceful shutdown
                    for _ in 1 2 3 4 5; do
                        kill -0 "$pid" 2>/dev/null || break
                        sleep 1
                    done
                fi

                # Force kill if still alive
                if kill -0 "$pid" 2>/dev/null; then
                    kill -9 -- -"$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true
                    ok "Killed: $name (PID: $pid)"
                else
                    ok "Stopped: $name (PID: $pid)"
                fi

                if [ "$name" = "rusk" ]; then
                    ensure_rusk_stopped
                fi
            else
                info "$name (PID: $pid) was not running"
                if [ "$name" = "rusk" ]; then
                    info "Ensuring no rusk process is still running..."
                    ensure_rusk_stopped
                    ok "Stopped: rusk"
                fi
            fi
            ;;
    esac
done < "$PID_FILE"

# Clean up PID file
rm -f "$PID_FILE"

# Kill any leftover process on the Dusk Explorer port
if command -v fuser &>/dev/null; then
    fuser -k "${DUSK_EXPLORER_PORT}/tcp" 2>/dev/null || true
fi

# Restore explorer .env backup if one exists
if [ -d "${EXPLORER_DIR:-}" ]; then
    LATEST_BACKUP=$(ls -t "$EXPLORER_DIR"/.env.backup.* 2>/dev/null | head -1)
    if [ -n "$LATEST_BACKUP" ]; then
        mv "$LATEST_BACKUP" "$EXPLORER_DIR/.env"
        ok "Restored Dusk Explorer .env from backup"
    fi
fi

echo ""
ok "Bridge environment stopped."
echo ""
