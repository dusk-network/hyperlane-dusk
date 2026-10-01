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
    # Match real argv on Linux and macOS, including paths containing spaces.
    # A stale wrapper PID must never hide a surviving demo-owned node.
    local pid pids
    pids="$(python3 "$SCRIPT_DIR/rusk-processes.py" --state "$RUSK_STATE")" || return 1
    [ -n "$pids" ] || return 0
    while read -r pid; do
        kill "$pid" 2>/dev/null || true
    done <<< "$pids"
    for _ in 1 2 3 4 5; do
        pids="$(python3 "$SCRIPT_DIR/rusk-processes.py" --state "$RUSK_STATE")" || return 1
        [ -n "$pids" ] || return 0
        sleep 1
    done
    while read -r pid; do
        kill -9 "$pid" 2>/dev/null || true
    done <<< "$pids"
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
        group)
            # Only start-env's explicitly recorded process groups are owned.
            group="$rest"
            [[ "$group" =~ ^[1-9][0-9]*$ ]] && [ "$group" -gt 1 ] \
                || fail "Invalid process group in PID file"
            kill -- -"$group" 2>/dev/null || true
            if [ "$FORCE" = false ]; then
                for _ in 1 2 3 4 5; do
                    kill -0 -- -"$group" 2>/dev/null || break
                    sleep 1
                done
            fi
            kill -9 -- -"$group" 2>/dev/null || true
            ok "Stopped: $name (process group $group)"
            ;;
        external)
            # Service was already running when start-env.sh ran
            info "Skipping $name (was already running externally and is not demo-owned)"
            ;;
        *)
            # PID-based process (format is name:pid)
            pid="$type"
            if [ "$name" = "rusk" ]; then
                info "Stopping Rusk processes using this demo's state archive..."
                ensure_rusk_stopped
                ok "Stopped: rusk"
                continue
            fi
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

            else
                info "$name (PID: $pid) was not running"
            fi
            ;;
    esac
done < "$PID_FILE"

# Clean up PID file
rm -f "$PID_FILE"

# Service ownership comes from the PID file. Never kill a process merely
# because it is listening on the configured explorer port.

# Restore explorer .env backup if one exists
if [ -d "${EXPLORER_DIR:-}" ]; then
    LATEST_BACKUP=$(ls -t "$EXPLORER_DIR"/.env.backup.* 2>/dev/null | head -1 || true)
    if [ -n "$LATEST_BACKUP" ]; then
        mv "$LATEST_BACKUP" "$EXPLORER_DIR/.env"
        ok "Restored Dusk Explorer .env from backup"
    fi
fi

echo ""
ok "Bridge environment stopped."
echo ""
