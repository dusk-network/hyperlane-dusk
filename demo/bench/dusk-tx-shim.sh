#!/usr/bin/env bash
# =============================================================================
# dusk-tx shim for the local agents benchmark
# =============================================================================
#
# Set as an agent's DUSK_TX_BIN. Each time the agent runs the helper, the shim
# logs the call to BENCH_DUSK_TX_LOG and runs the real dusk-tx
# (BENCH_DUSK_TX_REAL). When the agent passes the RUES URL on stdin, the shim
# appends /dusk-tx/ to it, so the helper's requests reach the agent's proxy
# port under that prefix and proxy.py logs them as the helper's. The signing
# key that follows the URL on stdin is passed through unread.
#
# One JSON line per call: start and end time, agent (BENCH_AGENT), function,
# contract, exit code, the result dusk-tx printed (transaction hash, block,
# gas spent, or the error), and for `process` the arguments in hex (the
# message and its metadata, so the call can be matched to a message). The
# helper's stdout is held until it exits, which is when the agent reads it;
# stderr, where it reports the prepared transaction hash, is passed through
# as it comes.

set -u
real="${BENCH_DUSK_TX_REAL:?BENCH_DUSK_TX_REAL must name the dusk-tx binary}"
log="${BENCH_DUSK_TX_LOG:?BENCH_DUSK_TX_LOG must name the log file}"
agent="${BENCH_AGENT:-unknown}"

start="$(date +%s.%N)"
fn="" contract="" args="" url_stdin=0 prev=""
for arg in "$@"; do
    case "$prev" in
        --fn-name) fn="$arg" ;;
        --contract) contract="$arg" ;;
        --args) args="$arg" ;;
    esac
    [ "$arg" = "--rues-url-stdin" ] && url_stdin=1
    prev="$arg"
done

out="${log%.jsonl}.$$.out"
if [ "$url_stdin" = 1 ]; then
    IFS= read -r url || exit 1
    { printf '%s\n' "${url%/}/dusk-tx/"; cat; } | "$real" "$@" > "$out"
    rc="${PIPESTATUS[1]}"
else
    "$real" "$@" > "$out"
    rc=$?
fi
cat "$out"

[ "$fn" = "process" ] || args=""
result="$(jq -c '{tx: .tx_id, height: .block_height, gas: .gas_spent, final: .finalized,
    error: (.error // null | if . then .[0:300] else . end)}' "$out" 2>/dev/null)"
rm -f "$out"
printf '{"ts":%s,"end":%s,"agent":"%s","fn":"%s","contract":"%s","rc":%d,"pid":%d,"result":%s,"args":"%s"}\n' \
    "$start" "$(date +%s.%N)" "$agent" "$fn" "$contract" "$rc" "$$" "${result:-null}" "$args" >> "$log"
exit "$rc"
