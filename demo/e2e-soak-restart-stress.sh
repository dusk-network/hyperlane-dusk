#!/usr/bin/env bash
# =============================================================================
# E2E: Repeated Relayer Restart/Backlog Soak
# =============================================================================
#
# Repeats the relayer restart/backlog stress scenario for multiple cycles or
# until a time budget is reached. Each cycle starts from a fresh local
# environment through e2e-relayer-restart-stress.sh.
#
# Environment:
# - SOAK_CYCLES: max number of cycles to run (default: 3).
# - SOAK_MINUTES: optional wall-clock budget. If set, no new cycle starts
#   after this budget has elapsed.
# - TRANSFERS: transfers per direction in each cycle (default inherited by
#   e2e-relayer-restart-stress.sh).
# - TRANSFER_AMOUNT_WEI: per-transfer amount in wei.
# - TIMEOUT_SECS: wait timeout for each checkpoint within a cycle.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

fail() { echo "[FAIL] $*" >&2; exit 1; }
info() { echo "[INFO] $*" >&2; }

SOAK_CYCLES="${SOAK_CYCLES:-3}"
SOAK_MINUTES="${SOAK_MINUTES:-}"

if ! [[ "$SOAK_CYCLES" =~ ^[1-9][0-9]*$ ]]; then
    fail "SOAK_CYCLES must be a positive integer"
fi

if [ -n "$SOAK_MINUTES" ] && ! [[ "$SOAK_MINUTES" =~ ^[1-9][0-9]*$ ]]; then
    fail "SOAK_MINUTES must be a positive integer when set"
fi

run_id="$(date +%s)"
summary_log="/tmp/hyperlane-soak-restart-stress-${run_id}.log"
start_ts="$(date +%s)"
deadline_ts=""

if [ -n "$SOAK_MINUTES" ]; then
    deadline_ts=$((start_ts + SOAK_MINUTES * 60))
fi

info "Starting restart/backlog soak"
info "  cycles:      $SOAK_CYCLES"
info "  minutes:     ${SOAK_MINUTES:-unbounded}"
info "  summary log: $summary_log"

{
    echo "run_id=$run_id"
    echo "start_ts=$start_ts"
    echo "soak_cycles=$SOAK_CYCLES"
    echo "soak_minutes=${SOAK_MINUTES:-}"
    echo "transfers=${TRANSFERS:-}"
    echo "transfer_amount_wei=${TRANSFER_AMOUNT_WEI:-}"
    echo "timeout_secs=${TIMEOUT_SECS:-}"
    echo "rusk_dir=${RUSK_DIR:-}"
} >"$summary_log"

completed=0

for cycle in $(seq 1 "$SOAK_CYCLES"); do
    now_ts="$(date +%s)"
    if [ -n "$deadline_ts" ] && [ "$now_ts" -ge "$deadline_ts" ]; then
        info "Time budget reached before cycle $cycle; stopping soak"
        break
    fi

    cycle_log="/tmp/hyperlane-soak-restart-stress-${run_id}-cycle-${cycle}.log"
    info "Starting soak cycle $cycle/$SOAK_CYCLES (log: $cycle_log)"
    {
        echo ""
        echo "cycle=$cycle"
        echo "cycle_log=$cycle_log"
        echo "cycle_start_ts=$now_ts"
    } >>"$summary_log"

    if bash "$SCRIPT_DIR/e2e-relayer-restart-stress.sh" >"$cycle_log" 2>&1; then
        completed=$((completed + 1))
        cycle_end_ts="$(date +%s)"
        {
            echo "cycle_end_ts=$cycle_end_ts"
            echo "cycle_status=passed"
        } >>"$summary_log"
        info "Cycle $cycle passed"
    else
        cycle_end_ts="$(date +%s)"
        {
            echo "cycle_end_ts=$cycle_end_ts"
            echo "cycle_status=failed"
        } >>"$summary_log"
        tail -n 200 "$cycle_log" >&2 || true
        fail "Soak cycle $cycle failed (log: $cycle_log)"
    fi
done

end_ts="$(date +%s)"
elapsed=$((end_ts - start_ts))

{
    echo ""
    echo "completed_cycles=$completed"
    echo "end_ts=$end_ts"
    echo "elapsed_secs=$elapsed"
} >>"$summary_log"

if [ "$completed" -eq 0 ]; then
    fail "No soak cycles completed"
fi

info "Soak result:"
info "  completed cycles: $completed"
info "  elapsed seconds:  $elapsed"
info "  summary log:      $summary_log"
info "CASE OK: relayer restart soak"
