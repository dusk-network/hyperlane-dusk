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
# Validators (docs/validators.md):
#   bash deploy/testnet.sh announce <location> [address]  # announce a
#                                                #   validator on Dusk from its
#                                                #   signed announcement
#   bash deploy/testnet.sh set-validators        # the ISMs against the settings'
#                                                #   validator sets; dry run, add
#                                                #   --apply to move them
#
# State goes to deploy/out/<DEPLOY_NAME>/deployment.json. Re-running `deploy`
# skips every step the state file records as done and re-checks it on chain.
# Every command but fees reads the state once, holding the lock on that
# directory's .lock until it exits: one command at a time on a deployment.
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
DUSK_MULTICALL="${DUSK_MULTICALL-93de287b7d349d678a0038fe7772e8960e1a84a1b5da2d0b1f62f532616ce1d4}"
WASM_DIR="${WASM_DIR:-$DUSK_DIR/target/contract/wasm32-unknown-unknown/release}"
OUT_DIR="$DUSK_DIR/deploy/out/$DEPLOY_NAME"
STATE="$OUT_DIR/deployment.json"
ZERO_ADDRESS=0x0000000000000000000000000000000000000000
ZERO_ID=0000000000000000000000000000000000000000000000000000000000000000
TRANSFER_CONTRACT=0100000000000000000000000000000000000000000000000000000000000000
# The state_version of each core contract this checkout builds. Older ones
# keep storage anyone can fill, and this checkout's agents don't read them
# (docs/storage.md): before Mailbox 6 and ValidatorAnnounce 3 a contract can
# deliver or announce many times in one transaction. An IGP before 4 can't be
# topped up (pay_for_gas), so a message that paid it too little is never
# relayed (docs/operations.md).
CORE_STATE_VERSIONS="mailbox=6 merkle_tree_hook=2 igp=4 validator_announce=3"
# The state_version of each warp route this checkout builds. Older routes may
# pay a sponsored escrow claim to the key as given, which a forged twin of the
# recipient's key can exploit (docs/security.md, CRITICAL-1), don't register a
# recipient that claims its own escrow, and register any key that asks
# (MEDIUM-10). WarpDrc20 8 limits allowances and keeps its last GiB of memory
# for the bridge (docs/storage.md). Older collateral routes can be re-entered
# from their token (MEDIUM-13).
ROUTE_STATE_VERSIONS="warp_native=5 warp_drc20=8 warp_drc20_collateral=7"
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
# Initialized(uint8). IMPLEMENTATION_SLOT and ADMIN_SLOT are EIP-1967's.
UPGRADED_TOPIC=0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b
INITIALIZED_TOPIC=0x7f26b83ff96e1f2b6a682f133852f6798a09c465da95921460cefb3847402498
IMPLEMENTATION_SLOT=0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc
ADMIN_SLOT=0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103
# QuoteSignerAdded(address), which the EVM IGP emits when its owner adds a
# quote signer.
QUOTE_SIGNER_ADDED_TOPIC=0x1999e59a6c4f2d603b5236b4b6a9ab4caecb598d4a59fe663b6eae45ef79a3d6
# The most blocks one log query to EVM_RPC spans: the relayer's config
# indexes in chunks of 1,999.
EVM_LOG_BLOCK_RANGE="${EVM_LOG_BLOCK_RANGE:-1999}"

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

# The deployment record, as this command holds it. lock_deployment reads
# $STATE once, under the deployment's lock: every check, what deploy records
# and the agent configs come from this copy, and state_set writes it back.
STATE_JSON=""
state_get() { [ -n "$STATE_JSON" ] && jq -r "$1 // empty" <<<"$STATE_JSON" || true; }
# The record as JSON, {} when there is none.
state_json() { if [ -n "$STATE_JSON" ]; then printf '%s\n' "$STATE_JSON"; else echo '{}'; fi; }
# A record that can't be written stops the command there, so nothing goes
# on from a step the record lacks. A failed write in an && list wouldn't
# stop it under set -e, so each step is checked. The record is on disk
# when state_set returns, so what a step sends after it, as an EVM send
# after its record (evm_intend), is in the record after a crash of the
# host too.
state_set() {
    local new tmp
    new="$(state_json | jq "$@")" || fail "Can't update $STATE"
    mkdir -p "$OUT_DIR" || fail "Can't create $OUT_DIR"
    tmp="$(mktemp "$OUT_DIR/.state.XXXXXX")" || fail "Can't write to $OUT_DIR"
    if ! printf '%s\n' "$new" > "$tmp" || ! durable_replace "$tmp" "$STATE"; then
        rm -f "$tmp"
        fail "Can't write $STATE, so this run stops before the next step. Free space or fix the directory's permissions, then run it again: it resumes from what $STATE records."
    fi
    STATE_JSON="$new"
}
# sync_directories <directory>...: have each directory's entries on disk.
sync_directories() {
    python3 - "$@" <<'PY'
import os
import sys

for name in sys.argv[1:]:
    fd = os.open(name, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
PY
}
# durable_replace <temporary file> <file>: put the temporary file in the
# file's place for good: its contents reach the disk, then it is renamed
# over the file, then the directory that holds both reaches the disk.
durable_replace() {
    python3 - "$1" "$2" <<'PY'
import os
import sys

temporary, path = sys.argv[1:]


def sync(name):
    fd = os.open(name, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


sync(temporary)
os.replace(temporary, path)
sync(os.path.dirname(os.path.abspath(path)))
PY
}

# One command at a time on a deployment. Every command but fees takes the
# lock on deploy/out/<DEPLOY_NAME>/.lock before it reads the record, and
# holds it until it exits, so no other command records or replaces anything
# between what this one checks and what it records or configures.
lock_deployment() {
    local made=""
    command -v flock >/dev/null || fail "flock is required (util-linux)"
    [ -d "$OUT_DIR" ] || made=1
    mkdir -p "$OUT_DIR"
    # A deployment directory made here, and deploy/out if it is new too, is
    # in its parent for good, after a crash of the host as well, once the
    # parent is synced: so it is, before anything is sent or recorded.
    [ -z "$made" ] || sync_directories "$(dirname "$OUT_DIR")" "$(dirname "$(dirname "$OUT_DIR")")" \
        || fail "Can't sync the directories $OUT_DIR was made in"
    exec 9>>"$OUT_DIR/.lock"
    flock -n 9 \
        || fail "Another deploy/testnet.sh command is running on $OUT_DIR. Wait for it to finish, then run this one again."
    [ -f "$STATE" ] || return 0
    STATE_JSON="$(<"$STATE")"
    jq -e 'type == "object"' <<<"$STATE_JSON" >/dev/null 2>&1 || fail "$STATE isn't a JSON object"
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
# DUSK_READ_HINT, when set, says what to do after a failed read.
DUSK_READ_HINT=""
dusk_value() {
    local label="$1" out value
    shift
    out="$(dusk_query "$@" 2>/dev/null)" \
        || fail "Can't read $label: $(jq -r '.error // "the query failed"' <<<"$out" 2>/dev/null)${DUSK_READ_HINT:+. $DUSK_READ_HINT}"
    value="$(jq -er '.value | select(. != null) | tostring' <<<"$out" 2>/dev/null)" \
        || fail "Can't read $label: the query returned no value${DUSK_READ_HINT:+. $DUSK_READ_HINT}"
    printf '%s' "$value"
}
dusk_tx() { DUSK_CONSENSUS_PASSWORD="$DUSK_CONSENSUS_PASSWORD" "$DUSK_TX" "$@"; }
# EVM_RPC can be a load balancer over nodes a block or more apart, such as the
# public Sepolia endpoints, so a read right after a write can reach a node
# that hasn't seen it: a check of what was just written read the
# implementation slot of a proxy just created as zero. So once this command
# has written, every EVM read is made at a block, the newest of its last
# write's and the head the endpoint reports, never at "latest": a node that
# has that block answers with the write in it, and one that doesn't have it
# yet refuses the read ("header not found"), which evm_read tries again. A
# read before any write, or the first of a command, is at "latest".
EVM_WRITE_BLOCK=""
# Note the block of a transaction this command sent.
evm_wrote() {
    [[ "${1:-}" =~ ^[0-9]+$ ]] || return 0
    if [ -z "$EVM_WRITE_BLOCK" ] || [ "$1" -gt "$EVM_WRITE_BLOCK" ]; then
        EVM_WRITE_BLOCK="$1"
    fi
}
# The block EVM reads are made at: "latest" before this command's first
# write, then the newest of that write's block and the head, so that a read
# right before a write still sees as late a state as the endpoint can show.
evm_read_block() {
    local head
    if [ -z "$EVM_WRITE_BLOCK" ]; then
        echo latest
        return 0
    fi
    head="$(cast block-number --rpc-url "$EVM_RPC" 2>/dev/null)" || head=""
    if [[ "$head" =~ ^[0-9]+$ ]] && [ "$head" -gt "$EVM_WRITE_BLOCK" ]; then
        echo "$head"
    else
        echo "$EVM_WRITE_BLOCK"
    fi
}
# Run an EVM read, a cast command that takes --block, at evm_read_block's
# block. Once this command has written, a read that fails for anything but
# a revert is tried again, at a block looked up again, for up to
# EVM_SETTLE_SECONDS (120): it reached a node that doesn't have the block
# yet. A revert is the contract's answer, at any block. Prints the read's
# output, or its error.
evm_read() {
    local block out errors deadline
    deadline="$(evm_deadline "${EVM_SETTLE_SECONDS:-120}")"
    block="$(evm_read_block)"
    if [ "$block" = latest ]; then
        "$@" --rpc-url "$EVM_RPC"
        return
    fi
    errors="$(mktemp)" || return 1
    while [ "$SECONDS" -lt "$deadline" ]; do
        if out="$("$@" --block "$block" --rpc-url "$EVM_RPC" 2>"$errors")"; then
            rm -f "$errors"
            printf '%s\n' "$out"
            return 0
        fi
        grep -qi 'revert' "$errors" && break
        evm_sleep_until "$deadline" 1
        block="$(evm_read_block)"
    done
    cat "$errors" >&2
    rm -f "$errors"
    return 1
}
evm_call() { evm_read cast call "$@"; }
# After each write, evm_settle also waits until the endpoint answers at or
# past the write's block on three reads in a row, for up to
# EVM_SETTLE_SECONDS (120), so that the next transaction is less likely to
# get its nonce from a node that hasn't seen this one (evm_next_nonce reads
# it three times, and evm_submit fails rather than send anything again).
evm_settle() {
    local block="$1" seen=0 now deadline=$((SECONDS + ${EVM_SETTLE_SECONDS:-120}))
    [[ "$block" =~ ^[0-9]+$ ]] || fail "No block number to wait for on $EVM_CHAIN_NAME"
    while :; do
        now="$(cast block-number --rpc-url "$EVM_RPC" 2>/dev/null)" || now=0
        if [[ "$now" =~ ^[0-9]+$ ]] && [ "$now" -ge "$block" ]; then
            seen=$((seen + 1))
            [ "$seen" -lt 3 ] || return 0
            continue
        fi
        seen=0
        [ "$SECONDS" -lt "$deadline" ] \
            || fail "EVM_RPC still answers from before block $block, which holds a transaction just sent, after ${EVM_SETTLE_SECONDS:-120}s"
        sleep 2
    done
}
# A transaction's receipt, as `cast receipt` prints it (with a field or
# --json), waiting as evm_settle does while EVM_RPC doesn't have it: the
# node that answers may not have seen the transaction yet. Prints the last
# error if it never does.
evm_receipt() {
    local tx="$1" out errors deadline
    deadline="$(evm_deadline "${EVM_SETTLE_SECONDS:-120}")"
    shift
    errors="$(mktemp)" || return 1
    while [ "$SECONDS" -lt "$deadline" ]; do
        if out="$(cast receipt "$tx" "$@" --rpc-timeout "$(evm_time_left "$deadline")" \
            --rpc-url "$EVM_RPC" 2>"$errors")" && [ -n "$out" ]; then
            rm -f "$errors"
            printf '%s\n' "$out"
            return 0
        fi
        evm_sleep_until "$deadline" 2
    done
    cat "$errors" >&2
    rm -f "$errors"
    return 1
}
# The block a transaction is in, once EVM_RPC has its receipt.
evm_tx_block() {
    local block
    block="$(evm_receipt "$1" blockNumber 2>/dev/null)" && block="$(evm_block_number "$block")" \
        && [[ "$block" =~ ^[0-9]+$ ]] && printf '%s' "$block"
}
# The deployer's next nonce on the EVM chain: the highest of three reads of
# its pending nonce, as each node behind a load-balanced EVM_RPC can be
# behind. Every transaction deploy sends carries the nonce it was given, so
# that what took that nonce can be told afterwards.
evm_next_nonce() {
    local deployer read best="" i
    deployer="$(evm_deployer)" || return 1
    for i in 1 2 3; do
        read="$(cast nonce "$deployer" --block pending --rpc-url "$EVM_RPC" 2>/dev/null)" || continue
        [[ "$read" =~ ^[0-9]+$ ]] || continue
        if [ -z "$best" ] || [ "$read" -gt "$best" ]; then best="$read"; fi
    done
    [ -n "$best" ] || return 1
    printf '%s' "$best"
}
# evm_submit <nonce> <recipient, or empty for a creation> <input>
# <command...>: run a command that sends one EVM transaction (cast send,
# forge create) with the nonce it carries (--nonce). Nothing is ever sent
# again: a node's "nonce too low" doesn't show the transaction was refused,
# as a gateway that sent it on and then answered a replay of the request
# reports it so. The deployer's transaction with that nonce tells: a
# creation that is that very one, executed at the address the nonce gives,
# is taken (evm_adopt_creation); another transaction means this one never
# executes (exit status 4); this call, or none yet, fails, for the next
# run to settle (evm_settle_pending). A gas estimate that reverted sent
# nothing (exit status 3). Prints the command's output, or an adopted
# creation's receipt.
evm_submit() {
    local nonce="$1" to="$2" input="$3" from out errors receipt tx final
    shift 3
    from="$(cast block-number --rpc-url "$EVM_RPC" 2>/dev/null)" || from=""
    errors="$(mktemp)" || return 1
    if out="$("$@" 2>"$errors")"; then
        cat "$errors" >&2
        rm -f "$errors"
        printf '%s\n' "$out"
        return 0
    fi
    if grep -qi 'nonce too low' "$errors"; then
        cat "$errors" >&2
        rm -f "$errors"
        # What took the nonce: this transaction, or another.
        if ! [[ "$from" =~ ^[0-9]+$ ]] || ! tx="$(evm_sent_with_nonce "$(evm_deployer)" "$nonce" "$from")"; then
            echo "[FAIL] $EVM_CHAIN_NAME answered the transaction with nonce $nonce with \"nonce too low\", and no transaction with that nonce is in a block: it may still go out, so nothing was sent again. Run deploy again: it settles it first." >&2
            return 1
        fi
        if ! jq -e --arg to "$(lower "$to")" --arg i "$(lower "$input")" \
            '((.to // "") | ascii_downcase) == $to and ((.input // .data // "") | ascii_downcase) == $i' \
            <<<"$tx" >/dev/null; then
            # Until the other transaction's block is final, another block can
            # take its place, without it, and this one, if a node took it,
            # can still execute: it is forgotten only then.
            final=0
            evm_final_canonical "$tx" || final=$?
            if [ "$final" = 0 ]; then
                echo "[FAIL] $EVM_CHAIN_NAME answered the transaction with nonce $nonce with \"nonce too low\": another transaction of the deployer's took that nonce, in a block that is final, so this one never executes. Nothing was sent again. Run deploy again: it reads what is on chain afresh and sends what isn't done." >&2
                return 4
            fi
            echo "[FAIL] $EVM_CHAIN_NAME answered the transaction with nonce $nonce with \"nonce too low\": another transaction of the deployer's took that nonce, in a block that isn't final, or that another block has replaced. Until that is final, this one can still execute, so nothing was sent again. Run deploy again: it settles it first." >&2
            return 1
        fi
        if [ -z "$to" ] && receipt="$(evm_adopt_creation "$nonce" "$input" "$tx")"; then
            info "$EVM_CHAIN_NAME answered the creation with \"nonce too low\", but the deployer's transaction with its nonce $nonce is that very creation, executed: taking it"
            printf '%s\n' "$receipt"
            return 0
        fi
        echo "[FAIL] $EVM_CHAIN_NAME answered the transaction with nonce $nonce with \"nonce too low\", though it went out with that nonce: nothing was sent again. Run deploy again: it settles it first." >&2
        return 1
    fi
    # A revert in the gas estimate: nothing went out.
    if grep -qi 'execution reverted' "$errors"; then
        cat "$errors" >&2
        rm -f "$errors"
        return 3
    fi
    cat "$errors" >&2
    rm -f "$errors"
    printf '%s\n' "$out"
    return 1
}
# The EVM transaction each step of this script is sending, recorded in
# $STATE before it is sent, under .evm_sending, by the step that sends it:
# "create <the key $STATE records the contract at>" for a creation, "call
# <function> on <recipient>" for a call. The entry holds the step, the
# nonce, the recipient (none for a creation), the exact input and the
# block to look for it from, until the step records what it did
# (evm_done). A run that stops between the two leaves it for the step's
# next run, which settles it before the step sends anything
# (evm_settle_pending). Only the step settles its own entry: a creation's
# address is recorded by its step alone, so a step of another command, or
# another step of deploy, leaves the entry as it is. These change $STATE,
# so they run in this shell, never in a command substitution.
evm_creation_step() { printf 'create %s' "$1"; }
# A send recorded as .evm_pending, the form only an earlier version of this
# script wrote, has no step: which step it was, and how it ended, are for
# you to reconcile (docs/testnet.md, Reconciling .evm_pending). Every
# command that sends on the EVM side stops on it before sending anything.
check_no_unattributed_evm_send() {
    local pending
    pending="$(state_json | jq -c '.evm_pending // empty')"
    [ -n "$pending" ] || return 0
    fail "$STATE records an EVM send as .evm_pending, the form only an earlier version of this script wrote: nonce $(jq -r '.nonce' <<<"$pending") ($(jq -r '.label // "no label"' <<<"$pending")), without the step that sent it. Nothing was sent. Reconcile it by hand (docs/testnet.md, Reconciling .evm_pending), then remove .evm_pending from $STATE and run this again."
}
evm_call_step() { printf 'call %s on %s' "$2" "$(lower "$1")"; }
evm_intend() {
    local step="$1" nonce="$2" to="$3" input="$4" label="$5" from
    from="$(cast block-number --rpc-url "$EVM_RPC" 2>/dev/null)" && [[ "$from" =~ ^[0-9]+$ ]] \
        || fail "Can't read $EVM_CHAIN_NAME's block number"
    state_set --arg s "$step" --argjson n "$nonce" --arg to "$(lower "$to")" --arg i "$(lower "$input")" \
        --arg l "$label" --argjson b "$from" \
        '.evm_sending[$s] = {step: $s, nonce: $n, to: $to, input: $i, label: $l, from_block: $b}'
}
evm_done() {
    [ -n "$(state_json | jq -r --arg s "$1" '.evm_sending[$s] // empty')" ] || return 0
    state_set --arg s "$1" 'del(.evm_sending[$s]) | if .evm_sending == {} then del(.evm_sending) else . end'
}
# evm_record <step> [jq arguments...] <filter>: record in $STATE what <step>
# made, as state_set does, and forget its send, in the same write, so that
# no run finds the one without the other.
evm_record() {
    local step="$1"
    shift
    state_set --arg evm_step "$step" "${@:1:$#-1}" \
        "${!#} | del(.evm_sending[\$evm_step]) | if .evm_sending == {} then del(.evm_sending) else . end"
}
# evm_settle_call <recipient> <function>: settle the call an earlier run
# recorded as sending, before the call's step reads the chain to decide
# whether to send it. Once its transaction is in a block and executed, the
# record is forgotten, and the step's reads, made at that block or after
# it (evm_wrote), show what it did. Taking it as this run's send instead
# would be wrong for a call made again and again, as claim() is.
evm_settle_call() {
    local receipt
    evm_settle_pending receipt "$(evm_call_step "$1" "$2")" "$1" "*"
}
# evm_settle_pending <variable> <step> <recipient, or empty for a creation>
# <input>: settle the transaction <step> recorded as sending in an earlier
# run, if any, before the step chooses a nonce: the deployer's transaction
# with its nonce. If it is that transaction, executed, and what the step
# sends now, <variable> is set to its receipt, for the step to take as its
# own send; a creation also must have made the contract at the address the
# nonce gives. With <input> "*" (evm_settle_call), it is forgotten instead.
# If it failed, or the step no longer sends it, or another transaction
# took its nonce, the record is forgotten once that block is final, for
# the step to send afresh. One that isn't in a block within
# EVM_SETTLE_SECONDS, or in a final one within EVM_FINALITY_SECONDS, stops
# the script: nothing is sent.
evm_settle_pending() {
    local _sp_var="$1" _sp_step="$2" _sp_to="$3" _sp_input="$4" _sp_pending _sp_nonce _sp_label _sp_tx _sp_receipt _sp_address
    local _sp_what _sp_final
    printf -v "$_sp_var" '%s' ""
    check_no_unattributed_evm_send
    _sp_pending="$(state_json | jq -c --arg s "$_sp_step" '.evm_sending[$s] // empty')"
    [ -n "$_sp_pending" ] || return 0
    _sp_nonce="$(jq -r .nonce <<<"$_sp_pending")" _sp_label="$(jq -r .label <<<"$_sp_pending")"
    info "Settling the deployer's transaction with nonce $_sp_nonce ($_sp_label), which an earlier run sent or was sending..."
    # One deadline for the whole attempt, rechecks after a replaced block
    # included: each wait below keeps to it (evm_deadline).
    local evm_attempt_deadline=$((SECONDS + ${EVM_SETTLE_SECONDS:-120} + ${EVM_FINALITY_SECONDS:-1800}))
    while :; do
        if ! _sp_tx="$(evm_sent_with_nonce "$(evm_deployer)" "$_sp_nonce" "$(jq -r .from_block <<<"$_sp_pending")")"; then
            [ "$SECONDS" -lt "$evm_attempt_deadline" ] \
                || fail "The deployer's transaction with nonce $_sp_nonce ($_sp_label): settling it again didn't finish by the attempt's deadline (EVM_SETTLE_SECONDS plus EVM_FINALITY_SECONDS), so nothing was sent. Run deploy again."
            fail "The deployer's transaction with nonce $_sp_nonce ($_sp_label), which an earlier run sent or was sending, isn't in the blocks read (above): it may still be pending, or in a later block, so nothing was sent. Run deploy again: it reads from block $(jq -r .from_block <<<"$_sp_pending") again, and a larger EVM_SETTLE_SECONDS reads further. If it never went out, and the deployer's nonce stays below $_sp_nonce, remove the entry \"$_sp_step\" from .evm_sending in $STATE first."
        fi
        if jq -e --argjson p "$_sp_pending" \
            '((.to // "") | ascii_downcase) == $p.to and ((.input // .data // "") | ascii_downcase) == $p.input' \
            <<<"$_sp_tx" >/dev/null; then
            _sp_receipt="$(evm_receipt "$(jq -r .hash <<<"$_sp_tx")" --json)" \
                || fail "Can't read the receipt of $(jq -r .hash <<<"$_sp_tx") on $EVM_CHAIN_NAME"
            evm_wrote "$(evm_block_number "$(jq -r .blockNumber <<<"$_sp_receipt")")"
            if [ "$_sp_input" = "*" ] && jq -e '.status | tostring | test("^(0x)?1$")' <<<"$_sp_receipt" >/dev/null; then
                info "It executed: the step reads the chain as of its block, or later, to see what is left to do"
                evm_done "$_sp_step"
                return 0
            fi
            if jq -e '.status | tostring | test("^(0x)?1$")' <<<"$_sp_receipt" >/dev/null \
                && [ "$(jq -r .to <<<"$_sp_pending")" = "$(lower "$_sp_to")" ] \
                && [ "$(jq -r .input <<<"$_sp_pending")" = "$(lower "$_sp_input")" ]; then
                if [ -z "$_sp_to" ]; then
                    _sp_address="$(cast compute-address "$(evm_deployer)" --nonce "$_sp_nonce" 2>/dev/null | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)"
                    [ "$(lower "$(jq -r '.contractAddress // empty' <<<"$_sp_receipt")")" = "$(lower "$_sp_address")" ] \
                        && evm_has_code "$_sp_address" \
                        || fail "The deployer's creation with nonce $_sp_nonce ($_sp_label) didn't make the contract at $_sp_address, the address its nonce gives: nothing was sent. Find out why before running deploy again."
                fi
                info "It is this step's own transaction, executed: taking it"
                printf -v "$_sp_var" '%s' "$_sp_receipt"
                return 0
            fi
            if jq -e '.status | tostring | test("^(0x)?1$")' <<<"$_sp_receipt" >/dev/null; then
                _sp_what="It executed, but isn't what this step sends now"
            else
                _sp_what="It failed"
            fi
        else
            _sp_what="Another transaction of the deployer's, $(jq -r .hash <<<"$_sp_tx"), took its nonce"
            warn "$_sp_what: the one recorded as sending ($_sp_label) never executes once that is final"
        fi
        # Until its block is final, another block can take its place, and
        # what took the nonce, or how it ended, can change: it is forgotten
        # only once that block is final, and still the chain's.
        info "$_sp_what: waiting until its block is final..."
        _sp_final=0
        evm_final_canonical "$_sp_tx" || _sp_final=$?
        case "$_sp_final" in
            0)
                info "Its block is final: forgetting it, for the step to go on afresh"
                evm_done "$_sp_step"
                return 0
                ;;
            2)
                [ "$SECONDS" -lt "$evm_attempt_deadline" ] \
                    || fail "The deployer's transaction with nonce $_sp_nonce ($_sp_label): ${_sp_what,}, in a block another took the place of, and settling it again would go past the attempt's deadline (EVM_SETTLE_SECONDS plus EVM_FINALITY_SECONDS): nothing was sent. Run deploy again."
                info "Another block took the place of the one it was in: settling it again..."
                ;;
            *) fail "The deployer's transaction with nonce $_sp_nonce ($_sp_label): ${_sp_what,}, in a block that wasn't final, and the chain's, by the attempt's deadline (EVM_FINALITY_SECONDS for the block, within EVM_SETTLE_SECONDS plus EVM_FINALITY_SECONDS for the attempt). Until it is, that can change, so nothing was sent. Run deploy again once it is." ;;
        esac
    done
}
# evm_final_canonical <tx>: wait, for up to EVM_FINALITY_SECONDS (1800), until
# the block of <tx>, a transaction as `cast block --full --json` shows it,
# is final, then check that it is still the block at its height. Returns 0
# if it is, 2 if another block took its place, and 1 if it isn't final in
# time, or can't be read.
evm_final_canonical() {
    local number hash final canonical deadline
    deadline="$(evm_deadline "${EVM_FINALITY_SECONDS:-1800}")"
    number="$(evm_block_number "$(jq -r '.blockNumber // empty' <<<"$1")")" || return 1
    hash="$(lower "$(jq -r '.blockHash // empty' <<<"$1")")"
    [[ "$number" =~ ^[0-9]+$ ]] && [[ "$hash" =~ ^0x[0-9a-f]{64}$ ]] || return 1
    while [ "$SECONDS" -lt "$deadline" ]; do
        final="$(cast block-number finalized --rpc-timeout "$(evm_time_left "$deadline")" \
            --rpc-url "$EVM_RPC" 2>/dev/null)" || final=""
        if [[ "$final" =~ ^[0-9]+$ ]] && [ "$final" -ge "$number" ] && [ "$SECONDS" -lt "$deadline" ]; then
            canonical="$(cast block "$number" --json --rpc-timeout "$(evm_time_left "$deadline")" \
                --rpc-url "$EVM_RPC" 2>/dev/null | jq -r '.hash // empty' 2>/dev/null)" || canonical=""
            if [[ "$canonical" =~ ^0x[0-9a-fA-F]{64}$ ]]; then
                [ "$(lower "$canonical")" = "$hash" ] && return 0
                return 2
            fi
        fi
        evm_sleep_until "$deadline" 2
    done
    return 1
}
# The transaction <sender> sent with <nonce>, as `cast block --full --json`
# shows it, in a block from <from block> to the head, waiting up to
# EVM_SETTLE_SECONDS for EVM_RPC to show it. The deadline holds however
# many blocks are left to read, and each read is given only what is left
# of it.
evm_sent_with_nonce() {
    local sender="$1" nonce="$2" from="$3" block="$3" head="" txs found deadline
    deadline="$(evm_deadline "${EVM_SETTLE_SECONDS:-120}")"
    sender="$(lower "$sender")"
    while [ "$SECONDS" -lt "$deadline" ]; do
        head="$(cast block-number --rpc-timeout "$(evm_time_left "$deadline")" --rpc-url "$EVM_RPC" 2>/dev/null)" \
            || head=""
        while [[ "$head" =~ ^[0-9]+$ ]] && [ "$block" -le "$head" ] && [ "$SECONDS" -lt "$deadline" ]; do
            txs="$(cast block "$block" --full --json --rpc-timeout "$(evm_time_left "$deadline")" \
                --rpc-url "$EVM_RPC" 2>/dev/null)" || break
            found="$(jq -c --arg s "$sender" --arg n "$nonce" --arg h "$(printf '0x%x' "$nonce")" \
                '.transactions[]? | select(((.from // "") | ascii_downcase) == $s
                    and ((.nonce | tostring | ascii_downcase) as $v | $v == $n or $v == $h))' <<<"$txs" 2>/dev/null | head -1)"
            if [ -n "$found" ]; then
                printf '%s\n' "$found"
                return 0
            fi
            block=$((block + 1))
        done
        evm_sleep_until "$deadline" 2
    done
    if [ "$block" -gt "$from" ]; then
        info "No transaction of $sender's with nonce $nonce in blocks $from to $((block - 1)) of $EVM_CHAIN_NAME (the head was ${head:-unknown}), read before the deadline"
    else
        info "No block from $from on of $EVM_CHAIN_NAME was read before the deadline, for $sender's transaction with nonce $nonce"
    fi
    return 1
}
# The whole seconds left until <deadline> (a $SECONDS value), at least 1:
# what an RPC request made against it may take.
evm_time_left() {
    local left=$(($1 - SECONDS))
    [ "$left" -ge 1 ] || left=1
    printf '%s' "$left"
}
# The deadline (a $SECONDS value) of a wait of <seconds> starting now: the
# end of those seconds, or the deadline of the settlement attempt it is part
# of (evm_attempt_deadline, set by evm_settle_pending), if that is sooner.
evm_deadline() {
    local own=$((SECONDS + $1))
    if [ -n "${evm_attempt_deadline:-}" ] && [ "$evm_attempt_deadline" -lt "$own" ]; then
        own="$evm_attempt_deadline"
    fi
    printf '%s' "$own"
}
# evm_sleep_until <deadline> <seconds>: sleep the seconds, or until the
# deadline if that is sooner.
evm_sleep_until() {
    local left=$(($1 - SECONDS))
    [ "$left" -le 0 ] || sleep "$((left < $2 ? left : $2))"
}
# The receipt of the deployer's transaction with <nonce>, <tx> as
# evm_sent_with_nonce shows it, if it is the creation <input>: it executed,
# and created the contract at the address the deployer and the nonce give,
# which has code.
evm_adopt_creation() {
    local nonce="$1" input="$2" tx="$3" deployer address receipt
    deployer="$(evm_deployer)" || return 1
    address="$(cast compute-address "$deployer" --nonce "$nonce" 2>/dev/null | grep -oE '0x[0-9a-fA-F]{40}' | tail -1)"
    [[ "$address" =~ ^0x[0-9a-fA-F]{40}$ ]] || return 1
    jq -e --arg i "$(lower "$input")" '(.to == null) and ((.input // .data // "") | ascii_downcase) == $i' \
        <<<"$tx" >/dev/null || return 1
    receipt="$(evm_receipt "$(jq -r .hash <<<"$tx")" --json)" || return 1
    jq -e --arg a "$(lower "$address")" \
        '(.status | tostring | test("^(0x)?1$")) and ((.contractAddress // "") | ascii_downcase) == $a' \
        <<<"$receipt" >/dev/null || return 1
    evm_wrote "$(evm_block_number "$(jq -r '.blockNumber' <<<"$receipt")")"
    evm_has_code "$address" || return 1
    printf '%s\n' "$receipt"
}
evm_send() {
    local receipt block nonce input step code=0
    input="$(cast calldata "$2" "${@:3}")" || fail "Can't encode $2 for $1"
    step="$(evm_call_step "$1" "$2")"
    evm_settle_pending receipt "$step" "$1" "$input"
    if [ -z "$receipt" ]; then
        nonce="$(evm_next_nonce)" || fail "Can't read the EVM deployer's nonce on $EVM_CHAIN_NAME"
        evm_intend "$step" "$nonce" "$1" "$input" "$2 on $1"
        receipt="$(evm_submit "$nonce" "$1" "$input" cast send "$@" --nonce "$nonce" --rpc-url "$EVM_RPC" \
            --private-key "$EVM_DEPLOYER_KEY" --json)" || code=$?
        [ "$code" != 3 ] && [ "$code" != 4 ] || evm_done "$step"
        [ "$code" = 0 ] || return 1
    fi
    jq -e '.status | tostring | test("^(0x)?1$")' <<<"$receipt" >/dev/null || return 1
    block="$(evm_block_number "$(jq -r '.blockNumber' <<<"$receipt")")" || return 1
    evm_wrote "$block"
    evm_done "$step"
    evm_settle "$block"
}
# A number `cast call` returns, without the scientific form it adds.
evm_number() { local out; out="$(evm_call "$@")" || return 1; awk 'NR == 1 {print $1}' <<<"$out"; }
# Whether there is a contract at an address. A failed lookup stops the script
# rather than count as either.
evm_has_code() {
    local code
    code="$(evm_read cast code "$1")" || fail "Can't read the code at $1 on $EVM_CHAIN_NAME"
    [ "$code" != "0x" ]
}
# A storage slot of an EVM contract.
evm_storage() { evm_read cast storage "$1" "$2"; }
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
# forge_create <variable> <key> <contract> [forge create arguments...]:
# deploy a contract from the Hyperlane Solidity sources, and set <variable>
# to its address, which the caller records at <key> in $STATE with
# evm_record "$(evm_creation_step <key>)". It runs in this shell, not in a
# command substitution, so that the reads after it are made at its block
# (evm_wrote).
forge_create() {
    local _fc_var="$1" _fc_step _fc_contract="$3" _fc_created _fc_address _fc_tx _fc_block _fc_nonce _fc_input _fc_code
    _fc_step="$(evm_creation_step "$2")"
    shift 3
    _fc_input="$(forge_creation_input "$_fc_contract" "$@")" \
        || fail "Can't work out the creation code of $_fc_contract from $SOLIDITY_DIR"
    evm_settle_pending _fc_created "$_fc_step" "" "$_fc_input"
    if [ -z "$_fc_created" ]; then
        _fc_nonce="$(evm_next_nonce)" || fail "Can't read the EVM deployer's nonce on $EVM_CHAIN_NAME"
        evm_intend "$_fc_step" "$_fc_nonce" "" "$_fc_input" "$_fc_contract"
        _fc_code=0
        _fc_created="$(cd "$SOLIDITY_DIR" && evm_submit "$_fc_nonce" "" "$_fc_input" forge create "$_fc_contract" \
            --broadcast --json --nonce "$_fc_nonce" --rpc-url "$EVM_RPC" --private-key "$EVM_DEPLOYER_KEY" "$@")" \
            || _fc_code=$?
        [ "$_fc_code" != 3 ] && [ "$_fc_code" != 4 ] || evm_done "$_fc_step"
        [ "$_fc_code" = 0 ] || return 1
    fi
    _fc_address="$(jq -r '.deployedTo // .contractAddress // empty' <<<"$_fc_created")"
    _fc_tx="$(jq -r '.transactionHash // empty' <<<"$_fc_created")"
    [[ "$_fc_address" =~ ^0x[0-9a-fA-F]{40}$ ]] && [[ "$_fc_tx" =~ ^0x[0-9a-fA-F]{64}$ ]] || return 1
    # The contract exists whatever the endpoint shows next: return it so the
    # caller records it, and a re-run doesn't deploy it again.
    printf -v "$_fc_var" '%s' "$_fc_address"
    if _fc_block="$(evm_tx_block "$_fc_tx")"; then
        evm_wrote "$_fc_block"
        ( evm_settle "$_fc_block" ) || info "Going on without EVM_RPC showing $_fc_tx's block; a check that fails on it is safe to re-run"
    fi
}
# What `forge create <contract> [--constructor-args <args...>]` sends: the
# contract's creation code, then its constructor's arguments, ABI-encoded.
forge_creation_input() {
    local contract="$1" code types args=""
    shift
    [ "${1:-}" != --constructor-args ] || shift
    code="$(cd "$SOLIDITY_DIR" && forge inspect "$contract" bytecode 2>/dev/null)" \
        && [[ "$code" =~ ^0x[0-9a-fA-F]+$ ]] || return 1
    if [ "$#" -gt 0 ]; then
        types="$(cd "$SOLIDITY_DIR" && forge inspect "$contract" abi --json 2>/dev/null \
            | jq -r '[.[] | select(.type == "constructor") | .inputs[].type] | join(",")')" || return 1
        args="$(cast abi-encode "constructor($types)" "$@")" && [[ "$args" =~ ^0x[0-9a-fA-F]*$ ]] || return 1
    fi
    printf '%s%s' "$code" "${args#0x}"
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
# deploy_evm_proxied <base> <key> <label> <implementation>
#     <initialize calldata> [forge create arguments for the implementation...]
# deploys the implementation, unless <base>.<key>_implementation records one,
# then the proxy, and records <base>.<key> and <base>.<key>_creation_tx.
# <base> is where $STATE keeps the contract: .evm, or a token route's
# .token_routes["SYMBOL"].evm.
deploy_evm_proxied() {
    local base="$1" key="$2" label="$3" contract="$4" init="$5" impl code args receipt address tx block nonce sent step
    shift 5
    [[ "$init" =~ ^0x[0-9a-fA-F]{8,}$ ]] || fail "No initialize call for the $label"
    impl="$(state_get "$base.${key}_implementation")"
    if [ -z "$impl" ]; then
        info "Deploying the $label's implementation on $EVM_CHAIN_NAME..."
        forge_create impl "$base.${key}_implementation" "$contract" "$@" \
            || fail "$label implementation deployment failed"
        evm_record "$(evm_creation_step "$base.${key}_implementation")" --arg i "$impl" \
            "$base.${key}_implementation = \$i"
    fi
    evm_has_code "$impl" || fail "No $label implementation at $impl, which $STATE records"
    code="$(evm_proxy_creation_code)" && [[ "$code" =~ ^0x[0-9a-fA-F]+$ ]] \
        || fail "Can't get ERC1967Proxy's creation code from $SOLIDITY_DIR"
    args="$(cast abi-encode "constructor(address,bytes)" "$impl" "$init")" \
        || fail "Can't encode the $label proxy's constructor arguments"
    info "Deploying the $label behind an ERC1967Proxy that initializes it in the same transaction..."
    step="$(evm_creation_step "$base.$key")"
    evm_settle_pending receipt "$step" "" "$code${args#0x}"
    if [ -z "$receipt" ]; then
        nonce="$(evm_next_nonce)" || fail "Can't read the EVM deployer's nonce on $EVM_CHAIN_NAME"
        evm_intend "$step" "$nonce" "" "$code${args#0x}" "the $label's proxy"
        sent=0
        receipt="$(evm_submit "$nonce" "" "$code${args#0x}" cast send --rpc-url "$EVM_RPC" \
            --private-key "$EVM_DEPLOYER_KEY" --json --nonce "$nonce" --create "$code${args#0x}")" || sent=$?
        [ "$sent" != 3 ] && [ "$sent" != 4 ] || evm_done "$step"
        [ "$sent" = 0 ] || fail "$label deployment failed"
    fi
    address="$(jq -r '.contractAddress // empty' <<<"$receipt")"
    tx="$(jq -r '.transactionHash // empty' <<<"$receipt")"
    [[ "$address" =~ ^0x[0-9a-fA-F]{40}$ ]] && [[ "$tx" =~ ^0x[0-9a-fA-F]{64}$ ]] \
        || fail "$label deployment returned no contract address and transaction hash"
    evm_record "$step" --arg a "$address" --arg t "$tx" --arg d "$(evm_deployer)" \
        "$base.$key = \$a | $base.${key}_creation_tx = \$t | .evm.deployer = \$d"
    block="$(evm_block_number "$(jq -r '.blockNumber // empty' <<<"$receipt")")"
    evm_wrote "$block"
    evm_settle "$block"
    check_evm_creation "$base" "$key" "$label"
}

# Prove from the receipt of the transaction that created the recorded
# contract that the deployer sent it, that it created the contract as a proxy
# of the recorded implementation, and that the contract was initialized in
# it, so nobody could have initialized it before. A contract recorded without
# that transaction was deployed by an older version of this script. The
# proxy must still run that implementation and have no admin: an
# ERC1967Proxy has none, and a proxy whose admin can upgrade it isn't one
# this deployment created.
check_evm_creation() {
    local base="$1" key="$2" label="$3" address tx impl creator receipt slot
    address="$(state_get "$base.$key")"
    [ -n "$address" ] || return 0
    tx="$(state_get "$base.${key}_creation_tx")" impl="$(state_get "$base.${key}_implementation")"
    [ -n "$tx" ] && [ -n "$impl" ] \
        || fail "$STATE records the EVM $label $address without the transaction that created and initialized it. An older deploy/testnet.sh initialized it in a second transaction, which anyone could send first, and so choose its owner, its beneficiary or its pricing (docs/security.md, HIGH-8). Move $OUT_DIR aside and deploy again."
    creator="$(evm_creator)" || exit 1
    receipt="$(evm_receipt "$tx" --json)" \
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
    slot="$(evm_storage "$address" "$IMPLEMENTATION_SLOT")" \
        || fail "Can't read the implementation of the EVM $label $address"
    [ "$(lower "0x${slot: -40}")" = "$(lower "$impl")" ] \
        || fail "The EVM $label $address runs the implementation 0x${slot: -40}, not $impl, which it was created with"
    slot="$(evm_storage "$address" "$ADMIN_SLOT")" \
        || fail "Can't read the proxy admin of the EVM $label $address"
    [[ "$slot" =~ ^0x0{64}$ ]] \
        || fail "The EVM $label $address has the proxy admin 0x${slot: -40}, which can change its code: this deployment's ERC1967Proxy has no admin. Move $OUT_DIR aside and deploy again."
}

# The EVM contracts deploy initializes: our IGP, once the state records its
# oracle (before, .evm.igp was Hyperlane's), and each route's token, which
# must also be bound to this deployment (check_evm_token_binding).
check_evm_creations() {
    local route
    [ -z "$(state_get '.evm.storage_gas_oracle')" ] || check_evm_creation .evm igp IGP
    for route in $(recorded_routes); do
        check_evm_creation "$(route_evm_base "$route")" token "$(route_symbol "$route") token"
        check_evm_token_binding "$route"
    done
}

# ── Routes ───────────────────────────────────────────────────────────────────
#
# A route is the native DUSK route, `native`, or a token route,
# `token:SYMBOL`, for a token issued on Dusk (TOKEN_ROUTES). Each is a route
# on Dusk and a HypERC20 on the EVM chain, which $STATE records as:
#
#   native        .dusk.warp_native                .evm.token, .evm.token_*
#   token:SYMBOL  .token_routes.SYMBOL.dusk.route  .token_routes.SYMBOL.evm.token, ...
#
# with a token route's settings, its token on Dusk and the deploy nonces
# under .token_routes.SYMBOL too.

# The routes of the recorded deployment: the native one, then each token
# route deploy finished on Dusk, in the record's order.
recorded_routes() {
    echo native
    state_json | jq -r '.token_routes // {} | to_entries[] | select(.value.dusk.read_back == true) | "token:" + .key'
}
route_symbol() { if [ "$1" = native ]; then echo DUSK; else echo "${1#token:}"; fi; }
# Where $STATE keeps the route's Dusk route ID, and its EVM token.
route_path() { if [ "$1" = native ]; then echo '.dusk.warp_native'; else echo ".token_routes[\"${1#token:}\"].dusk.route"; fi; }
route_evm_base() { if [ "$1" = native ]; then echo '.evm'; else echo ".token_routes[\"${1#token:}\"].evm"; fi; }
# The route as messages name it: the Dusk route, or the Dusk EURd route.
route_name() { if [ "$1" = native ]; then echo "route"; else echo "${1#token:} route"; fi; }
# Each route's token on the EVM chain, one a line, as $STATE records them.
recorded_evm_tokens() {
    local route token
    for route in $(recorded_routes); do
        token="$(state_get "$(route_evm_base "$route").token")"
        [ -z "$token" ] || echo "$token"
    done
}
# What the route's EVM token was initialized with, NAME|SYMBOL|DECIMALS:
# the DUSK token's, or the token route's settings.
route_token_metadata() {
    if [ "$1" = native ]; then
        echo "Dusk|DUSK|9"
    else
        state_json | jq -r --arg s "${1#token:}" '.token_routes[$s] | "\(.name)|\(.symbol)|\(.decimals)"'
    fi
}

# The route's recorded token must be this deployment's route on the EVM
# chain: bound to EVM_MAILBOX (a HypERC20's mailbox() is fixed in its
# implementation), verifying messages from Dusk with the ISM recorded in
# $STATE, a MessageIdMultisig ISM of DUSK_ORIGIN_VALIDATORS and
# DUSK_ORIGIN_THRESHOLD that lists each validator once, with the name,
# symbol and decimals deploy initialized it with and no scaling (Dusk's
# routes move amounts as they are, in the token's smallest unit), and routed
# to the recorded Dusk route or to none yet (check_evm_token_router). A
# token another deployment of the same deployer created, the same way,
# passes the creation checks; this keeps every command from adopting it,
# and deploy from rewiring it, before either uses it.
check_evm_token_binding() {
    local route="$1" label token mailbox metadata name symbol decimals value expected
    token="$(state_get "$(route_evm_base "$route").token")"
    [ -n "$token" ] || return 0
    label="EVM $(route_symbol "$route") token"
    require DUSK_ORIGIN_VALIDATORS DUSK_ORIGIN_THRESHOLD
    mailbox="$(evm_call "$token" "mailbox()(address)")" || fail "Can't read the Mailbox of the $label $token"
    [ "$(lower "$mailbox")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "The $label $token sends and takes its messages through the Mailbox $mailbox, not EVM_MAILBOX $EVM_MAILBOX: it is another deployment's. Move $OUT_DIR aside and deploy again."
    [ -n "$TOKEN_ISMS_CHECKED_ELSEWHERE" ] || check_evm_token_ism "$token" "$label"
    metadata="$(route_token_metadata "$route")"
    IFS='|' read -r name symbol decimals <<<"$metadata"
    for expected in "name()(string)=$name" "symbol()(string)=$symbol" "decimals()(uint8)=$decimals" \
        "scaleNumerator()(uint256)=1" "scaleDenominator()(uint256)=1"; do
        if [[ "$expected" == *"(string)="* ]]; then
            value="$(evm_call "$token" "${expected%%=*}" --json | jq -r '.[0] // empty')"
        else
            value="$(evm_number "$token" "${expected%%=*}")"
        fi || fail "Can't read the ${expected%%(*} of the $label $token"
        [ "$value" = "${expected#*=}" ] \
            || fail "The $label $token's ${expected%%(*} is '$value', not '${expected#*=}', which deploy initialized it with: it is another deployment's, or another token's. Move $OUT_DIR aside and deploy again."
    done
    check_evm_token_router "$route" "$token"
}
# set-validators moves the tokens from one ISM to another, and checks their
# ISMs itself (cmd_set_validators): while it does, the bindings check the
# rest of each token.
TOKEN_ISMS_CHECKED_ELSEWHERE=""
# The token verifies messages from Dusk with the ISM recorded in $STATE, of
# DUSK_ORIGIN_THRESHOLD of DUSK_ORIGIN_VALIDATORS.
check_evm_token_ism() {
    local token="$1" label="$2" ism recorded
    ism="$(evm_call "$token" "interchainSecurityModule()(address)")" \
        || fail "Can't read the ISM of the $label $token"
    recorded="$(state_get '.evm.ism')"
    [ "$(lower "$ism")" = "$(lower "$recorded")" ] \
        || fail "The $label $token verifies messages from Dusk with the ISM $ism, not $recorded recorded in $STATE"
    check_evm_ism_policy "$ism" "The $label's ISM $ism"
}
# The EVM ISM <ism> is a MessageIdMultisig ISM that takes exactly
# DUSK_ORIGIN_THRESHOLD of DUSK_ORIGIN_VALIDATORS. <what> names it in what
# fails.
check_evm_ism_policy() {
    local ism="$1" what="$2" type policy validators repeated threshold expected
    type="$(evm_number "$ism" "moduleType()(uint8)")" || fail "Can't read the module type of the EVM ISM $ism"
    [ "$type" = "$MESSAGE_ID_MULTISIG_ISM" ] \
        || fail "$what is of module type $type; it must be a MessageIdMultisig ISM ($MESSAGE_ID_MULTISIG_ISM)"
    policy="$(evm_ism_policy "$ism")" || exit 1
    validators="${policy#* of [}" validators="${validators%]}" threshold="${policy%% of *}"
    repeated="$(repeated_addresses "$validators")"
    [ -z "$repeated" ] \
        || fail "$what lists [$repeated] more than once among its validators. A MessageIdMultisig ISM counts a signature for each place its validator is listed, so fewer than DUSK_ORIGIN_THRESHOLD validators can sign for a message. This deployment's ISM lists each once: it is another's. Move $OUT_DIR aside and deploy again."
    validators="$(sorted_addresses "$validators")"
    expected="$(sorted_addresses "$DUSK_ORIGIN_VALIDATORS")"
    [ "$validators" = "$expected" ] && [ "$threshold" = "$DUSK_ORIGIN_THRESHOLD" ] \
        || fail "$what needs $threshold of [$validators], but DUSK_ORIGIN_THRESHOLD and DUSK_ORIGIN_VALIDATORS say $DUSK_ORIGIN_THRESHOLD of [$expected]"
}
# What an EVM multisig ISM takes, as "<threshold> of [<validators>]": its
# validators in its order, comma-separated and lowercase, each place it
# lists one.
evm_ism_policy() {
    local policy validators threshold
    policy="$(evm_call "$1" "validatorsAndThreshold(bytes)(address[],uint8)" 0x)" \
        || fail "Can't read the validators and threshold of the EVM ISM $1"
    validators="$(sed -n 1p <<<"$policy" | tr -d '[] ' | tr '[:upper:]' '[:lower:]')"
    threshold="$(sed -n 2p <<<"$policy" | awk '{print $1}')"
    [[ "$threshold" =~ ^[0-9]+$ ]] || fail "The EVM ISM $1 returned a malformed validator set"
    printf '%s of [%s]' "$threshold" "$validators"
}

# The token's router for Dusk must be its route's Dusk route recorded in
# $STATE, or none yet, and with none the token must have no supply.
# Deployments by the same deployer through the same Mailbox and validators
# share the ISM, so another deployment's token passes the binding checks;
# its router is that deployment's route, whose custody backs its supply.
# Enrolling this route on it would let those tokens be redeemed for what
# this route holds, and so would supply minted before any route was
# enrolled, which nothing backs. deploy creates each token with no supply
# and enrolls its route after it, so a new or resumed deployment passes.
# Runs at preflight, and again right before deploy writes to the token.
# Sets EVM_TOKEN_ROUTER to the router it read.
EVM_TOKEN_ROUTER=""
check_evm_token_router() {
    local route="$1" token="$2" label dusk_route router supply
    EVM_TOKEN_ROUTER=""
    label="EVM $(route_symbol "$route") token"
    dusk_route="$(lower "$(state_get "$(route_path "$route")")")"
    router="$(evm_call "$token" "routers(uint32)(bytes32)" "$DUSK_DOMAIN")" \
        || fail "Can't read the Dusk router of the $label $token"
    router="$(lower "$router")"
    if [ -n "$dusk_route" ] && [ "$router" = "0x$dusk_route" ]; then
        EVM_TOKEN_ROUTER="$router"
        return 0
    fi
    [[ "$router" =~ ^0x0{64}$ ]] \
        || fail "The $label $token is enrolled with the Dusk route ${router#0x}, not the route recorded in $STATE (${dusk_route:-none}): it is another deployment's, or another route's, and that route's custody backs its tokens. Move $OUT_DIR aside and deploy again."
    supply="$(evm_number "$token" "totalSupply()(uint256)")" && [[ "$supply" =~ ^[0-9]+$ ]] \
        || fail "Can't read the supply of the $label $token"
    [ "$supply" = 0 ] \
        || fail "The $label $token has a supply of $supply but no Dusk route: nothing on Dusk backs those tokens, and enrolling this deployment's route would let them be redeemed for what it holds. deploy creates the token with none. Move $OUT_DIR aside and deploy again."
    EVM_TOKEN_ROUTER="$router"
}

sorted_addresses() {
    tr ',' '\n' <<<"$1" | tr '[:upper:]' '[:lower:]' | sed 's/^ *//; s/ *$//' | grep -v '^$' | sort -u | paste -sd, -
}
# The addresses a list names more than once, in any case, comma-separated,
# or nothing. sorted_addresses folds them into one, so a validator set an ISM
# reads on chain is checked with this first: Hyperlane's MessageIdMultisig
# ISMs take a signature for each place a validator is listed, and [A,A,B]
# at threshold 2 accepts A's signature twice.
repeated_addresses() {
    tr ',' '\n' <<<"$1" | tr '[:upper:]' '[:lower:]' | sed 's/^ *//; s/ *$//' | awk 'NF' | sort | uniq -d | paste -sd, -
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
        TOKEN_ROUTE_COUNT="$(token_route_count)" \
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
# MessageId multisig ISM and a HypERC20's handle, as the relayer estimates it
# (eth_estimateGas): at most this, plus this per signature and per
# validator. A delivery stores a new delivery record, a mint to a new holder
# a new balance, and a token's first mint a new supply. Sepolia charges
# about 111,000 gas for each new storage slot (Anvil, and Sepolia before its
# Glamsterdam upgrade, 22,100), so it estimated a delivery of a live
# transfer, at 1 of 2, at 218,195 gas to a holder, 320,137 to a new one and
# 424,972 as the token's first mint (docs/costs.md, Fee coverage). The
# local rehearsal, on Anvil, measured 140,569 at 2 of 3.
EVM_DELIVERY_BASE, EVM_DELIVERY_PER_SIGNATURE, EVM_DELIVERY_PER_VALIDATOR = 420_000, 7_500, 2_500
# A relayed process on Dusk: at most this, plus this per signature the Dusk
# ISM checks, the route's handle included. A token route's collateral route
# pays out with its token's transfer, which costs more than the native
# route's payout, so a deployment with token routes is priced at their base
# (test_relayed_deliveries_cost_no_more_than_deploy_testnet_sh_prices: a
# payout from a token route took 2,183,528 gas at 1 signature).
DUSK_PROCESS_BASE_NATIVE, DUSK_PROCESS_BASE_TOKEN_ROUTE, DUSK_PROCESS_PER_SIGNATURE = 1_950_000, 1_980_000, 220_000

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
token_routes = int(s["TOKEN_ROUTE_COUNT"] or 0)
dusk_process_base = DUSK_PROCESS_BASE_TOKEN_ROUTE if token_routes else DUSK_PROCESS_BASE_NATIVE
dusk_process = dusk_process_base + DUSK_PROCESS_PER_SIGNATURE * evm_threshold
if evm_overhead < dusk_process:
    errors.append(
        f"EVM_IGP_GAS_OVERHEAD is {evm_overhead:,}, but a relayed process with "
        f"EVM_ORIGIN_THRESHOLD {evm_threshold} takes about {dusk_process:,} Dusk gas "
        f"({dusk_process_base:,} plus {DUSK_PROCESS_PER_SIGNATURE:,} per signature"
        + (", for a payout from a token route, which the deployment has" if token_routes else "")
        + "). Raise it.")
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
        # The costliest route the price covers: a token route's payout, or
        # the native route's.
        "costliest_route": "token" if token_routes else "native",
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
        f"{e['delivery_gas']:,} gas"
        + (f" (a payout from a token route, the costliest of the {token_routes + 1} routes)" if token_routes else ""),
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
# this deployment takes its calls from the Mailbox and is its required or
# default hook, or takes them from one of those hooks, which takes its own
# from the Mailbox and calls it. One that names the Mailbox, which never
# calls it, is detached: another deployment's, or none.
check_dusk_hook_binding() {
    local name="$1" contract="$2" mailbox="$3" bound method hook hook_bound hooks
    bound="$(dusk_value "the caller of the Dusk $name $contract" \
        --contract "$contract" --method mailbox --return-type bytes32)" || exit 1
    for method in required_hook default_hook; do
        hook="$(dusk_value "the $method of the Dusk Mailbox $mailbox" \
            --contract "$mailbox" --method "$method" --return-type bytes32)" || exit 1
        if [ "$bound" = "$mailbox" ]; then
            [ "$contract" != "$hook" ] || return 0
            continue
        fi
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
    [ "$bound" != "$mailbox" ] \
        || fail "The Dusk $name $contract takes its calls from the Mailbox $mailbox, which doesn't call it: it is neither the Mailbox's required hook nor its default hook"
    fail "The Dusk $name $contract takes its calls from $bound, which is neither the Mailbox $mailbox recorded in $STATE nor one of its hooks"
}

# The Dusk contracts the fee commands use must be this deployment's: the
# recorded Mailbox on the expected chain at DUSK_DOMAIN, the contracts at
# this checkout's state versions, the IGP and ProtocolFee taking their
# calls from that Mailbox or its hooks (check_dusk_hook_binding), and every
# contract reading back as deploy left it (check_recorded_dusk_contracts).
# With `keys`, their owner must also be DUSK_KEYS's account.
check_dusk_deployment() {
    local mailbox domain
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
    check_recorded_dusk_contracts "${1:-}"
}

# The recorded Dusk deployment: the fee contracts' bindings, then the
# read-back of every contract against the owner deploy recorded. A command
# reads the same contracts back once: dusk_read_back records which contracts
# and owner read back (DUSK_READ_BACK_OF), and whether that owner was
# DUSK_KEYS's (DUSK_READ_BACK is `keys`) or only the recorded one (`owner`).
# A record that names other contracts or another owner is read back again.
DUSK_READ_BACK="" DUSK_READ_BACK_OF=""
dusk_read_back() {
    DUSK_READ_BACK="$1"
    DUSK_READ_BACK_OF="$(dusk_read_back_record "$2" "$3")"
}
# What a read-back covers: the core contracts and the native route, each
# token route deploy finished on Dusk, and the owner.
dusk_read_back_record() {
    state_json | jq -cS --argjson d "$1" --arg o "$2" \
        '{dusk: $d, owner: $o, owner_key: (.dusk_owner_key // ""),
          token_routes: (.token_routes // {} | with_entries(select(.value.dusk.read_back == true))
                         | map_values({name, symbol, decimals, supply, dusk, token: .evm.token}))}'
}
check_recorded_dusk_contracts() {
    local mailbox name contract owner contracts record route owner_key
    mailbox="$(state_get '.dusk.mailbox')"
    for name in igp protocol_fee; do
        contract="$(state_get ".dusk.$name")"
        [ -n "$contract" ] || continue
        check_dusk_hook_binding "$name" "$contract" "$mailbox"
    done
    contracts="$(state_json | jq -c '.dusk')"
    record="$(dusk_read_back_record "$contracts" "$(state_get '.dusk_owner')")"
    if [ "$DUSK_READ_BACK_OF" = "$record" ] \
        && { [ "$DUSK_READ_BACK" = keys ] || [ "${1:-}" != keys ]; }; then
        return 0
    fi
    owner="$(recorded_dusk_owner "${1:-}")" || exit 1
    check_dusk_contracts "$contracts" "$owner" recorded
    for route in $(recorded_routes); do
        [ "$route" != native ] || continue
        owner_key="$(recorded_dusk_owner_key "$owner")" || exit 1
        check_dusk_token_route "$route" "$owner" "$owner_key" recorded
    done
    dusk_read_back "${1:-owner}" "$contracts" "$owner"
}

# DUSK_KEYS's account, as `dusk-tx account` prints it: its address, public
# key and hash.
deployer_identity() {
    local out
    out="$(dusk_tx account --offline --keys "$DUSK_KEYS" 2>/dev/null)" \
        && jq -e '(.account_h256 | test("^[0-9a-f]{64}$")) and (.public_key | test("^[0-9a-f]{192}$"))' \
            <<<"$out" >/dev/null 2>&1 \
        || { echo "[FAIL] Can't read the account of DUSK_KEYS $DUSK_KEYS: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)" >&2; return 1; }
    printf '%s' "$out"
}

# The public key of the owner of the recorded Dusk contracts, whose hash is
# `owner`. A token route's canonical DRC20 names its admin by the key, not
# the hash, so deploy records it (.dusk_owner_key) before its first token
# route. Its hash must be the owner's.
recorded_dusk_owner_key() {
    local owner="$1" key out
    key="$(state_get '.dusk_owner_key')"
    [[ "$key" =~ ^[0-9a-f]{192}$ ]] || {
        echo "[FAIL] $STATE records token routes but not the public key of their owner (.dusk_owner_key), which their tokens name as admin. Run deploy, which records it from DUSK_KEYS." >&2
        return 1
    }
    out="$("$DUSK_TX" account --offline --public-key "$key" 2>/dev/null)" \
        && [ "$(jq -r '.account_h256 // empty' <<<"$out" 2>/dev/null)" = "$owner" ] || {
        echo "[FAIL] $STATE records the Dusk contracts' owner as the account with hash $owner, but its key $key as another's" >&2
        return 1
    }
    printf '%s' "$key"
}

# The account hash of DUSK_KEYS: the owner dusk-tx deploy-hyperlane gives
# every Dusk contract it deploys, and the fee contracts' first beneficiary.
deployer_account() {
    local out hash
    out="$(dusk_tx account --offline --keys "$DUSK_KEYS" 2>/dev/null)" \
        && hash="$(jq -er '.account_h256' <<<"$out" 2>/dev/null)" && [[ "$hash" =~ ^[0-9a-f]{64}$ ]] \
        || { echo "[FAIL] Can't read the account of DUSK_KEYS $DUSK_KEYS: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)" >&2; return 1; }
    printf '%s' "$hash"
}

# The owner the recorded Dusk contracts must have: the account deploy read
# back as every contract's owner when it recorded them (.dusk_owner). An
# older deploy/testnet.sh recorded contracts without reading them back, so a
# deployment recorded without it is refused. With `keys`, it must also be
# DUSK_KEYS's account.
recorded_dusk_owner() {
    local recorded keys
    recorded="$(state_get '.dusk_owner')"
    [[ "$recorded" =~ ^[0-9a-f]{64}$ ]] || {
        echo "[FAIL] $STATE records Dusk contracts but not the owner deploy read back from them: an older deploy/testnet.sh recorded them without reading back what it deployed, and whoever deployed or initialized one of them first chose its owner and bindings. Move $OUT_DIR aside and deploy again." >&2
        return 1
    }
    if [ "${1:-}" = keys ]; then
        keys="$(deployer_account)" || return 1
        [ "$keys" = "$recorded" ] || {
            echo "[FAIL] $STATE records the Dusk contracts as owned by the account with hash $recorded, not DUSK_KEYS's ($keys): it is another deployer's deployment. Use that deployer's DUSK_KEYS, or another DEPLOY_NAME." >&2
            return 1
        }
    fi
    printf '%s' "$recorded"
}

# The state_version of each Dusk contract deploy-hyperlane deploys besides
# the core contracts and routes above; none has had another.
OTHER_STATE_VERSIONS="aggregation_hook=1 ism_multisig=1 protocol_fee=1 test_mock=1 test_recipient=1"
# The ProtocolFee deploy-hyperlane deploys: 0.001 DUSK a dispatch, which its
# owner can raise to at most 0.1 DUSK.
DUSK_PROTOCOL_FEE=1000000
DUSK_MAX_PROTOCOL_FEE=100000000

# Read back the Dusk contracts in `contracts`, a JSON object of IDs by name
# as dusk-tx deploy-hyperlane prints them and $STATE records them, against
# what deploy-hyperlane deploys. Someone can deploy at the IDs first, or
# front-run a contract's init, and choose its owner and bindings
# (docs/testnet.md, Before mainnet). So every contract's owner must be
# `owner`, an account hash, and each binding its init set must hold: the
# Mailbox's domain and hooks, the aggregation hook's Mailbox and hooks, the
# Merkle hook's, the fee contracts' and ValidatorAnnounce's callers, the
# route's Mailbox and hook, TestRecipient's ISM, and every contract's state
# version. The route has no router for Dusk, and for EVM_DOMAIN none or the
# recorded token.
#
# `fresh`, right after deploy-hyperlane, also reads what init set and only
# the owner can change later: the Mailbox's default ISM, the ISM's
# validators and threshold, the fee contracts' beneficiary, the ProtocolFee's
# fee, the IGP's pricing and the route's own ISM, and the route has no
# router yet. `recorded` leaves those to the checks that hold them to the
# settings (check_deployed_ism, check_dusk_igp_pricing), since the owner can
# change them on purpose, and the core contracts' and routes' state
# versions and the Mailbox's domain to its callers, which read them first
# (check_state_versions).
#
# Routes have no view that lists their routers, so only the domains above
# are read; dusk-tx deploy-hyperlane reads the same, for each domain it
# prices. Each failed read stops the script, as a check that can't read the
# chain must not pass.
check_dusk_contracts() {
    local contracts="$1" owner="$2" mode="$3" name id entry value expected token hint versions
    local -A c=()
    if [ "$mode" = fresh ]; then
        hint="These aren't the contracts deploy-hyperlane sent: a deploy or a contract's init may have been front-run, and whoever came first chose its owner and bindings. Nothing was recorded. Don't use them: move $OUT_DIR aside and deploy again with another DUSK_KEYS."
        DUSK_READ_HINT="Nothing was recorded: run deploy again to read the contracts back again"
    else
        hint="$STATE records a Dusk deployment that isn't as deploy left it: it is another deployment's, or its owner changed it, which deploy/testnet.sh never does. Nothing was sent. Find out which; to start over, move $OUT_DIR aside and deploy again."
    fi
    for name in mailbox merkle_tree_hook aggregation_hook protocol_fee igp validator_announce \
        ism_multisig test_mock test_recipient warp_native; do
        id="$(jq -r --arg n "$name" '.[$n] // empty' <<<"$contracts")"
        [[ "$id" =~ ^[0-9a-f]{64}$ ]] || fail "The Dusk deployment names no $name ('$id'). $hint"
        c[$name]="$id"
    done

    versions="$OTHER_STATE_VERSIONS"
    [ "$mode" = recorded ] || versions="$CORE_STATE_VERSIONS $ROUTE_STATE_VERSIONS $versions"
    for entry in $versions; do
        name="${entry%%=*}" expected="${entry#*=}"
        [ -n "${c[$name]:-}" ] || continue
        dusk_expect "$name" "${c[$name]}" state_version u32 "$expected" "this checkout's" "$hint"
    done
    for name in mailbox ism_multisig protocol_fee igp warp_native; do
        value="$(dusk_owner_of "$name" "${c[$name]}")" || exit 1
        [ "$value" = "$owner" ] \
            || fail "The Dusk $name ${c[$name]}'s owner is $value, not the deployer's account $owner. $hint"
    done
    # Bindings nobody can change after init.
    [ "$mode" = recorded ] \
        || dusk_expect mailbox "${c[mailbox]}" local_domain u32 "$DUSK_DOMAIN" "DUSK_DOMAIN" "$hint"
    dusk_expect merkle_tree_hook "${c[merkle_tree_hook]}" mailbox bytes32 "${c[aggregation_hook]}" \
        "the aggregation hook, which calls it" "$hint"
    dusk_expect aggregation_hook "${c[aggregation_hook]}" mailbox bytes32 "${c[mailbox]}" "the Mailbox" "$hint"
    dusk_expect aggregation_hook "${c[aggregation_hook]}" hooks contract-id-list \
        "[\"${c[merkle_tree_hook]}\",\"${c[protocol_fee]}\"]" "the Merkle hook, then the ProtocolFee" "$hint"
    dusk_expect protocol_fee "${c[protocol_fee]}" mailbox bytes32 "${c[aggregation_hook]}" \
        "the aggregation hook, which calls it" "$hint"
    dusk_expect protocol_fee "${c[protocol_fee]}" max_protocol_fee u64 "$DUSK_MAX_PROTOCOL_FEE" \
        "the most deploy-hyperlane lets its owner charge" "$hint"
    dusk_expect igp "${c[igp]}" mailbox bytes32 "${c[mailbox]}" "the Mailbox" "$hint"
    dusk_expect validator_announce "${c[validator_announce]}" mailbox bytes32 "${c[mailbox]}" "the Mailbox" "$hint"
    dusk_expect validator_announce "${c[validator_announce]}" local_domain u32 "$DUSK_DOMAIN" "DUSK_DOMAIN" "$hint"
    dusk_expect warp_native "${c[warp_native]}" mailbox bytes32 "${c[mailbox]}" "the Mailbox" "$hint"
    dusk_expect test_mock "${c[test_mock]}" module_type u8 "$TEST_MOCK_ISM" "TestMock's" "$hint"
    dusk_expect test_recipient "${c[test_recipient]}" interchain_security_module bytes32 "$ZERO_ID" \
        "none: the Mailbox's default ISM" "$hint"
    # Bindings only the owner can change, which deploy/testnet.sh never does.
    dusk_expect mailbox "${c[mailbox]}" required_hook bytes32 "${c[aggregation_hook]}" "the aggregation hook" "$hint"
    dusk_expect mailbox "${c[mailbox]}" default_hook bytes32 "${c[igp]}" "the IGP" "$hint"
    dusk_expect warp_native "${c[warp_native]}" hook bytes32 "$ZERO_ID" "none: the Mailbox's default hook, the IGP" "$hint"
    dusk_expect warp_native "${c[warp_native]}" enrolled_router bytes32 "$ZERO_ID" "none" "$hint" \
        --arg-u32 "$DUSK_DOMAIN"
    value="$(dusk_value "the router of the Dusk route ${c[warp_native]} for $EVM_CHAIN_NAME" \
        --contract "${c[warp_native]}" --method enrolled_router --return-type bytes32 --arg-u32 "$EVM_DOMAIN")" || exit 1
    token="$(state_get '.evm.token')"
    if [ "$mode" = fresh ]; then
        expected="none: deploy-hyperlane enrolls none"
    elif [ -n "$token" ]; then
        expected="none or the EVM DUSK token $token recorded in $STATE"
        [ "$value" != "$(pad32 "$token")" ] || value="$ZERO_ID"
    else
        expected="none: $STATE records no EVM DUSK token"
    fi
    [ "$value" = "$ZERO_ID" ] \
        || fail "The Dusk route ${c[warp_native]} is enrolled with the router $value for $EVM_CHAIN_NAME (domain $EVM_DOMAIN), not $expected. $hint"
    [ "$mode" = fresh ] || return 0

    # What init set and only the owner can change later.
    dusk_expect mailbox "${c[mailbox]}" default_ism bytes32 "${c[ism_multisig]}" "the ISM deploy-hyperlane deployed" "$hint"
    dusk_expect ism_multisig "${c[ism_multisig]}" module_type u8 "$MESSAGE_ID_MULTISIG_ISM" "a MessageIdMultisig ISM's" "$hint"
    value="$(dusk_value "the validators and threshold of the Dusk ISM ${c[ism_multisig]}" \
        --contract "${c[ism_multisig]}" --method validators_and_threshold --return-type validators-and-threshold)" || exit 1
    value="$(jq -er '"\(.threshold) of [\(.validators | map(ascii_downcase) | join(","))]"' <<<"$value")" \
        || fail "The Dusk ISM ${c[ism_multisig]} returned a malformed validator set"
    expected="$EVM_ORIGIN_THRESHOLD of [$(sorted_addresses "$EVM_ORIGIN_VALIDATORS")]"
    [ "$value" = "$expected" ] \
        || fail "The Dusk ISM ${c[ism_multisig]} needs $value, not $expected (EVM_ORIGIN_THRESHOLD of EVM_ORIGIN_VALIDATORS). $hint"
    dusk_expect protocol_fee "${c[protocol_fee]}" beneficiary bytes32 "$owner" "the deployer's account" "$hint"
    dusk_expect protocol_fee "${c[protocol_fee]}" protocol_fee u64 "$DUSK_PROTOCOL_FEE" "deploy-hyperlane's" "$hint"
    dusk_expect igp "${c[igp]}" beneficiary bytes32 "$owner" "the deployer's account" "$hint"
    value="$(dusk_value "the Dusk IGP's pricing for domain $EVM_DOMAIN" --contract "${c[igp]}" \
        --method domain_gas_config --return-type domain-gas-config --arg-u32 "$EVM_DOMAIN")" || exit 1
    value="$(jq -cS '{gas_overhead, token_exchange_rate, gas_price} | map_values(tostring)' <<<"$value")"
    expected="$(fee '.dusk_igp | {gas_overhead, token_exchange_rate, gas_price}' | jq -cS .)"
    [ "$value" = "$expected" ] \
        || fail "The Dusk IGP ${c[igp]} prices domain $EVM_DOMAIN at $value, not the settings' $expected. $hint"
    dusk_expect warp_native "${c[warp_native]}" interchain_security_module bytes32 "$ZERO_ID" \
        "none: the Mailbox's default ISM" "$hint"
    DUSK_READ_HINT=""
}

# Fail unless the view `method` of the Dusk contract `name` at `id` returns
# `expected` (as dusk_value prints it); `what` says what that is, and
# `hint` what to do. Arguments after those go to the query.
dusk_expect() {
    local name="$1" id="$2" method="$3" type="$4" expected="$5" what="$6" hint="$7" value
    shift 7
    value="$(dusk_value "the $method of the Dusk $name $id" \
        --contract "$id" --method "$method" --return-type "$type" "$@")" || exit 1
    [ "$value" = "$expected" ] || fail "The Dusk $name $id's $method is $value, not $expected ($what). $hint"
}

# A Dusk contract's owner(): an account hash, or "nobody" once ownership is
# renounced.
dusk_owner_of() {
    local out value
    out="$(dusk_query --contract "$2" --method owner --return-type option-bytes32 2>/dev/null)" \
        && value="$(jq -er 'if has("value") then (.value // "nobody") else error("no value") end' <<<"$out" 2>/dev/null)" \
        || { echo "[FAIL] Can't read the owner of the Dusk $1 $2: $(jq -r '.error // "the query returned no value"' <<<"$out" 2>/dev/null)${DUSK_READ_HINT:+. $DUSK_READ_HINT}" >&2; return 1; }
    [ "$value" = nobody ] || [[ "$value" =~ ^[0-9a-f]{64}$ ]] \
        || { echo "[FAIL] The Dusk $1 $2 returned a malformed owner ($value)" >&2; return 1; }
    printf '%s' "$value"
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

# ── Token routes ─────────────────────────────────────────────────────────────
#
# TOKEN_ROUTES lists tokens issued on Dusk that get a route to the EVM chain,
# one a line, SYMBOL|NAME|DECIMALS|SUPPLY (deploy/testnet.env.example). Each
# is the pinned canonical DRC20 on Dusk, with DUSK_KEYS its admin, holding
# every role and the initial SUPPLY, locked by a WarpDrc20Collateral bound
# to the deployment's Mailbox, and on the EVM chain a synthetic HypERC20 of
# the same name, symbol and decimals. deploy finishes every token route
# $STATE records and adds those TOKEN_ROUTES lists that it doesn't, and
# leaves the rest of the deployment as it is.

# The deploy nonce of the first token route's DRC20: the core contracts take
# 0 to 9. Each route takes two, its token's and the next for its route.
TOKEN_ROUTE_FIRST_NONCE=10
# The largest supply a canonical DRC20 holds: a u64.
U64_MAX=18446744073709551615

# TOKEN_ROUTES as JSON, [{symbol, name, decimals, supply}], once
# parse_token_routes has read it.
TOKEN_ROUTES_JSON="[]"
parse_token_routes() {
    local line n=0 symbol name decimals supply extra seen=" " json="[]"
    while IFS= read -r line || [ -n "$line" ]; do
        n=$((n + 1))
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        [ -n "$line" ] && [[ "$line" != \#* ]] || continue
        IFS='|' read -r symbol name decimals supply extra <<<"$line"
        symbol="$(trim "$symbol")" name="$(trim "$name")" decimals="$(trim "$decimals")" supply="$(trim "$supply")"
        [ -z "$extra" ] && [ -n "$supply" ] \
            || fail "TOKEN_ROUTES line $n ('$line') isn't SYMBOL|NAME|DECIMALS|SUPPLY"
        [[ "$symbol" =~ ^[A-Za-z][A-Za-z0-9]{0,10}$ ]] \
            || fail "TOKEN_ROUTES line $n: the symbol '$symbol' must be a letter and up to 10 more letters or digits"
        [ "${symbol^^}" != DUSK ] \
            || fail "TOKEN_ROUTES line $n: DUSK is the native route's token"
        [[ "$seen" != *" ${symbol^^} "* ]] \
            || fail "TOKEN_ROUTES lists the symbol $symbol twice (case doesn't tell symbols apart)"
        seen+="${symbol^^} "
        [ -n "$name" ] && [ "${#name}" -le 64 ] && [[ "$name" != *[[:cntrl:]]* ]] \
            || fail "TOKEN_ROUTES line $n: the name '$name' must be 1 to 64 printable characters"
        [[ "$decimals" =~ ^[0-9]{1,2}$ ]] && [ "$((10#$decimals))" -le 18 ] \
            || fail "TOKEN_ROUTES line $n: the decimals '$decimals' must be a whole number from 0 to 18"
        [[ "$supply" =~ ^(0|[1-9][0-9]{0,19})$ ]] \
            && { [ "${#supply}" -lt 20 ] || [[ "$supply" < "$U64_MAX" ]] || [ "$supply" = "$U64_MAX" ]; } \
            || fail "TOKEN_ROUTES line $n: the supply '$supply' must be a whole number of the token's smallest unit, at most $U64_MAX"
        json="$(jq -c --arg s "$symbol" --arg n "$name" --argjson d "$((10#$decimals))" --arg u "$supply" \
            '. + [{symbol: $s, name: $n, decimals: $d, supply: $u}]' <<<"$json")"
    done <<<"${TOKEN_ROUTES:-}"
    TOKEN_ROUTES_JSON="$json"
}
trim() { local v="$1"; v="${v#"${v%%[![:space:]]*}"}"; printf '%s' "${v%"${v##*[![:space:]]}"}"; }

# A route $STATE records must keep its settings: its tokens can't change
# their name, symbol or decimals, and its initial supply is minted. A
# TOKEN_ROUTES line for it that differs is refused, rather than taken as a
# route to change; one that $STATE has and TOKEN_ROUTES doesn't is still
# checked, and finished by deploy.
check_token_route_settings() {
    local symbol settings recorded other
    for symbol in $(jq -r '.[].symbol' <<<"$TOKEN_ROUTES_JSON"); do
        other="$(state_json | jq -r --arg s "$symbol" \
            '.token_routes // {} | keys_unsorted[] | select(ascii_downcase == ($s | ascii_downcase) and . != $s)')"
        [ -z "$other" ] \
            || fail "TOKEN_ROUTES lists $symbol, and $STATE records the token route $other: case doesn't tell symbols apart. Write it $other."
        recorded="$(state_json | jq -c --arg s "$symbol" '.token_routes[$s] // empty | {symbol, name, decimals, supply}')"
        [ -n "$recorded" ] || continue
        settings="$(jq -c --arg s "$symbol" '.[] | select(.symbol == $s)' <<<"$TOKEN_ROUTES_JSON")"
        [ "$(jq -cS . <<<"$recorded")" = "$(jq -cS . <<<"$settings")" ] \
            || fail "TOKEN_ROUTES has $symbol as $(jq -r '"\(.name)|\(.decimals)|\(.supply)"' <<<"$settings"), but $STATE records its route as $(jq -r '"\(.name)|\(.decimals)|\(.supply)"' <<<"$recorded"). A token's name and decimals can't change, and its initial supply is minted: put the line back as it was, and mint more with dusk-tx drc20-mint (docs/testnet.md)."
    done
    for symbol in $(state_json | jq -r '.token_routes // {} | keys_unsorted[]'); do
        jq -e --arg s "$symbol" 'any(.[]; .symbol == $s)' <<<"$TOKEN_ROUTES_JSON" >/dev/null \
            || info "$STATE has the token route $symbol, which TOKEN_ROUTES doesn't list: it is checked, and deploy finishes it"
    done
}

# The token routes deploy works on: the ones $STATE records, then the new
# ones TOKEN_ROUTES lists.
token_routes_to_deploy() {
    { state_json | jq -r '.token_routes // {} | keys_unsorted[]'; jq -r '.[].symbol' <<<"$TOKEN_ROUTES_JSON"; } \
        | awk '!seen[$0]++'
}
# Token routes $STATE doesn't record as deployed on Dusk yet, that deploy
# would deploy there.
pending_token_routes() {
    local symbol
    for symbol in $(token_routes_to_deploy); do
        [ "$(state_get ".token_routes[\"$symbol\"].dusk.read_back")" = true ] || echo "$symbol"
    done
}
# How many token routes the deployment has or deploy would add, which the
# pricing of deliveries to Dusk depends on (compute_fees).
token_route_count() { token_routes_to_deploy | grep -c . || true; }

# The deploy nonce of a new token route's DRC20: past every nonce $STATE's
# token routes use, or have left to someone else's contracts.
next_token_route_nonce() {
    state_json | jq -r --argjson first "$TOKEN_ROUTE_FIRST_NONCE" \
        '[.token_routes // {} | .[].dusk | (.deploy_nonce // empty), (.abandoned // [] | .[]) | . + 2] + [$first] | max'
}

# Record DUSK_KEYS's public key, the token routes' admin, before deploy
# deploys the first: a canonical DRC20 names its admin by the key, and the
# commands that don't read DUSK_KEYS check the admin with the recorded one.
record_dusk_owner_key() {
    local identity
    identity="$(deployer_identity)" || exit 1
    [ "$(jq -r .account_h256 <<<"$identity")" = "$(state_get '.dusk_owner')" ] \
        || fail "DUSK_KEYS's account isn't the owner $STATE records for the Dusk contracts"
    [ "$(state_get '.dusk_owner_key')" = "$(jq -r .public_key <<<"$identity")" ] \
        || state_set --arg k "$(jq -r .public_key <<<"$identity")" '.dusk_owner_key = $k'
}

# Deploy a token route on Dusk: plan its deploy nonces and contract IDs, and
# record them with its settings before anything is sent, then deploy the
# token and its route with dusk-tx deploy-token-route, which deploys only
# what isn't at the planned IDs yet and reads both back, then read them
# back here against $STATE and record them as deployed. A run that stops
# anywhere in between resumes at the same IDs. deploy-token-route writes
# each deploy transaction to the route's tx log before it sends it, and the
# transactions for the planned IDs go into $STATE: a contract already at a
# planned ID is taken only if one of them created it, in a final block. One
# that none created is someone else's, deployed there first with an init of
# their choosing, whatever it reads back (docs/testnet.md, Before mainnet):
# the pair of nonces is left, and the next two planned, before anything
# else is sent. The plan also skips nonces whose IDs are taken already.
deploy_dusk_token_route() {
    local symbol="$1" base settings name decimals supply nonce attempt token dusk_route out report log
    local code taken tx recorded
    base=".token_routes[\"$symbol\"]"
    if [ "$(state_get "$base.dusk.read_back")" = true ]; then
        ok "Dusk $symbol route already deployed ($(state_get "$base.dusk.route"))"
        return
    fi
    settings="$(state_json | jq -c --arg s "$symbol" '.token_routes[$s] // empty | {symbol, name, decimals, supply}')"
    [ -n "$settings" ] || settings="$(jq -c --arg s "$symbol" '.[] | select(.symbol == $s)' <<<"$TOKEN_ROUTES_JSON")"
    name="$(jq -r .name <<<"$settings")" decimals="$(jq -r .decimals <<<"$settings")" supply="$(jq -r .supply <<<"$settings")"
    log="$OUT_DIR/dusk-route-$symbol.txs"
    out="$OUT_DIR/dusk-route-$symbol.json"
    for attempt in $(seq 16); do
        [ -n "$(state_get "$base.dusk.deploy_nonce")" ] \
            || plan_dusk_token_route "$symbol" "$name" "$decimals" "$supply"
        record_token_route_txs "$symbol" "$log"
        nonce="$(state_get "$base.dusk.deploy_nonce")"
        token="$(state_get "$base.dusk.token")" dusk_route="$(state_get "$base.dusk.route")"
        recorded=()
        tx="$(state_get "$base.dusk.token_tx")"
        [ -z "$tx" ] || recorded+=(--token-tx "$tx" --token-tx-nonce "$(state_get "$base.dusk.token_tx_nonce")")
        tx="$(state_get "$base.dusk.route_tx")"
        [ -z "$tx" ] || recorded+=(--route-tx "$tx" --route-tx-nonce "$(state_get "$base.dusk.route_tx_nonce")")
        info "Deploying the $symbol token and its route on Dusk, at deploy nonces $nonce and $((nonce + 1)) (the token $token, the route $dusk_route)..."
        code=0
        dusk_tx deploy-token-route --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
            --mailbox "$(state_get '.dusk.mailbox')" --token-name "$name" --token-symbol "$symbol" \
            --token-decimals "$decimals" --token-supply "$supply" --deploy-nonce "$nonce" \
            --remote-domain "$EVM_DOMAIN" --expected-token "$token" --expected-route "$dusk_route" \
            --tx-log "$log" "${recorded[@]}" --wait-finality \
            --wasm-dir "$WASM_DIR" --gas-limit "$DUSK_GAS_LIMIT" --gas-price "$DUSK_GAS_PRICE" \
            --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID" > "$out" || code=$?
        record_token_route_txs "$symbol" "$log"
        taken="$(jq -r '.taken // empty' "$out" 2>/dev/null)" || taken=""
        if [ -n "$taken" ]; then
            warn "$(jq -r .error "$out") Leaving deploy nonces $nonce and $((nonce + 1)) for the $symbol route; planning the next two."
            state_set --argjson n "$nonce" "$base.dusk = {abandoned: (($base.dusk.abandoned // []) + [\$n])}"
            continue
        fi
        [ "$code" = 0 ] \
            || fail "The $symbol route's deployment on Dusk failed: $(jq -r '.error // empty' "$out" 2>/dev/null). Run deploy again: it resumes at the same contract IDs."
        report="$(<"$out")"
        jq -e --arg t "$token" --arg r "$dusk_route" '.success == true and .contracts.token == $t and .contracts.route == $r' \
            <<<"$report" >/dev/null || fail "deploy-token-route reported other contracts than the $symbol route's planned ones in $out"
        check_dusk_token_route "token:$symbol" "$(state_get '.dusk_owner')" \
            "$(recorded_dusk_owner_key "$(state_get '.dusk_owner')")" fresh
        state_set "$base.dusk.read_back = true"
        ok "Dusk $symbol token $token and its route $dusk_route: both read back as deployed, owned by DUSK_KEYS"
        return
    done
    fail "16 pairs of deploy nonces planned for the $symbol route were taken by someone else's contracts. Nothing of theirs is used: find out who is deploying at this deployer's IDs before running deploy again."
}

# Plan a token route's deploy nonces and contract IDs, the first pair past
# every nonce $STATE's token routes use or left whose IDs no contract has
# yet, and record them with the route's settings.
plan_dusk_token_route() {
    local symbol="$1" name="$2" decimals="$3" supply="$4" base nonce plan tries token dusk_route
    base=".token_routes[\"$symbol\"]"
    nonce="$(next_token_route_nonce)"
    for tries in $(seq 16); do
        plan="$(dusk_tx deploy-token-route --plan --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
            --mailbox "$(state_get '.dusk.mailbox')" --token-name "$name" --token-symbol "$symbol" \
            --token-decimals "$decimals" --token-supply "$supply" --deploy-nonce "$nonce" \
            --wasm-dir "$WASM_DIR")" && jq -e '.success == true' <<<"$plan" >/dev/null \
            || fail "Can't plan the $symbol route's contracts on Dusk: $(jq -r '.error // empty' <<<"$plan" 2>/dev/null)"
        jq -e '.exists.token or .exists.route' <<<"$plan" >/dev/null || break
        info "Deploy nonces $nonce and $((nonce + 1)) give contract IDs someone has taken; trying the next two"
        nonce=$((nonce + 2))
        [ "$tries" -lt 16 ] || fail "The first 16 pairs of deploy nonces from $(next_token_route_nonce) give the $symbol route contract IDs someone has taken"
    done
    token="$(jq -r .contracts.token <<<"$plan")" dusk_route="$(jq -r .contracts.route <<<"$plan")"
    [[ "$token" =~ ^[0-9a-f]{64}$ ]] && [[ "$dusk_route" =~ ^[0-9a-f]{64}$ ]] \
        || fail "deploy-token-route planned no contract IDs for the $symbol route"
    state_set --arg n "$name" --arg s "$symbol" --argjson d "$decimals" --arg u "$supply" \
        --argjson nonce "$nonce" --arg t "$token" --arg r "$dusk_route" \
        ".token_routes = (.token_routes // {}) | ($base.dusk.abandoned // []) as \$left
         | $base = {name: \$n, symbol: \$s, decimals: \$d, supply: \$u,
             dusk: ({deploy_nonce: \$nonce, token: \$t, route: \$r} + if \$left == [] then {} else {abandoned: \$left} end)}"
}

# Record in $STATE the transactions deploy-token-route sent for a token
# route's planned contracts, from the tx log it writes each to before it
# sends it: the last for each, or none once deploy-token-route has marked
# that one void, as never sent.
record_token_route_txs() {
    local symbol="$1" log="$2" base txs
    base=".token_routes[\"$symbol\"]"
    [ -s "$log" ] || return 0
    txs="$(jq -Rsc --arg t "$(state_get "$base.dusk.token")" --arg r "$(state_get "$base.dusk.route")" '
        [splits("\n") | fromjson? | objects] as $sent
        | ($sent | map(select(.contract == "token" and .id == $t)) | last) as $token
        | ($sent | map(select(.contract == "route" and .id == $r)) | last) as $route
        | (if $token == null then {} elif $token.void then {token_tx: null, token_tx_nonce: null}
           else {token_tx: $token.tx, token_tx_nonce: $token.nonce} end)
          + (if $route == null then {} elif $route.void then {route_tx: null, route_tx_nonce: null}
             else {route_tx: $route.tx, route_tx_nonce: $route.nonce} end)' "$log")" \
        || fail "Can't read the tx log $log"
    [ "$txs" = "{}" ] || state_set --argjson x "$txs" "$base.dusk += \$x | $base.dusk |= with_entries(select(.value != null))"
}

# Read back a token route's contracts on Dusk, its canonical DRC20 and the
# collateral route that locks it, against $STATE and the settings, as
# check_dusk_contracts reads back the rest. Someone can deploy at the
# planned IDs first, or front-run an init, and choose their owner and
# bindings (docs/testnet.md, Before mainnet). Always: the route's owner is
# `owner`, it is bound to the recorded Mailbox and locks the recorded token,
# has no hook of its own and no router for Dusk; the token has the route's
# name, symbol and decimals, and `owner_key`, `owner`'s public key, holds
# its admin role. `fresh`, right after deploy-token-route, also reads what
# only the owner can change later: the route has no ISM of its own and no
# router for the EVM chain yet, and the token's whole supply is the route's
# initial supply, held by the owner, who also holds the minter and pauser
# roles, with no cap and not paused. `recorded` holds the route's router for
# the EVM chain to none or the recorded EVM token instead, and leaves its
# ISM to check_deployed_ism.
check_dusk_token_route() {
    local route="$1" owner="$2" owner_key="$3" mode="$4" symbol base dusk_route token label hint
    local info field value expected what
    symbol="$(route_symbol "$route")" base=".token_routes[\"$(route_symbol "$route")\"]"
    dusk_route="$(state_get "$base.dusk.route")" token="$(state_get "$base.dusk.token")"
    label="$(route_name "$route")"
    if [ "$mode" = fresh ]; then
        hint="They don't hold what deploy-token-route sent, though this deployment's own transactions created them, or the node doesn't answer as they do. The route isn't recorded as deployed. Don't use them: take .token_routes.$symbol out of $STATE and run deploy again, which plans the route at other deploy nonces."
    else
        hint="$STATE records a token route that isn't as deploy left it: it is another deployment's, or its owner changed it, which deploy/testnet.sh never does. Nothing was sent. Find out which."
    fi
    [[ "$dusk_route" =~ ^[0-9a-f]{64}$ ]] && [[ "$token" =~ ^[0-9a-f]{64}$ ]] \
        || fail "$STATE names no Dusk contracts for the $symbol route"
    [ "$mode" = recorded ] || dusk_expect "$label" "$dusk_route" state_version u32 \
        "$(sed -n 's/.*warp_drc20_collateral=\([0-9]*\).*/\1/p' <<<"$ROUTE_STATE_VERSIONS")" "this checkout's" "$hint"
    value="$(dusk_owner_of "$label" "$dusk_route")" || exit 1
    [ "$value" = "$owner" ] || fail "The Dusk $label $dusk_route's owner is $value, not the deployer's account $owner. $hint"
    dusk_expect "$label" "$dusk_route" mailbox bytes32 "$(state_get '.dusk.mailbox')" "the Mailbox" "$hint"
    dusk_expect "$label" "$dusk_route" wrapped_token bytes32 "$token" "the $symbol token recorded in $STATE" "$hint"
    dusk_expect "$label" "$dusk_route" hook bytes32 "$ZERO_ID" "none: the Mailbox's default hook, the IGP" "$hint"
    dusk_expect "$label" "$dusk_route" enrolled_router bytes32 "$ZERO_ID" "none" "$hint" --arg-u32 "$DUSK_DOMAIN"
    value="$(dusk_value "the router of the Dusk $label $dusk_route for $EVM_CHAIN_NAME" --contract "$dusk_route" \
        --method enrolled_router --return-type bytes32 --arg-u32 "$EVM_DOMAIN")" || exit 1
    expected="$(state_get "$base.evm.token")"
    if [ "$mode" = fresh ]; then
        expected="none: deploy-token-route enrolls none"
    elif [ -n "$expected" ]; then
        [ "$value" != "$(pad32 "$expected")" ] || value="$ZERO_ID"
        expected="none or the EVM $symbol token $expected recorded in $STATE"
    else
        expected="none: $STATE records no EVM $symbol token"
    fi
    [ "$value" = "$ZERO_ID" ] \
        || fail "The Dusk $label $dusk_route is enrolled with the router $value for $EVM_CHAIN_NAME (domain $EVM_DOMAIN), not $expected. $hint"
    [ "$mode" = recorded ] || dusk_expect "$label" "$dusk_route" interchain_security_module bytes32 "$ZERO_ID" \
        "none: the Mailbox's default ISM" "$hint"

    info="$("$DUSK_TX" drc20-info --rues-url "$DUSK_RUES_URL" --token "$token" --account "$owner_key" 2>/dev/null)" \
        && jq -e '.success == true' <<<"$info" >/dev/null \
        || fail "Can't read the Dusk $symbol token $token: $(jq -r '.error // "the query failed"' <<<"$info" 2>/dev/null)${DUSK_READ_HINT:+. $DUSK_READ_HINT}"
    for field in name symbol decimals account.admin $( [ "$mode" = fresh ] && echo total_supply account.balance account.minter account.pauser cap paused ); do
        value="$(jq -r ".$field | tostring" <<<"$info")"
        case "$field" in
            name|symbol|decimals) expected="$(state_get "$base.$field")" what="$field" ;;
            total_supply) expected="$(state_get "$base.supply")" what="supply" ;;
            account.balance) expected="$(state_get "$base.supply")" what="balance of the deployer" ;;
            account.*) expected=true what="${field#account.} role held by the deployer" ;;
            cap) expected=0 what="cap" ;;
            paused) expected=false what="paused" ;;
        esac
        [ "$value" = "$expected" ] \
            || fail "The Dusk $symbol token $token's $what is $value, not $expected. $hint"
    done
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
# The hook type a hook under the EVM Mailbox's required hook reports. A hook
# deployed before hookType() existed has none; it counts as a ProtocolFee only
# when EVM_TYPELESS_HOOKS lists it as one and it answers protocolFee(), and
# stops the script otherwise. Prints the type.
required_hook_type() {
    local hook="$1" entry address kind
    if evm_number "$hook" "hookType()(uint8)" 2>/dev/null; then
        return 0
    fi
    for entry in ${EVM_TYPELESS_HOOKS//,/ }; do
        address="${entry%%=*}"
        kind="${entry#*=}"
        [[ "$entry" == *=* && "$address" =~ ^0x[0-9a-fA-F]{40}$ ]] \
            || fail "EVM_TYPELESS_HOOKS entry '$entry' isn't address=kind"
        [ "$kind" = protocol_fee ] \
            || fail "EVM_TYPELESS_HOOKS lists $address as $kind; the only kind it takes is protocol_fee"
        [ "$(lower "$address")" = "$(lower "$hook")" ] || continue
        evm_number "$hook" "protocolFee()(uint256)" > /dev/null \
            || fail "$hook, in the EVM Mailbox's required hook, has no hookType() and doesn't answer protocolFee(), though EVM_TYPELESS_HOOKS lists it as a ProtocolFee"
        printf '%s' "$HOOK_PROTOCOL_FEE"
        return 0
    done
    fail "Can't read the hook type of $hook, in the EVM Mailbox's required hook. If it is a ProtocolFee deployed before hookType(), list it in EVM_TYPELESS_HOOKS"
}

merkle_insertions() {
    local hook="$1" depth="${2:-0}" type children child route count=0 n
    [ "$depth" -lt "$MAX_HOOK_DEPTH" ] \
        || fail "The EVM Mailbox's required hook nests hooks more than $MAX_HOOK_DEPTH deep at $hook: deploy/testnet.sh can't tell how often it inserts a message to Dusk into EVM_MERKLE_TREE_HOOK"
    [ "$(lower "$hook")" != "$ZERO_ADDRESS" ] \
        || fail "The EVM Mailbox's required hook reaches the zero address: every dispatch would revert"
    type="$(required_hook_type "$hook")" || exit 1
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
    local deployer balance n account need pending wasm bytes=0
    pending="$(pending_token_routes | grep -c . || true)"
    if [ "$pending" -gt 0 ]; then
        for wasm in canonical_drc20_roles_pausable.wasm hyperlane_dusk_warp_drc20_collateral.wasm; do
            [ -f "$WASM_DIR/$wasm" ] \
                || fail "$WASM_DIR/$wasm isn't built, which deploy needs for the token routes (run: make all canonical-drc20-fixture)"
            bytes=$((bytes + $(wc -c <"$WASM_DIR/$wasm")))
        done
    fi
    account="$(dusk_tx account --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS")" \
        || fail "Can't read the Dusk deployer account: $(jq -r '.error // empty' <<<"$account" 2>/dev/null)"
    # Deploying the contracts costs about 300 DUSK, and each deploy needs
    # gas limit x gas price in the balance up front. Afterwards only cheap
    # calls remain, but for the token routes deploy has yet to deploy: 100
    # gas a byte of their Wasms and up to 2M for each one's init, 95 DUSK a
    # route at 2,000 LUX (the local rehearsal's took 47.5M gas), and the
    # same gas limit up front.
    if [ -z "$(state_get '.dusk.mailbox')" ]; then
        need=$((400 * 1000000000))
    elif [ "$pending" -gt 0 ]; then
        need=$((DUSK_GAS_LIMIT * DUSK_GAS_PRICE))
    else
        need=1000000000
    fi
    need=$((need + pending * (bytes * 100 + 2 * 2000000) * DUSK_GAS_PRICE))
    balance="$(jq -r '.balance' <<<"$account")"
    [ "$balance" -ge "$need" ] \
        || fail "Dusk deployer $(jq -r '.address' <<<"$account") has $(jq -r '.balance_dusk' <<<"$account") DUSK, needs at least $((need / 1000000000))$([ "$pending" = 0 ] || echo " for the $pending token routes to deploy")"
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
# Dusk ISM checks (test_relayed_process_fits_the_relayer_gas_limit), 1.98M
# plus that for a token route's payout. Below that every delivery runs out
# of gas and pays the whole limit, which the relayer's simulation can't
# catch, so insist on 1.5 times the cost.
# check_deployed_ism makes sure the deployed ISM's threshold is
# EVM_ORIGIN_THRESHOLD; the larger of the two is checked all the same.
check_relayer_gas_limit() {
    local threshold="$EVM_ORIGIN_THRESHOLD" basis="EVM_ORIGIN_THRESHOLD" need base=1950000
    if [ -n "$DEPLOYED_ISM_THRESHOLD" ] && [ "$DEPLOYED_ISM_THRESHOLD" -gt "$threshold" ]; then
        threshold="$DEPLOYED_ISM_THRESHOLD" basis="the deployed ISM's threshold"
    fi
    # A token route's payout costs more (compute_fees).
    [ "$(token_route_count)" = 0 ] || base=1980000
    need=$(((base + 220000 * threshold) * 3 / 2))
    [ "$DUSK_RELAYER_GAS_LIMIT" -ge "$need" ] \
        || fail "DUSK_RELAYER_GAS_LIMIT is $DUSK_RELAYER_GAS_LIMIT; with $basis of $threshold it must be at least $need"
}

# The relayer's config carries neither pipelineDeliveries nor
# maxBatchSize, so every agent build delivers to Dusk one at a time
# (docs/operations.md, Deliveries to Dusk). An env file from before may
# still set DUSK_RELAYER_MAX_BATCH_SIZE, which nothing reads now.
check_agent_settings() {
    [ -z "${DUSK_RELAYER_MAX_BATCH_SIZE+x}" ] \
        || warn "DUSK_RELAYER_MAX_BATCH_SIZE is no longer read: the relayer's config delivers to Dusk one at a time, and leaves maxBatchSize and the experimental pipelineDeliveries out (docs/operations.md, Deliveries to Dusk). Remove it from $ENV_FILE."
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
    local route
    DEPLOYED_ISM_THRESHOLD=""
    [ -n "$(state_get '.dusk.warp_native')" ] && [ -n "$(state_get '.dusk.mailbox')" ] || return 0
    check_evm_origin_policy
    for route in $(recorded_routes); do
        check_route_ism "$route"
    done
}
# The ISM one route uses: its own, or the Mailbox's default.
check_route_ism() {
    local key="$1" label route mailbox ism which type policy validators repeated threshold expected
    label="Dusk $(route_name "$key")"
    route="$(state_get "$(route_path "$key")")"
    mailbox="$(state_get '.dusk.mailbox')"
    ism="$(dusk_value "the ISM of the $label $route recorded in $STATE (if the network was reset, move $OUT_DIR aside and deploy again)" \
        --contract "$route" --method interchain_security_module --return-type bytes32)" || exit 1
    which="its own ISM"
    if [ "$ism" = "$ZERO_ID" ]; then
        ism="$(dusk_value "the default ISM of the Dusk Mailbox $mailbox" \
            --contract "$mailbox" --method default_ism --return-type bytes32)" || exit 1
        which="the Mailbox's default ISM"
    fi
    [[ "$ism" =~ ^[0-9a-f]{64}$ ]] && [ "$ism" != "$ZERO_ID" ] \
        || fail "The $label uses $which, which is not set ('$ism')"
    which="$which, $ism,"
    type="$(dusk_value "the module type of the Dusk ISM $ism" \
        --contract "$ism" --method module_type --return-type u8)" || exit 1
    case "$type" in
        "$MESSAGE_ID_MULTISIG_ISM") ;;
        "$TEST_MOCK_ISM")
            allows_test_mock_ism \
                || fail "The $label uses $which a TestMock ISM that accepts every message. Point it at a MessageIdMultisig ISM."
            warn "The $label uses $which a TestMock ISM that accepts every message. Allowed for this local rehearsal only."
            return 0 ;;
        *) fail "The $label uses $which of module type $type; it must be a MessageIdMultisig ISM ($MESSAGE_ID_MULTISIG_ISM)" ;;
    esac
    policy="$(dusk_value "the validators and threshold of the Dusk ISM $ism" \
        --contract "$ism" --method validators_and_threshold --return-type validators-and-threshold)" || exit 1
    validators="$(jq -er '.validators | join(",")' <<<"$policy")" \
        && threshold="$(jq -er '.threshold' <<<"$policy")" && [[ "$threshold" =~ ^[0-9]+$ ]] \
        || fail "The Dusk ISM $ism returned a malformed validator set"
    repeated="$(repeated_addresses "$validators")"
    [ -z "$repeated" ] \
        || fail "The $label uses $which which lists [$repeated] more than once among its validators, so fewer than its threshold of validators could sign for a message. This checkout's MessageIdMultisig ISM lists each once; point the route at one."
    validators="$(sorted_addresses "$validators")"
    expected="$(sorted_addresses "$EVM_ORIGIN_VALIDATORS")"
    [ "$validators" = "$expected" ] \
        || fail "The $label uses $which which trusts [$validators], but EVM_ORIGIN_VALIDATORS is [$expected]. Set EVM_ORIGIN_VALIDATORS to what the chain has, or change the ISM."
    [ "$threshold" -eq "$EVM_ORIGIN_THRESHOLD" ] \
        || fail "The $label uses $which which needs $threshold signatures, but EVM_ORIGIN_THRESHOLD is $EVM_ORIGIN_THRESHOLD. Set EVM_ORIGIN_THRESHOLD, and a DUSK_RELAYER_GAS_LIMIT that covers it, to what the chain has, or change the ISM."
    if [ -z "$DEPLOYED_ISM_THRESHOLD" ] || [ "$threshold" -gt "$DEPLOYED_ISM_THRESHOLD" ]; then
        DEPLOYED_ISM_THRESHOLD="$threshold"
    fi
    ok "The $label uses $which a MessageIdMultisig ISM: $threshold of [$validators]"
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
    evm_settle_call "$oracle" "setRemoteGasDataConfigs((uint32,uint128,uint128)[])"
    evm_settle_call "$igp" "setDestinationGasConfigs((uint32,(address,uint96))[])"
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
# deployment uses none, so the IGP must have no signer now, and never have
# had one (check_evm_igp_quote_history): a quote for a sender nobody asks
# about prices that sender's messages alone. The IGP resolves quotes by the
# caller, so also ask it what a transfer pays as the route, whose messages
# it prices, and as any other sender, against what the pricing `current`
# (evm_igp_pricing) comes to.
check_evm_igp_quotes() {
    local current="$1" igp signers gas expected sender quote
    igp="$(state_get '.evm.igp')"
    signers="$(evm_call "$igp" "quoteSigners()(address[])")" || fail "Can't read the EVM IGP $igp's quote signers"
    [ -z "$(tr -d '[] ' <<<"$signers")" ] \
        || fail "The EVM IGP $igp has quote signers $signers: offchain quotes they sign override its StorageGasOracle's pricing, which the relayer's requirements follow, and their standing quotes outlive them. This deployment uses no signed quotes: deploy a new IGP (docs/operations.md, An IGP that had a quote signer)."
    check_evm_igp_quote_history
    gas="$(fee .evm_igp.gas_overhead)"
    expected="$(igp_cost "$gas" "$(jq -r .gas_price <<<"$current")" "$(jq -r .token_exchange_rate <<<"$current")")"
    for sender in $(recorded_evm_tokens) "$ZERO_ADDRESS"; do
        quote="$(evm_number "$igp" "quoteGasPayment(uint32,uint256)(uint256)" "$DUSK_DOMAIN" "$gas" --from "$sender")" \
            || fail "Can't read the EVM IGP $igp's quote for a message from $sender"
        [ "$quote" = "$expected" ] \
            || fail "The EVM IGP $igp quotes $quote wei for $gas Dusk gas to a message from $sender, not the $expected its oracle's pricing comes to: a signed offchain quote overrides the oracle (docs/security.md, HIGH-8)."
    done
}
# Refuse an IGP that ever had a quote signer: read its QuoteSignerAdded
# events from the block that created it (a new IGP has none) to the head,
# EVM_LOG_BLOCK_RANGE blocks a request. The blocks read without one, up to
# EVM_REORG_PERIOD below the head, are recorded in $STATE
# (.evm.igp_quote_signers_scanned), and the next read starts after them.
check_evm_igp_quote_history() {
    local igp tx created scanned from head to logs found settled
    igp="$(lower "$(state_get '.evm.igp')")" tx="$(state_get '.evm.igp_creation_tx')"
    [[ "$EVM_LOG_BLOCK_RANGE" =~ ^[1-9][0-9]*$ ]] \
        || fail "EVM_LOG_BLOCK_RANGE must be a whole number of blocks, not '$EVM_LOG_BLOCK_RANGE'"
    [[ "$EVM_REORG_PERIOD" =~ ^[0-9]+$ ]] || fail "EVM_REORG_PERIOD must be a whole number of blocks, not '$EVM_REORG_PERIOD'"
    created="$(evm_block_number "$(evm_receipt "$tx" blockNumber)")" \
        || fail "Can't read the block of $tx, which created the EVM IGP $igp"
    scanned="$(state_json | jq -r --arg igp "$igp" '.evm.igp_quote_signers_scanned // {} | select(.igp == $igp) | .to // empty')"
    from="$created"
    if [[ "$scanned" =~ ^[0-9]+$ ]] && [ "$scanned" -ge "$created" ]; then from=$((scanned + 1)); else scanned=""; fi
    head="$(cast block-number --rpc-url "$EVM_RPC")" && [[ "$head" =~ ^[0-9]+$ ]] \
        || fail "Can't read the head of $EVM_CHAIN_NAME"
    while [ "$from" -le "$head" ]; do
        to=$((from + EVM_LOG_BLOCK_RANGE - 1))
        [ "$to" -le "$head" ] || to="$head"
        logs="$(cast logs --from-block "$from" --to-block "$to" --address "$igp" "$QUOTE_SIGNER_ADDED_TOPIC" \
            --json --rpc-url "$EVM_RPC")" \
            || fail "Can't read the EVM IGP $igp's events in blocks $from to $to; if the RPC limits the blocks a query spans, lower EVM_LOG_BLOCK_RANGE ($EVM_LOG_BLOCK_RANGE)"
        found="$(jq -r '[.[] | "\(.transactionHash) (block \(.blockNumber))"] | join(", ")' <<<"$logs")" \
            || fail "$EVM_RPC returned malformed logs for the EVM IGP $igp"
        [ -z "$found" ] \
            || fail "The EVM IGP $igp has had a quote signer: QuoteSignerAdded in $found. Standing quotes a signer stored outlive its removal and come before the oracle, for the senders they name, and this deployment uses no signed quotes. Nothing was sent: deploy a new IGP (docs/operations.md, An IGP that had a quote signer)."
        from=$((to + 1))
    done
    settled=$((head - EVM_REORG_PERIOD))
    if [ "$settled" -ge "$created" ] && { [ -z "$scanned" ] || [ "$settled" -gt "$scanned" ]; }; then
        state_set --arg igp "$igp" --argjson to "$settled" '.evm.igp_quote_signers_scanned = {igp: $igp, to: $to}'
    fi
}
# A block number as cast prints it, in decimal or hex, in decimal.
evm_block_number() {
    local n="$1"
    [[ "$n" =~ ^0x[0-9a-fA-F]+$ ]] && n=$((16#${n#0x}))
    [[ "$n" =~ ^[0-9]+$ ]] && printf '%s' "$n"
}
# What gas costs at a gas price and token exchange rate, as the IGPs compute
# it. Bash's integers are too small.
igp_cost() { python3 -c 'import sys; g, p, r = map(int, sys.argv[1:]); print(g * p * r // 10**10)' "$@"; }

# Every route's EVM token must dispatch through the recorded hook, and that
# hook must insert into the Merkle tree validators sign, unless the required
# hook does, and pay our IGP. Before `deploy`, a token that doesn't yet only
# warns: deploy sets its hook.
check_evm_route_hook() {
    local when="${1:-}" route
    for route in $(recorded_routes); do
        check_evm_route_hook_of "$when" "$route"
    done
}
check_evm_route_hook_of() {
    local when="$1" route="$2" label token hook igp current children expected
    token="$(state_get "$(route_evm_base "$route").token")" hook="$(state_get '.evm.route_hook')" igp="$(state_get '.evm.igp')"
    [ -n "$token" ] || return 0
    if [ "$route" = native ]; then label="EVM route"; else label="EVM $(route_name "$route")"; fi
    if [ -z "$hook" ] || [ -z "$(state_get '.evm.storage_gas_oracle')" ]; then
        [ "$when" = before-deploy ] || fail "The $label $token has no hook that pays an IGP of this deployment: run deploy"
        warn "The $label $token has no hook that pays an IGP of this deployment yet; deploy sets one"
        return 0
    fi
    current="$(evm_call "$token" "hook()(address)")" || fail "Can't read the hook of the $label $token"
    if [ "$(lower "$current")" != "$(lower "$hook")" ]; then
        [ "$when" = before-deploy ] || fail "The $label $token dispatches through hook $current, not $hook recorded in $STATE: run deploy"
        warn "The $label $token dispatches through hook $current; deploy sets $hook"
        return 0
    fi
    if [ "$(lower "$hook")" = "$(lower "$igp")" ]; then
        [ "$ROUTE_HOOK_NEEDS_MERKLE" = 0 ] \
            || fail "The $label's hook is the IGP alone, so its messages never reach the Merkle tree validators sign"
        ok "The $label dispatches through the IGP $igp; the Mailbox's required hook inserts into $EVM_MERKLE_TREE_HOOK"
        return 0
    fi
    [ "$ROUTE_HOOK_NEEDS_MERKLE" = 1 ] \
        || fail "The EVM Mailbox's required hook now inserts into $EVM_MERKLE_TREE_HOOK, and so does the route's hook $hook: every message would be inserted twice. Point the route's hook at the IGP $igp."
    children="$(evm_call "$hook" "hooks(bytes)(address[])" 0x)" || fail "Can't read the hooks of $hook"
    children="$(tr -d '[] ' <<<"$children" | tr '[:upper:]' '[:lower:]')"
    expected="$(lower "$EVM_MERKLE_TREE_HOOK,$igp")"
    [ "$children" = "$expected" ] \
        || fail "The $label's hook $hook aggregates [$children], not [$expected] (the Merkle hook validators sign, then the IGP)"
    ok "The $label dispatches through $hook: Merkle tree $EVM_MERKLE_TREE_HOOK, then IGP $igp"
}

cmd_preflight() {
    for tool in jq cast forge curl python3; do
        command -v "$tool" >/dev/null || fail "$tool is required"
    done
    load_fees
    check_agent_settings
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

# Deploy the Dusk contracts, read them back, and only then record them.
# dusk-tx deploy-hyperlane reads every contract back before it reports it
# deployed (dusk-tx/src/deployment.rs); this reads them again against the
# settings (check_dusk_contracts), and records the owner it read with them.
# When deploy-hyperlane finished but this script stopped before recording
# its contracts, deploying again would find their IDs taken: the contracts
# deploy-hyperlane reported are read back instead. Its report is read once:
# the contracts read back are the ones recorded.
deploy_dusk() {
    local out="$OUT_DIR/dusk-deploy.json" owner report="" contracts
    if [ -n "$(state_get '.dusk.mailbox')" ]; then
        ok "Dusk contracts already deployed (Mailbox $(state_get '.dusk.mailbox'))"
        return
    fi
    owner="$(deployer_account)" || exit 1
    mkdir -p "$OUT_DIR"
    [ ! -f "$out" ] || report="$(<"$out")"
    if jq -e '.success == true and (.contracts | type) == "object"' <<<"$report" >/dev/null 2>&1; then
        info "Reading back the Dusk contracts in $out, which deploy-hyperlane deployed before this script stopped..."
    else
        info "Deploying the Dusk contracts (this takes a few minutes)..."
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
        report="$(<"$out")"
        jq -e '.success == true and (.contracts | type) == "object"' <<<"$report" >/dev/null 2>&1 \
            || fail "deploy-hyperlane reported no contracts in $out"
    fi
    jq -e --argjson chain "$DUSK_EXPECTED_CHAIN_ID" --argjson domain "$DUSK_DOMAIN" \
        '.chain_id == $chain and .domain == $domain' <<<"$report" >/dev/null \
        || fail "$out records a Dusk deployment on chain $(jq -r .chain_id <<<"$report") at domain $(jq -r .domain <<<"$report"), not DUSK_EXPECTED_CHAIN_ID $DUSK_EXPECTED_CHAIN_ID at DUSK_DOMAIN $DUSK_DOMAIN. Move $OUT_DIR aside."
    contracts="$(jq -c '.contracts' <<<"$report")"
    check_dusk_contracts "$contracts" "$owner" fresh
    state_set --argjson d "$contracts" --arg domain "$DUSK_DOMAIN" --arg owner "$owner" \
        '.dusk = $d | .dusk_domain = ($domain | tonumber) | .dusk_owner = $owner'
    dusk_read_back keys "$contracts" "$owner"
    ok "Dusk Mailbox $(state_get '.dusk.mailbox'), WarpNative $(state_get '.dusk.warp_native'): every contract reads back as deployed, owned by DUSK_KEYS"
}

# Who can claim the fees on Dusk: DUSK_FEE_BENEFICIARY, or the deployer that
# deploy-hyperlane makes the beneficiary.
configure_dusk_beneficiary() {
    [ -n "${DUSK_FEE_BENEFICIARY:-}" ] || return 0
    local name contract out wanted current
    out="$("$DUSK_TX" account --offline --address "$DUSK_FEE_BENEFICIARY" 2>/dev/null)" \
        && wanted="$(jq -er '.account_h256' <<<"$out" 2>/dev/null)" \
        || fail "DUSK_FEE_BENEFICIARY ($DUSK_FEE_BENEFICIARY) isn't a Moonlight address: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
    for name in igp protocol_fee; do
        contract="$(state_get ".dusk.$name")"
        # A beneficiary already set stays as it is: nothing is sent.
        current="$(dusk_value "the beneficiary of the Dusk $name $contract" --contract "$contract" \
            --method beneficiary --return-type bytes32)" || exit 1
        [ "$current" != "$wanted" ] || continue
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
        # A creation of the ISM an earlier run sent directly, without the
        # factory, can still execute: recording the factory's ISM would
        # forget it. It is kept, and stops deploy until it is settled.
        [ -z "$(state_json | jq -r --arg s "$(evm_creation_step .evm.ism)" '.evm_sending[$s] // empty')" ] \
            || fail "An earlier run sent a creation of the EVM security module without EVM_ISM_FACTORY, and it isn't settled (\"create .evm.ism\" in .evm_sending). Nothing was sent. Settle it first: run deploy with EVM_ISM_FACTORY empty, which takes that ISM if the creation executed; or, once the deployer's transaction with its nonce is final and isn't that creation, remove the entry (docs/testnet.md)."
        ism="$(evm_call "$EVM_ISM_FACTORY" "getAddress(address[],uint8)(address)" "[$validators]" "$DUSK_ORIGIN_THRESHOLD")"
        evm_settle_call "$EVM_ISM_FACTORY" "deploy(address[],uint8)"
        if ! evm_has_code "$ism"; then
            info "Deploying MessageIdMultisigIsm ($DUSK_ORIGIN_THRESHOLD of [$validators]) through the factory..."
            evm_send "$EVM_ISM_FACTORY" "deploy(address[],uint8)" "[$validators]" "$DUSK_ORIGIN_THRESHOLD"
        fi
    else
        info "Deploying StorageMessageIdMultisigIsm ($DUSK_ORIGIN_THRESHOLD of [$validators])..."
        forge_create ism .evm.ism contracts/isms/multisig/StorageMultisigIsm.sol:StorageMessageIdMultisigIsm \
            --constructor-args "[$validators]" "$DUSK_ORIGIN_THRESHOLD" || fail "EVM security module deployment failed"
    fi
    evm_has_code "$ism" || fail "EVM security module deployment failed"
    evm_record "$(evm_creation_step .evm.ism)" --arg ism "$ism" --arg v "$validators" \
        --argjson t "$DUSK_ORIGIN_THRESHOLD" \
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
        forge_create oracle .evm.storage_gas_oracle contracts/hooks/igp/StorageGasOracle.sol:StorageGasOracle \
            || fail "StorageGasOracle deployment failed"
        evm_record "$(evm_creation_step .evm.storage_gas_oracle)" --arg o "$oracle" '.evm.storage_gas_oracle = $o'
    fi
    [ "$(lower "$(evm_call "$oracle" "owner()(address)")")" = "$(lower "$deployer")" ] \
        || fail "The StorageGasOracle $oracle isn't owned by the EVM deployer $deployer"
    if [ -z "$(state_get '.evm.igp')" ]; then
        record_index_from
        init="$(cast calldata "initialize(address,address)" "$deployer" "$beneficiary")" \
            || fail "Can't encode the EVM IGP's initialize call"
        deploy_evm_proxied .evm igp IGP contracts/hooks/igp/InterchainGasPaymaster.sol:InterchainGasPaymaster "$init"
    fi
    igp="$(state_get '.evm.igp')"
    check_evm_creation .evm igp IGP
    # Only its owner, the deployer, could have handed it to someone else.
    owner="$(evm_call "$igp" "owner()(address)")" || fail "Can't read the owner of the EVM IGP $igp"
    [ "$(lower "$owner")" = "$(lower "$deployer")" ] \
        || fail "The EVM IGP $igp is owned by $owner, not the EVM deployer $deployer"
    # EVM_IGP_BENEFICIARY, when set, wins over what the IGP has; when it isn't,
    # the beneficiary stays what it is, as on Dusk.
    evm_settle_call "$igp" "setBeneficiary(address)"
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
            forge_create factory .evm.aggregation_hook_factory \
                contracts/hooks/aggregation/StaticAggregationHookFactory.sol:StaticAggregationHookFactory \
                || fail "StaticAggregationHookFactory deployment failed"
            evm_record "$(evm_creation_step .evm.aggregation_hook_factory)" --arg f "$factory" \
                '.evm.aggregation_hook_factory = $f'
        fi
        hook="$(evm_call "$factory" "getAddress(address[])(address)" "[$EVM_MERKLE_TREE_HOOK,$igp]")" \
            || fail "Can't get the aggregation hook's address from the factory $factory"
        evm_settle_call "$factory" "deploy(address[])"
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
    deploy_evm_proxied .evm token "DUSK token" contracts/token/HypERC20.sol:HypERC20 "$init" \
        --constructor-args 9 1 1 "$EVM_MAILBOX"
    state_set --arg o "$deployer" '.evm.owner = $o'
    ok "EVM DUSK token $(state_get '.evm.token')"
}

# A token route's synthetic token on the EVM chain: a HypERC20 with the
# route's name, symbol and decimals, and no scaling, so that an amount is
# the same number of the token's smallest units on both chains. It is
# created behind an ERC1967Proxy that initializes it with the route's hook
# and the EVM ISM, as the DUSK token is, and with no supply: the route's
# custody on Dusk backs every token it mints.
deploy_evm_route_token() {
    local route="$1" base symbol name decimals deployer init
    base="$(route_evm_base "$route")" symbol="$(route_symbol "$route")"
    if [ -n "$(state_get "$base.token")" ]; then
        ok "EVM $symbol token already deployed ($(state_get "$base.token"))"
        return
    fi
    IFS='|' read -r name symbol decimals <<<"$(route_token_metadata "$route")"
    deployer="$(evm_deployer)"
    record_index_from
    init="$(cast calldata "initialize(uint256,string,string,address,address,address)" \
        0 "$name" "$symbol" "$(state_get '.evm.route_hook')" "$(state_get '.evm.ism')" "$deployer")" \
        || fail "Can't encode the EVM $symbol token's initialize call"
    info "Deploying the synthetic $symbol token on $EVM_CHAIN_NAME ($name, $decimals decimals, as on Dusk)..."
    deploy_evm_proxied "$base" token "$symbol token" contracts/token/HypERC20.sol:HypERC20 "$init" \
        --constructor-args "$decimals" 1 1 "$EVM_MAILBOX"
    check_evm_token_binding "$route"
    ok "EVM $symbol token $(state_get "$base.token")"
}

# A token deployed before the route had its own hook dispatches through the
# Mailbox's default hook, which pays nothing to our IGP. The token is
# checked again right before the write, as enroll_routers checks it.
set_evm_route_hook() {
    local route="$1" token hook
    token="$(state_get "$(route_evm_base "$route").token")" hook="$(state_get '.evm.route_hook')"
    evm_settle_call "$token" "setHook(address)"
    if [ "$(lower "$(evm_call "$token" "hook()(address)")")" != "$(lower "$hook")" ]; then
        check_evm_token_binding "$route"
        info "Setting the EVM $(route_symbol "$route") token's hook to $hook..."
        evm_send "$token" "setHook(address)" "$hook"
    fi
    check_evm_route_hook_of "" "$route"
}

# Enroll a route's Dusk route on its EVM token, and the token on the route.
# Preflight checked both routers, but their owners can enroll others in
# between from elsewhere: the deployment's lock keeps out only this
# script's commands. So each side is read again right before each write,
# and enrolled only where it has no router: another one is refused on the
# EVM side (check_evm_token_router) as on Dusk (dusk_route_router). One
# enrolled between that read and the write is replaced, since enrolling
# replaces whatever is there; an on-chain check waits for the mainnet
# contracts (docs/testnet.md, Before mainnet).
enroll_routers() {
    local route="$1" token dusk_route router symbol
    symbol="$(route_symbol "$route")"
    token="$(state_get "$(route_evm_base "$route").token")"
    dusk_route="$(lower "$(state_get "$(route_path "$route")")")"
    evm_settle_call "$token" "enrollRemoteRouter(uint32,bytes32)"
    check_evm_token_binding "$route"
    router="$(dusk_route_router "$route")" || exit 1
    if [ "$EVM_TOKEN_ROUTER" != "0x$dusk_route" ]; then
        [[ "$EVM_TOKEN_ROUTER" =~ ^0x0{64}$ ]] \
            || fail "The EVM $symbol token $token is enrolled with the Dusk route ${EVM_TOKEN_ROUTER#0x}: not replacing it"
        info "Enrolling the Dusk $(route_name "$route") on $EVM_CHAIN_NAME..."
        evm_send "$token" "enrollRemoteRouter(uint32,bytes32)" "$DUSK_DOMAIN" "0x$dusk_route"
    fi
    [ "$(lower "$(evm_call "$token" "routers(uint32)(bytes32)" "$DUSK_DOMAIN")")" = "0x$dusk_route" ] \
        || fail "The EVM $symbol token does not have its Dusk route enrolled"
    [ "$router" = token ] || router="$(dusk_route_router "$route")" || exit 1
    if [ "$router" = none ]; then
        info "Enrolling the $EVM_CHAIN_NAME $symbol token on the Dusk $(route_name "$route")..."
        dusk_tx enroll-router --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
            --warp-contract "$dusk_route" --domain "$EVM_DOMAIN" --router "$(pad32 "$token")" \
            --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID" >/dev/null \
            || fail "Dusk router enrollment failed"
    fi
    router="$(dusk_route_router "$route")" || exit 1
    [ "$router" = token ] || fail "The Dusk $(route_name "$route") does not have the EVM $symbol token enrolled"
    ok "The $symbol routes are enrolled with each other"
}

# A Dusk route's router for EVM_DOMAIN: `token` when it is the route's
# recorded EVM token, `none` when there is none yet. Any other router
# fails, as the EVM side's does (check_evm_token_router): the route's
# custody backs that router's tokens, and replacing it would leave their
# holders nothing to bring them back to.
dusk_route_router() {
    local route="$1" dusk_route token router
    dusk_route="$(lower "$(state_get "$(route_path "$route")")")"
    token="$(state_get "$(route_evm_base "$route").token")"
    router="$(dusk_value "the router of the Dusk $(route_name "$route") $dusk_route for $EVM_CHAIN_NAME" \
        --contract "$dusk_route" --method enrolled_router --return-type bytes32 --arg-u32 "$EVM_DOMAIN")" || return 1
    if [ -n "$token" ] && [ "$router" = "$(pad32 "$token")" ]; then
        echo token
    elif [ "$router" = "$ZERO_ID" ]; then
        echo none
    else
        echo "[FAIL] The Dusk $(route_name "$route") $dusk_route is enrolled with the router $router for $EVM_CHAIN_NAME (domain $EVM_DOMAIN), not the EVM $(route_symbol "$route") token $token recorded in $STATE: not replacing it. The route's custody backs that router's tokens, and replacing it would cut their holders off. If $STATE recorded another token before, put it back; if the route is another deployment's, move $OUT_DIR aside." >&2
        return 1
    fi
}

# A state file from another network, or from before a network reset, would
# make `deploy` skip steps whose contracts don't exist. One whose routes are
# older than this checkout's would reuse contracts with known bugs.
check_state_matches_chain() {
    local mailbox name contract domain route
    [ -n "$STATE_JSON" ] || return 0
    [ "$(lower "$(state_get '.evm.mailbox')")" = "" ] || [ "$(lower "$(state_get '.evm.mailbox')")" = "$(lower "$EVM_MAILBOX")" ] \
        || fail "$STATE was made for EVM Mailbox $(state_get '.evm.mailbox'), not $EVM_MAILBOX. Use another DEPLOY_NAME or move it aside."
    mailbox="$(state_get '.dusk.mailbox')"
    if [ -n "$mailbox" ]; then
        domain="$(dusk_query --contract "$mailbox" --method local_domain --return-type u32 2>/dev/null | jq -r '.value // empty')" || true
        [ "$domain" = "$DUSK_DOMAIN" ] \
            || fail "$STATE records Dusk Mailbox $mailbox, but it isn't on this chain with domain $DUSK_DOMAIN. The network was reset or the state is from elsewhere; move it aside and redeploy."
    fi
    check_state_versions
    for name in storage_gas_oracle route_hook aggregation_hook_factory; do
        contract="$(state_get ".evm.$name")"
        [ -z "$contract" ] || evm_has_code "$contract" \
            || fail "$STATE records EVM $name $contract, but there is no contract there; move the state aside and redeploy."
    done
    for route in $(recorded_routes); do
        contract="$(state_get "$(route_evm_base "$route").token")"
        [ -z "$contract" ] || evm_has_code "$contract" \
            || fail "$STATE records the EVM $(route_symbol "$route") token $contract, but there is no contract there; move the state aside and redeploy."
    done
    contract="$(state_get '.evm.igp')"
    if [ -n "$contract" ] && [ -n "$(state_get '.evm.storage_gas_oracle')" ]; then
        evm_has_code "$contract" \
            || fail "$STATE records EVM igp $contract, but there is no contract there; move the state aside and redeploy."
    fi
    check_evm_creations
    [ -z "$mailbox" ] || check_recorded_dusk_contracts keys
}

# Refuse a deployment whose core contracts or warp routes report another
# state_version than this checkout's (CORE_STATE_VERSIONS,
# ROUTE_STATE_VERSIONS), its token routes' collateral routes included.
token_route_state_versions() {
    local route
    for route in $(recorded_routes); do
        [ "$route" = native ] \
            || echo "$route=$(sed -n 's/.*warp_drc20_collateral=\([0-9]*\).*/\1/p' <<<"$ROUTE_STATE_VERSIONS")"
    done
}
check_state_versions() {
    local entry name expected contract version kind older route
    for entry in $CORE_STATE_VERSIONS $ROUTE_STATE_VERSIONS $(token_route_state_versions); do
        name="${entry%%=*}" expected="${entry#*=}"
        if [[ "$name" == token:* ]]; then
            contract="$(state_get "$(route_path "$name")")"
            route="$name" name="warp_drc20_collateral, the $(route_symbol "$name") route"
            entry="warp_drc20_collateral=$expected"
        else
            contract="$(state_get ".dusk.$name")"
        fi
        [ -n "$contract" ] || continue
        if [[ " $ROUTE_STATE_VERSIONS " == *" $entry "* ]]; then
            kind="route"
            older="Older routes have bugs this checkout fixes (docs/security.md, CRITICAL-1 and MEDIUM-13), register any key that asks (MEDIUM-10), and WarpDrc20s before 8 have no allowance cap or holder reserve (docs/storage.md)."
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
    check_no_unattributed_evm_send
    cmd_preflight
    state_set --arg n "$EVM_CHAIN_NAME" --argjson d "$EVM_DOMAIN" --arg m "$EVM_MAILBOX" \
        --arg h "$EVM_MERKLE_TREE_HOOK" --arg va "$EVM_VALIDATOR_ANNOUNCE" \
        '.evm_chain = $n | .evm_domain = $d | .evm.mailbox = $m | .evm.merkle_tree_hook = $h
         | .evm.validator_announce = $va'
    deploy_dusk
    # The fee contracts are this deployment's, called by its Mailbox, before
    # configure_dusk_beneficiary changes who can claim their fees: restored
    # state can name a detached one the same key owns.
    check_dusk_deployment keys
    configure_dusk_beneficiary
    check_dusk_igp_pricing
    local symbol route
    if [ -n "$(token_routes_to_deploy)" ]; then
        record_dusk_owner_key
        for symbol in $(token_routes_to_deploy); do
            deploy_dusk_token_route "$symbol"
        done
    fi
    deploy_evm_ism
    deploy_evm_igp
    deploy_evm_route_hook
    deploy_evm_token
    for route in $(recorded_routes); do
        [ "$route" = native ] || deploy_evm_route_token "$route"
    done
    for route in $(recorded_routes); do
        set_evm_route_hook "$route"
        enroll_routers "$route"
    done
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
    [ -z "$apply" ] || check_no_unattributed_evm_send
    load_fees
    igp="$(state_get '.evm.igp')" oracle="$(state_get '.evm.storage_gas_oracle')"
    [ -n "$igp" ] && [ -n "$oracle" ] || fail "No EVM IGP recorded in $STATE; run deploy first"
    check_evm_deployment
    # An update an earlier --apply sent is settled before the pricing is
    # read: one still pending could land after this run found the pricing
    # as the settings say, and leave it as it said.
    # Only the two pricing steps' own sends matter here: another step's
    # pending send (a claim, say) is settled by that step.
    local oracle_step igp_step
    oracle_step="$(evm_call_step "$oracle" "setRemoteGasDataConfigs((uint32,uint128,uint128)[])")"
    igp_step="$(evm_call_step "$igp" "setDestinationGasConfigs((uint32,(address,uint96))[])")"
    if [ -n "$apply" ] && [ -n "$(state_json | jq -r --arg a "$oracle_step" --arg b "$igp_step" \
        '(.evm_sending // {}) | with_entries(select(.key == $a or .key == $b)) | keys[0] // empty')" ]; then
        require EVM_DEPLOYER_KEY
        evm_settle_call "$oracle" "setRemoteGasDataConfigs((uint32,uint128,uint128)[])"
        evm_settle_call "$igp" "setDestinationGasConfigs((uint32,(address,uint96))[])"
    fi
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
    for token in $(recorded_evm_tokens); do
        route_quote="$(evm_number "$token" "quoteGasPayment(uint32)(uint256)" "$DUSK_DOMAIN")" \
            || fail "Can't read the quote of the EVM token $token"
        ok "A transfer of $(evm_call "$token" "symbol()(string)" --json | jq -r '.[0] // "?"') to Dusk now pays $route_quote wei (quoteGasPayment on its token, the required hook's fee included)"
    done
    info "Regenerate the agent configs (agent-configs) if the overhead changed: the relayer's requirement follows it."
}

# ── Revenue ──────────────────────────────────────────────────────────────────

# What the fee contracts hold for their beneficiaries.
cmd_revenue() {
    local name contract claimable beneficiary igp balance
    [ -n "$STATE_JSON" ] || fail "Run deploy first"
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
    [ -n "$STATE_JSON" ] || fail "Run deploy first"
    check_no_unattributed_evm_send
    igp="$(state_get '.evm.igp')"
    [ -z "$igp" ] || [ -z "$(state_get '.evm.storage_gas_oracle')" ] || check_evm_deployment
    check_dusk_deployment
    if [ -n "$igp" ] && [ -n "$(state_get '.evm.storage_gas_oracle')" ]; then
        # A claim() an earlier run sent is settled before the balance is read.
        if [ -n "$(state_json | jq -r --arg s "$(evm_call_step "$igp" "claim()")" '.evm_sending[$s] // empty')" ]; then
            require EVM_DEPLOYER_KEY
            evm_settle_call "$igp" "claim()"
        fi
        # Read at the block of the claim() settled above, or later.
        balance="$(evm_read cast balance "$igp")" || fail "Can't read the EVM IGP $igp's balance"
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

# The Dusk chain of an agent config. Both agents batch their contract reads
# through Multicall for Dusk when it answers (check_dusk_multicall). The
# relayer delivers to Dusk one at a time: the chain has no
# pipelineDeliveries or maxBatchSize (check_agent_settings).
# ── Validators ───────────────────────────────────────────────────────────────
#
# A validator announces where it keeps its signed checkpoints on its origin
# chain's ValidatorAnnounce, and the relayer reads them from there. One
# without a Dusk account can't announce itself on Dusk: `announce` sends the
# signed announcement it wrote to its storage, from DUSK_KEYS's account (the
# contract checks the validator's signature, not who sends it), and records
# the location in $STATE. `set-validators` moves the ISMs to the settings'
# validator sets once every validator in them is announced and signing, so a
# switch can't stall deliveries. docs/validators.md has the procedure.

DUSK_VALIDATOR_CHECKPOINT_LAG="${DUSK_VALIDATOR_CHECKPOINT_LAG:-10}"
EVM_VALIDATOR_CHECKPOINT_LAG="${EVM_VALIDATOR_CHECKPOINT_LAG:-20}"
VALIDATOR_PROBE_SECONDS="${VALIDATOR_PROBE_SECONDS:-600}"
RELAYER_CONFIG="${RELAYER_CONFIG:-$OUT_DIR/relayer.json}"

# announce <location | URL | file> [validator]: announce a Dusk-origin
# validator from the signed announcement at its storage location (or a URL or
# copy of it), and record its location. With [validator], the address its
# operator gave, the announcement must be that validator's. One already
# announced is only recorded.
cmd_announce() {
    local source="${1:-}" expected="${2:-}" va out validator location
    [ -n "$source" ] && [ "$#" -le 2 ] \
        || fail "usage: bash deploy/testnet.sh announce <the validator's storage location, or a URL or file of its announcement> [its address]"
    [ -z "$expected" ] || [[ "$expected" =~ ^0x[0-9a-fA-F]{40}$ ]] || fail "The validator's address must be 0x and 40 hex characters, not '$expected'"
    require DUSK_KEYS DUSK_CONSENSUS_PASSWORD
    [ -x "$DUSK_TX" ] || fail "dusk-tx not built at $DUSK_TX (run: make dusk-tx)"
    check_dusk_deployment
    va="$(state_get '.dusk.validator_announce')"
    [ -n "$va" ] || fail "No Dusk ValidatorAnnounce recorded in $STATE; run deploy first"
    out="$(dusk_tx announce --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" \
        --validator-announce "$va" --announcement "$source" ${expected:+--validator "$expected"} \
        ${CHECKPOINT_S3_ENDPOINT:+--s3-endpoint "$CHECKPOINT_S3_ENDPOINT"} \
        --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
        || fail "Can't announce the validator: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
    validator="$(jq -r '.validator // empty' <<<"$out")" location="$(jq -r '.location // empty' <<<"$out")"
    [[ "$validator" =~ ^0x[0-9a-f]{40}$ ]] && [ -n "$location" ] \
        || fail "dusk-tx announce reported no validator and location: $out"
    state_set --arg v "$validator" --arg l "$location" '.dusk_origin_locations[$v] = $l'
    if [ "$(jq -r '.already_announced' <<<"$out")" = true ]; then
        ok "Validator $validator had announced $location on Dusk already; recorded it in $STATE"
    else
        ok "Announced validator $validator at $location on Dusk ($(jq -r '.tx_id' <<<"$out"), height $(jq -r '.block_height' <<<"$out"))"
    fi
    case ",$(sorted_addresses "${DUSK_ORIGIN_VALIDATORS:-}")," in
        *",$validator,"*) ;;
        *) info "It isn't in DUSK_ORIGIN_VALIDATORS: add it there, then run set-validators" ;;
    esac
}

# validator_checkpoint <validator> <location> <origin domain> <Merkle tree
# hook>: the validator's latest checkpoint at <location> must be its own,
# for that hook. Prints its index and message ID. A file:// location is read
# only in a local rehearsal. Call as `x="$(validator_checkpoint ...)" ||
# exit 1`.
validator_checkpoint() {
    local validator="$1" location="$2" domain="$3" hook="$4" out index
    local local_storage=() dusk_hook=()
    # A Dusk-origin checkpoint's message ID and root are checked against the
    # Dusk Merkle tree hook.
    [ "$domain" != "$DUSK_DOMAIN" ] || dusk_hook=(--rues-url "$DUSK_RUES_URL")
    if [[ "$location" == file://* ]]; then
        [ "${LOCAL_REHEARSAL_CHECKPOINTS:-0}" = 1 ] && is_local_url "$DUSK_RUES_URL" && is_local_url "$EVM_RPC" \
            || fail "Validator $validator keeps its checkpoints at $location, on this host: only a local rehearsal's validators do (LOCAL_REHEARSAL_CHECKPOINTS=1, with both endpoints local)"
        local_storage=(--allow-local-storage)
    fi
    out="$("$DUSK_TX" validator-checkpoint --location "$location" --validator "$validator" \
        --origin-domain "$domain" --merkle-tree-hook "$hook" \
        ${CHECKPOINT_S3_ENDPOINT:+--s3-endpoint "$CHECKPOINT_S3_ENDPOINT"} "${local_storage[@]}" "${dusk_hook[@]}")" \
        || fail "Validator $validator isn't signing at $location: $(jq -r '.error // "dusk-tx validator-checkpoint failed"' <<<"$out" 2>/dev/null)"
    index="$(jq -r '.index // empty' <<<"$out")"
    [[ "$index" =~ ^[0-9]+$ ]] || fail "dusk-tx validator-checkpoint reported no index: $out"
    printf '%s %s %s\n' "$index" "$(jq -r '.message_id' <<<"$out")" "$(jq -r '.root' <<<"$out")"
}

# checkpoint_is_current <validator> <index> <count> <lag setting>: a
# validator's latest checkpoint, at <index>, is at most <lag setting>
# messages behind <count>, its Merkle tree hook's, read after the
# checkpoint: a hook's count only grows, so a checkpoint past it is of
# another tree. Counting messages proves little where none arrive: the
# probe at --apply (probe_dusk_origin_validators) shows the Dusk-origin
# ones are signing now.
checkpoint_is_current() {
    local validator="$1" index="$2" count="$3" lag_name="$4" lag
    [[ "$count" =~ ^[0-9]+$ ]] || fail "Can't read the count of validator $validator's Merkle tree hook"
    [ "$index" -lt "$count" ] \
        || fail "Validator $validator's latest checkpoint is for index $index, but the Merkle tree hook it signs for has $count messages: it signs another chain's"
    lag=$((count - 1 - index))
    [[ "${!lag_name}" =~ ^[0-9]+$ ]] || fail "$lag_name must be a number of messages"
    [ "$lag" -le "${!lag_name}" ] \
        || fail "Validator $validator's latest checkpoint, index $index, is $lag messages behind the $count in its Merkle tree hook, more than $lag_name (${!lag_name}). Check it is running, or wait for it to catch up."
}

# A Dusk-origin validator is announced on Dusk at the location $STATE
# records for it, and signs the Dusk Merkle tree hook's checkpoints there.
check_dusk_origin_validator() {
    local validator="$1" count location hashes hash checkpoint index mth
    location="$(state_json | jq -r --arg v "$validator" '.dusk_origin_locations[$v] // empty')"
    [ -n "$location" ] \
        || fail "$STATE records no storage location for the Dusk-origin validator $validator. Announce it, or record where it announced, first: bash deploy/testnet.sh announce <its storage location>"
    hashes="$(dusk_value "the locations validator $validator announced on Dusk" \
        --contract "$(state_get '.dusk.validator_announce')" --method get_announced_storage_location_hashes \
        --return-type bytes32-list --arg-eth-address "$validator")" || exit 1
    hash="$(cast keccak "$location")" || fail "Can't hash $location"
    jq -e --arg h "${hash#0x}" 'index($h) != null' <<<"$hashes" >/dev/null \
        || fail "Validator $validator hasn't announced $location on Dusk: bash deploy/testnet.sh announce $location"
    # The relayer reads a validator's newest announcement.
    jq -e --arg h "${hash#0x}" '.[-1] == $h' <<<"$hashes" >/dev/null \
        || fail "Validator $validator has announced another location on Dusk since $location, and the relayer reads the newest: record that one with bash deploy/testnet.sh announce <its location>"
    mth="$(state_get '.dusk.merkle_tree_hook')"
    checkpoint="$(validator_checkpoint "$validator" "$location" "$DUSK_DOMAIN" "$mth")" || exit 1
    read -r index _ <<<"$checkpoint"
    count="$(dusk_value "the Dusk Merkle tree hook's count" --contract "$mth" --method count --return-type u32)" || exit 1
    checkpoint_is_current "$validator" "$index" "$count" DUSK_VALIDATOR_CHECKPOINT_LAG
    ok "Validator $validator is announced on Dusk at $location, and has signed up to index $index of $count"
}

# An EVM-origin validator is announced on the EVM chain's ValidatorAnnounce
# and signs the EVM Merkle tree hook's checkpoints at the location it
# announced last, as the relayer reads it.
check_evm_origin_validator() {
    local validator="$1" count locations location checkpoint index root latest
    # cast prints the string[][] as JSON, [["<location>", ...]].
    locations="$(evm_call "$(state_get '.evm.validator_announce')" \
        "getAnnouncedStorageLocations(address[])(string[][])" "[$validator]")" \
        || fail "Can't read where validator $validator announced on $EVM_CHAIN_NAME"
    location="$(jq -r 'flatten | last // empty' <<<"$locations" 2>/dev/null)" \
        || fail "Can't read where validator $validator announced on $EVM_CHAIN_NAME: $locations"
    [ -n "$location" ] \
        || fail "Validator $validator hasn't announced a storage location on $EVM_CHAIN_NAME's ValidatorAnnounce $(state_get '.evm.validator_announce')"
    checkpoint="$(validator_checkpoint "$validator" "$location" "$EVM_DOMAIN" "$(state_get '.evm.merkle_tree_hook')")" || exit 1
    read -r index _ root <<<"$checkpoint"
    count="$(evm_number "$(state_get '.evm.merkle_tree_hook')" "count()(uint32)")" \
        || fail "Can't read the count of $EVM_CHAIN_NAME's Merkle tree hook"
    checkpoint_is_current "$validator" "$index" "$count" EVM_VALIDATOR_CHECKPOINT_LAG
    # The EVM hook keeps only its latest root: a checkpoint at its latest
    # index must have it.
    latest="$(evm_call "$(state_get '.evm.merkle_tree_hook')" "latestCheckpoint()(bytes32,uint32)")" \
        || fail "Can't read the latest checkpoint of $EVM_CHAIN_NAME's Merkle tree hook"
    if [ "$(sed -n 2p <<<"$latest" | awk '{print $1}')" = "$index" ] \
        && [ "$(lower "$(sed -n 1p <<<"$latest")")" != "0x$root" ]; then
        fail "Validator $validator signed the root 0x$root at index $index, but $EVM_CHAIN_NAME's Merkle tree hook has $(sed -n 1p <<<"$latest") there: it can't join a quorum"
    fi
    ok "Validator $validator is announced on $EVM_CHAIN_NAME at $location, and has signed up to index $index of $count"
}

# A validator set from the settings, sorted and checked: each address once,
# and a threshold from 1 to their number.
checked_validator_set() {
    local name="$1" threshold_name="$2" list repeated n
    require "$name" "$threshold_name"
    repeated="$(repeated_addresses "${!name}")"
    [ -z "$repeated" ] || fail "$name lists [$repeated] more than once"
    list="$(sorted_addresses "${!name}")"
    tr ',' '\n' <<<"$list" | grep -qvE '^0x[0-9a-f]{40}$' && fail "$name must be comma-separated EVM addresses"
    n="$(count_addresses "$list")"
    [[ "${!threshold_name}" =~ ^[1-9][0-9]*$ ]] && [ "${!threshold_name}" -le "$n" ] \
        || fail "$threshold_name must be between 1 and the $n validators in $name"
    printf '%s' "$list"
}

# The ISM each recorded route on Dusk uses, its own or the Mailbox's default,
# once each, as "<ism> <which>" lines.
dusk_route_isms() {
    local route ism mailbox
    mailbox="$(state_get '.dusk.mailbox')"
    for route in $(recorded_routes); do
        ism="$(dusk_value "the ISM of the Dusk $(route_name "$route")" \
            --contract "$(state_get "$(route_path "$route")")" --method interchain_security_module \
            --return-type bytes32)" || exit 1
        if [ "$ism" = "$ZERO_ID" ]; then
            ism="$(dusk_value "the default ISM of the Dusk Mailbox $mailbox" \
                --contract "$mailbox" --method default_ism --return-type bytes32)" || exit 1
        fi
        printf '%s\n' "$ism"
    done | sort -u
}

# tokens_final_at <ism>: wait until every recorded EVM token verifies with
# <ism> as of the chain's finalized block, at most EVM_FINALITY_SECONDS, so
# no reorg can undo an update the record then names, this run's or an
# earlier one's. A local Anvil finalizes only 64 blocks back and doesn't
# reorg: its latest block counts.
tokens_final_at() {
    local target="$1" block route token ism pending deadline=$((SECONDS + ${EVM_FINALITY_SECONDS:-1800}))
    while :; do
        if is_local_url "$EVM_RPC"; then
            block=latest
        else
            block="$(cast block-number finalized --rpc-url "$EVM_RPC" 2>/dev/null)" || block=""
        fi
        pending=""
        if [[ "$block" =~ ^[0-9]+$ ]] && [ -n "$EVM_WRITE_BLOCK" ] && [ "$block" -lt "$EVM_WRITE_BLOCK" ]; then
            pending="the block of this run's last update, $EVM_WRITE_BLOCK,"
        elif [ "$block" = latest ] || [[ "$block" =~ ^[0-9]+$ ]]; then
            for route in $(recorded_routes); do
                token="$(state_get "$(route_evm_base "$route").token")"
                ism="$(cast call "$token" "interchainSecurityModule()(address)" --block "$block" --rpc-url "$EVM_RPC" 2>/dev/null)" \
                    || { pending="$token"; break; }
                [ "$(lower "$ism")" = "$target" ] || { pending="$token"; break; }
            done
        else
            pending="the finalized block"
        fi
        [ -n "$pending" ] || return 0
        [ "$SECONDS" -lt "$deadline" ] \
            || fail "The EVM token $pending doesn't verify with $target as of $EVM_CHAIN_NAME's finalized block $block after ${EVM_FINALITY_SECONDS:-1800}s. Nothing is recorded yet: run set-validators --apply again."
        sleep 5
    done
}

# probe_dusk_origin_validators <validators>: a message dispatched now, and
# each of <validators> signing it within VALIDATOR_PROBE_SECONDS, shows they
# are signing now, not only that they signed the latest message, which may
# be old. The probe goes from the Dusk TestRecipient, which pays its
# dispatch fee from Mailbox credit funded first (17.06 DUSK at the default
# prices), to VALIDATOR_PROBE_RECIPIENT on the EVM chain, the zero address
# unless set: a message the relayer can't deliver, and retries with backoff
# without paying for it.
probe_dusk_origin_validators() {
    local set="$1" mailbox recipient out amount probe count validator location checkpoint index deadline
    require DUSK_KEYS DUSK_CONSENSUS_PASSWORD
    mailbox="$(state_get '.dusk.mailbox')" recipient="$(state_get '.dusk.test_recipient')"
    [ -n "$recipient" ] || fail "No Dusk TestRecipient recorded in $STATE to send the probe from"
    amount="$(dusk_value "the ProtocolFee's fee" \
        --contract "$(state_get '.dusk.protocol_fee')" --method protocol_fee --return-type u64)" || exit 1
    amount=$(($(fee .dusk_to_evm.quote_lux) + amount))
    out="$(dusk_tx fund-dispatch --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" --mailbox "$mailbox" \
        --payer "$recipient" --amount "$amount" --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
        || fail "Can't fund the probe's dispatch fee: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
    out="$(dusk_tx dispatch --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" --mailbox "$mailbox" \
        --test-recipient "$recipient" --destination "$EVM_DOMAIN" \
        --recipient "$(pad32 "${VALIDATOR_PROBE_RECIPIENT:-$ZERO_ADDRESS}")" --body 0x70726f6265 \
        --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
        || fail "Can't dispatch the probe: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
    count="$(dusk_value "the Dusk Merkle tree hook's count" \
        --contract "$(state_get '.dusk.merkle_tree_hook')" --method count --return-type u32)" || exit 1
    probe=$((count - 1))
    info "Dispatched a probe from Dusk ($(jq -r '.tx_id' <<<"$out")): it is at index $probe or before. Waiting for each validator to sign up to it..."
    deadline=$((SECONDS + VALIDATOR_PROBE_SECONDS))
    for validator in ${set//,/ }; do
        location="$(state_json | jq -r --arg v "$validator" '.dusk_origin_locations[$v]')"
        while :; do
            if checkpoint="$(validator_checkpoint "$validator" "$location" "$DUSK_DOMAIN" "$(state_get '.dusk.merkle_tree_hook')" 2>/dev/null)"; then
                read -r index _ <<<"$checkpoint"
                [ "$index" -lt "$probe" ] || break
            fi
            [ "$SECONDS" -lt "$deadline" ] \
                || fail "Validator $validator hasn't signed the probe at index $probe within VALIDATOR_PROBE_SECONDS ($VALIDATOR_PROBE_SECONDS): it isn't signing. Nothing was switched."
            sleep 10
        done
        ok "Validator $validator signed the probe at index $probe"
    done
}

# A policy as evm_ism_policy prints it, with its validators sorted and each
# once, as the settings' are compared.
sorted_policy() {
    local validators="${1#* of [}"
    printf '%s of [%s]' "${1%% of *}" "$(sorted_addresses "${validators%]}")"
}

# set-validators [--apply]: move the ISMs to the settings' validator sets.
# Dusk -> EVM: every EVM token's ISM to a MessageIdMultisig ISM of
# DUSK_ORIGIN_THRESHOLD of DUSK_ORIGIN_VALIDATORS, from EVM_ISM_FACTORY, or
# deployed from source without one. EVM -> Dusk: the validators and
# threshold of the Dusk ISMs the routes use, to EVM_ORIGIN_THRESHOLD of
# EVM_ORIGIN_VALIDATORS. Every validator in a set that changes must be
# announced and signing first. Without --apply it only checks and reports.
cmd_set_validators() {
    local apply="${1:-}" dusk_set evm_set recorded target wanted route token ism next owner out
    local validator count isms policy relayer_limit evm_change="" dusk_change=()
    [ -z "$apply" ] || [ "$apply" = --apply ] || fail "usage: bash deploy/testnet.sh set-validators [--apply]"
    [ -n "$(state_get '.evm.token')" ] && [ -n "$(state_get '.dusk.warp_native')" ] \
        || fail "No deployment recorded in $STATE: run deploy first"
    [ -x "$DUSK_TX" ] || fail "dusk-tx not built at $DUSK_TX (run: make dusk-tx)"
    [[ "$VALIDATOR_PROBE_SECONDS" =~ ^[0-9]+$ ]] || fail "VALIDATOR_PROBE_SECONDS must be a number of seconds"
    dusk_set="$(checked_validator_set DUSK_ORIGIN_VALIDATORS DUSK_ORIGIN_THRESHOLD)" || exit 1
    evm_set="$(checked_validator_set EVM_ORIGIN_VALIDATORS EVM_ORIGIN_THRESHOLD)" || exit 1
    # The new thresholds must be paid for and fit the relayer's gas: the
    # IGPs' overheads a delivery with that many signatures (compute_fees),
    # each IGP must charge what the settings say on chain, and
    # DUSK_RELAYER_GAS_LIMIT must cover EVM_ORIGIN_THRESHOLD.
    load_fees
    check_agent_settings
    check_dusk_deployment
    TOKEN_ISMS_CHECKED_ELSEWHERE=1
    check_evm_deployment
    check_dusk_igp_pricing
    check_evm_igp_pricing
    DEPLOYED_ISM_THRESHOLD=""
    check_relayer_gas_limit
    [ -z "$apply" ] || check_no_unattributed_evm_send

    # Dusk -> EVM. Every token verifies with the recorded ISM, or with the
    # new one an earlier run of this command moved it to before it could
    # record the switch.
    recorded="$(lower "$(state_get '.evm.ism')")"
    wanted="$DUSK_ORIGIN_THRESHOLD of [$dusk_set]"
    policy="$(evm_ism_policy "$recorded")" || exit 1
    if [ "$(sorted_policy "$policy")" != "$wanted" ]; then
        evm_change=1
        if [ -n "${EVM_ISM_FACTORY:-}" ]; then
            target="$(evm_call "$EVM_ISM_FACTORY" "getAddress(address[],uint8)(address)" "[$dusk_set]" "$DUSK_ORIGIN_THRESHOLD")" \
                || fail "Can't read the ISM EVM_ISM_FACTORY makes for $wanted"
            target="$(lower "$target")"
        else
            next="$(state_json | jq -c '.evm.next_ism // empty')"
            if [ -n "$next" ] && [ "$(jq -r '"\(.threshold) of [\(.validators | join(","))]"' <<<"$next")" = "$wanted" ]; then
                target="$(lower "$(jq -r '.address' <<<"$next")")"
            else
                target=""
            fi
        fi
    else
        target=""
    fi
    for route in $(recorded_routes); do
        token="$(state_get "$(route_evm_base "$route").token")"
        # An ISM update an earlier run sent may still land: it is settled
        # first, so the token reads as it will stay.
        if [ -n "$(state_json | jq -r --arg s "$(evm_call_step "$token" "setInterchainSecurityModule(address)")" '.evm_sending[$s] // empty')" ]; then
            require EVM_DEPLOYER_KEY
            evm_settle_call "$token" "setInterchainSecurityModule(address)"
        fi
        ism="$(evm_call "$token" "interchainSecurityModule()(address)")" \
            || fail "Can't read the ISM of the EVM $(route_symbol "$route") token $token"
        ism="$(lower "$ism")"
        if [ "$ism" != "$recorded" ] && { [ -z "$target" ] || [ "$ism" != "$target" ]; }; then
            fail "The EVM $(route_symbol "$route") token $token verifies messages from Dusk with the ISM $ism, not the $recorded recorded in $STATE${target:+, nor the $target this command moves it to}. Point it back at $recorded."
        fi
        if [ -n "$evm_change" ] && [ -n "${EVM_DEPLOYER_KEY:-}" ]; then
            owner="$(evm_call "$token" "owner()(address)")" || fail "Can't read the owner of the EVM token $token"
            [ "$(lower "$owner")" = "$(lower "$(evm_deployer)")" ] \
                || fail "The EVM $(route_symbol "$route") token $token is owned by $owner, not by EVM_DEPLOYER_KEY's $(evm_deployer): only its owner can change its ISM"
        fi
    done
    if [ -z "$evm_change" ]; then
        check_evm_ism_policy "$recorded" "The EVM ISM $recorded recorded in $STATE"
        ok "Dusk -> $EVM_CHAIN_NAME: the EVM tokens verify with $recorded, $wanted, as the settings say"
    else
        info "Dusk -> $EVM_CHAIN_NAME: the EVM tokens verify with $recorded, $policy; the settings say $wanted"
        count="$(dusk_value "the Dusk Merkle tree hook's count" \
            --contract "$(state_get '.dusk.merkle_tree_hook')" --method count --return-type u32)" || exit 1
        [ "$count" -gt 0 ] \
            || fail "Nothing has been dispatched on Dusk yet, so no validator has a checkpoint to show it is signing. Send a transfer first."
        for validator in ${dusk_set//,/ }; do
            check_dusk_origin_validator "$validator"
        done
    fi

    # EVM -> Dusk.
    isms="$(dusk_route_isms)" || exit 1
    for ism in $isms; do
        policy="$(dusk_value "the module type of the Dusk ISM $ism" \
            --contract "$ism" --method module_type --return-type u8)" || exit 1
        [ "$policy" = "$MESSAGE_ID_MULTISIG_ISM" ] \
            || fail "A Dusk route uses the ISM $ism, of module type $policy: set-validators sets only a MessageIdMultisig ISM's ($MESSAGE_ID_MULTISIG_ISM) validators"
        policy="$(dusk_value "the validators and threshold of the Dusk ISM $ism" \
            --contract "$ism" --method validators_and_threshold --return-type validators-and-threshold)" || exit 1
        policy="$(jq -r '"\(.threshold) of [\(.validators | map(ascii_downcase) | join(","))]"' <<<"$policy")" \
            || fail "The Dusk ISM $ism returned a malformed validator set"
        if [ "$policy" = "$EVM_ORIGIN_THRESHOLD of [$evm_set]" ]; then
            ok "$EVM_CHAIN_NAME -> Dusk: the Dusk ISM $ism takes $policy, as the settings say"
        else
            info "$EVM_CHAIN_NAME -> Dusk: the Dusk ISM $ism takes $policy; the settings say $EVM_ORIGIN_THRESHOLD of [$evm_set]"
            dusk_change+=("$ism")
        fi
    done
    if [ "${#dusk_change[@]}" -gt 0 ]; then
        # The relayer must already run with a gas limit that covers the new
        # threshold: every delivery after the switch takes that many
        # signatures.
        # RELAYER_CONFIG is the config the relayer runs, by default the one
        # agent-configs wrote.
        relayer_limit="$(jq -r '.chains.dusk.gasLimit // empty' "$RELAYER_CONFIG" 2>/dev/null || true)"
        if ! [[ "$relayer_limit" =~ ^[0-9]+$ ]]; then
            [ "${RELAYER_GAS_LIMIT_UNCHECKED:-0}" = 1 ] \
                || fail "Can't read the relayer's Dusk gas limit from RELAYER_CONFIG ($RELAYER_CONFIG). Every delivery after the switch takes $EVM_ORIGIN_THRESHOLD signatures: point RELAYER_CONFIG at the config the relayer runs, or set RELAYER_GAS_LIMIT_UNCHECKED=1 once you have checked its gas limit is at least DUSK_RELAYER_GAS_LIMIT ($DUSK_RELAYER_GAS_LIMIT)."
            warn "RELAYER_GAS_LIMIT_UNCHECKED=1: the relayer's Dusk gas limit is taken to be at least $DUSK_RELAYER_GAS_LIMIT"
        elif [ "$relayer_limit" -lt "$DUSK_RELAYER_GAS_LIMIT" ]; then
            fail "$RELAYER_CONFIG gives the relayer a Dusk gas limit of $relayer_limit, under the DUSK_RELAYER_GAS_LIMIT of $DUSK_RELAYER_GAS_LIMIT that EVM_ORIGIN_THRESHOLD $EVM_ORIGIN_THRESHOLD needs. With the EVM_ORIGIN settings as on chain, run agent-configs and restart the relayer with its new relayer.json first."
        fi
        count="$(evm_number "$(state_get '.evm.merkle_tree_hook')" "count()(uint32)")" \
            || fail "Can't read the count of $EVM_CHAIN_NAME's Merkle tree hook"
        [ "$count" -gt 0 ] || fail "Nothing has been dispatched through $EVM_CHAIN_NAME's Merkle tree hook yet"
        for validator in ${evm_set//,/ }; do
            check_evm_origin_validator "$validator"
        done
    fi

    if [ -z "$evm_change" ] && [ "${#dusk_change[@]}" = 0 ]; then
        ok "Both directions' ISMs take the settings' validators and thresholds: nothing to change"
        return
    fi
    if [ "$apply" != --apply ]; then
        info "Dry run: nothing was sent. Every validator in the new sets is announced and signing. To move the ISMs: bash deploy/testnet.sh set-validators --apply"
        return
    fi

    if [ -n "$evm_change" ]; then
        require EVM_DEPLOYER_KEY
        probe_dusk_origin_validators "$dusk_set"
        if [ -n "${EVM_ISM_FACTORY:-}" ]; then
            evm_settle_call "$EVM_ISM_FACTORY" "deploy(address[],uint8)"
            if ! evm_has_code "$target"; then
                info "Deploying MessageIdMultisigIsm ($wanted) through the factory..."
                evm_send "$EVM_ISM_FACTORY" "deploy(address[],uint8)" "[$dusk_set]" "$DUSK_ORIGIN_THRESHOLD" \
                    || fail "Deploying the new EVM security module failed"
            fi
        elif [ -z "$target" ]; then
            # One an earlier run deployed for another set, which no token
            # verifies with (checked above), is forgotten.
            state_set 'del(.evm.next_ism)'
            info "Deploying StorageMessageIdMultisigIsm ($wanted)..."
            forge_create target .evm.next_ism contracts/isms/multisig/StorageMultisigIsm.sol:StorageMessageIdMultisigIsm \
                --constructor-args "[$dusk_set]" "$DUSK_ORIGIN_THRESHOLD" || fail "Deploying the new EVM security module failed"
            target="$(lower "$target")"
            evm_record "$(evm_creation_step .evm.next_ism)" --arg ism "$target" --arg v "$dusk_set" \
                --argjson t "$DUSK_ORIGIN_THRESHOLD" \
                '.evm.next_ism = {address: $ism, validators: ($v | split(",")), threshold: $t}'
        fi
        evm_has_code "$target" || fail "No new EVM security module at $target"
        check_evm_ism_policy "$target" "The new EVM ISM $target"
        for route in $(recorded_routes); do
            token="$(state_get "$(route_evm_base "$route").token")"
            evm_settle_call "$token" "setInterchainSecurityModule(address)"
            ism="$(evm_call "$token" "interchainSecurityModule()(address)")" || fail "Can't read the ISM of the EVM token $token"
            if [ "$(lower "$ism")" != "$target" ]; then
                info "Pointing the EVM $(route_symbol "$route") token $token at the new ISM..."
                evm_send "$token" "setInterchainSecurityModule(address)" "$target" \
                    || fail "Setting the ISM of the EVM $(route_symbol "$route") token $token failed"
                ism="$(evm_call "$token" "interchainSecurityModule()(address)")" || fail "Can't read the ISM of the EVM token $token"
                [ "$(lower "$ism")" = "$target" ] \
                    || fail "The EVM $(route_symbol "$route") token $token verifies with $ism after it was set to $target"
            fi
        done
        # A reorg could still undo an update, this run's or one an earlier
        # run sent: the record names the new ISM only once every token
        # verifies with it as of a final block.
        tokens_final_at "$target"
        state_set --arg ism "$target" --arg old "$recorded" --arg v "$dusk_set" --argjson t "$DUSK_ORIGIN_THRESHOLD" \
            '.evm.previous_isms = ((.evm.previous_isms // []) + [$old]) | .evm.ism = $ism
             | .dusk_origin_validators = ($v | split(",")) | .dusk_origin_threshold = $t | del(.evm.next_ism)'
        ok "Dusk -> $EVM_CHAIN_NAME: the EVM tokens verify with $target, $wanted"
    fi
    if [ "${#dusk_change[@]}" -gt 0 ]; then
        require DUSK_KEYS DUSK_CONSENSUS_PASSWORD
        for ism in "${dusk_change[@]}"; do
            out="$(dusk_tx set-ism-validators --rues-url "$DUSK_RUES_URL" --keys "$DUSK_KEYS" --ism "$ism" \
                --validators "$evm_set" --threshold "$EVM_ORIGIN_THRESHOLD" \
                --expected-chain-id "$DUSK_EXPECTED_CHAIN_ID")" \
                || fail "Can't set the validators of the Dusk ISM $ism: $(jq -r '.error // empty' <<<"$out" 2>/dev/null)"
            ok "$EVM_CHAIN_NAME -> Dusk: the Dusk ISM $ism takes $EVM_ORIGIN_THRESHOLD of [$evm_set] ($(jq -r '.tx_id // "unchanged"' <<<"$out"))"
        done
    fi

    TOKEN_ISMS_CHECKED_ELSEWHERE=""
    check_evm_creations
    check_deployed_ism
    check_relayer_gas_limit
    ok "The ISMs take the settings' validator sets. The relayer reads them from the chains, so its config doesn't change; regenerate it (agent-configs) only if an IGP overhead changed."
}

dusk_chain_json() {
    local cursor_dir="$1" gas_limit="$2"
    jq -n --arg url "$DUSK_RUES_URL" --argjson domain "$DUSK_DOMAIN" \
        --argjson chain "$DUSK_EXPECTED_CHAIN_ID" --arg cursors "$cursor_dir" \
        --argjson gl "$gas_limit" --argjson gp "$DUSK_AGENT_GAS_PRICE" \
        --arg multicall "$DUSK_MULTICALL_ID" \
        --arg key_file "$DUSK_SIGNER_KEY_FILE" --argjson s "$(state_json)" '{
          name: "dusk", protocol: "dusk", domainId: $domain, chainId: $chain,
          rpcUrls: [{http: $url}], eventCursorDir: $cursors,
          gasLimit: $gl, gasPrice: $gp,
          mailbox: ("0x" + $s.dusk.mailbox),
          interchainGasPaymaster: ("0x" + $s.dusk.igp),
          validatorAnnounce: ("0x" + $s.dusk.validator_announce),
          merkleTreeHook: ("0x" + $s.dusk.merkle_tree_hook),
          submitter: "Classic",
          signer: {type: "duskKey", keyFile: $key_file}
        } + (if $multicall == "" then {} else {batchContractAddress: ("0x" + $multicall)} end)'
}

cmd_agent_configs() {
    [ -n "$(state_get '.evm.token')" ] || fail "Run deploy first"
    [ -x "$DUSK_TX" ] || fail "dusk-tx not built at $DUSK_TX (run: make dusk-tx)"
    load_fees
    check_agent_settings
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
        --argjson s "$(state_json)" '{
          name: $name, protocol: "ethereum", domainId: $domain, chainId: $chain,
          rpcUrls: [{http: $rpc}],
          blocks: {reorgPeriod: $reorg, estimateBlockTime: $bt},
          index: {from: $s.evm.index_from, chunk: 1999},
          mailbox: $s.evm.mailbox,
          interchainGasPaymaster: $s.evm.igp,
          validatorAnnounce: $s.evm.validator_announce,
          merkleTreeHook: $s.evm.merkle_tree_hook
        }')"

    # The relayer relays any message between the two chains (no whitelist),
    # once it is paid for: a message from each origin must have paid that
    # chain's IGP, the one this deployment owns, for enough gas
    # (onChainFeeQuoting; docs/costs.md#fee-coverage). A message from another
    # origin matches no policy and is not relayed.
    jq -n --arg evm "$EVM_CHAIN_NAME" --argjson evm_chain "$evm_chain" \
        --argjson dusk_chain "$(dusk_chain_json "$AGENT_STATE_DIR/relayer-dusk-events" "$DUSK_RELAYER_GAS_LIMIT")" \
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

    # A Dusk-origin validator an outside operator runs (docs/validators.md).
    # It has no Dusk account, so it doesn't announce itself: it waits until
    # `announce` sends the announcement it writes to its bucket. Its state
    # is under /data, and the operator's env file names its bucket
    # (HYP_CHECKPOINTSYNCER_*).
    jq -n --argjson dusk_chain "$(dusk_chain_json /data/validator-dusk-events "$DUSK_AGENT_GAS_LIMIT" \
            | jq --arg url "$OPERATOR_DUSK_RUES_URL" 'del(.signer) | .rpcUrls = [{http: $url}]')" '{
          db: "/data/validator-db",
          originChainName: "dusk",
          interval: 5,
          checkpointSyncer: {type: "s3", bucket: "<bucket>", region: "<region>", folder: "<folder>"},
          chains: {dusk: $dusk_chain},
          metricsPort: 9091
        }' > "$OUT_DIR/validator-dusk-operator.json"

    ok "Wrote $OUT_DIR/relayer.json, $OUT_DIR/validator-dusk.json and $OUT_DIR/validator-dusk-operator.json"
    ok "The relayer delivers to Dusk one at a time; pipelined deliveries are experimental and off (docs/operations.md, Deliveries to Dusk)"
    ok "The relayer relays a Dusk-origin message once its Dusk IGP payment covers $(fee .relayer.dusk_origin_gas_fraction) of the gas it estimates for $EVM_CHAIN_NAME, and an $EVM_CHAIN_NAME-origin one once it paid the EVM IGP $(state_get '.evm.igp') for $(fee .relayer.evm_origin_required_gas) Dusk gas"
    cat >&2 <<EOF
Secrets are not in these files. Provide them when starting the agents:
  relayer:   HYP_CHAINS_$(tr '[:lower:]' '[:upper:]' <<<"$EVM_CHAIN_NAME")_SIGNER_KEY=0x...  (funded on $EVM_CHAIN_NAME)
             a Dusk key file at $DUSK_SIGNER_KEY_FILE (funded on Dusk)
  validator: HYP_VALIDATOR_KEY=0x...  (one of DUSK_ORIGIN_VALIDATORS)
             a Dusk key file at $DUSK_SIGNER_KEY_FILE (pays for its announcement)
             AWS credentials for the checkpoint bucket
  an outside operator's validator (validator-dusk-operator.json): its own
             HYP_VALIDATOR_KEY, and the bucket's HYP_CHECKPOINTSYNCER_*,
             AWS_* and AWS_ENDPOINT_URL; no Dusk key (docs/validators.md)
Write a Dusk key file from a rusk-wallet export with:
  dusk-tx export-signer-key --keys <consensus.keys> --out $DUSK_SIGNER_KEY_FILE
Run the agents from hyperlane-monorepo/rust/main with DUSK_TX_BIN pointing at
dusk-tx. See docs/testnet.md.
EOF
}

parse_token_routes
case "${1:-}" in
    preflight|deploy|agent-configs|dusk-igp|evm-igp|revenue|claim|announce|set-validators)
        lock_deployment
        check_token_route_settings ;;
esac
case "${1:-}" in
    preflight) cmd_preflight ;;
    deploy) cmd_deploy ;;
    agent-configs) cmd_agent_configs ;;
    fees) cmd_fees "${2:-}" ;;
    dusk-igp) cmd_dusk_igp "${2:-}" ;;
    evm-igp) cmd_evm_igp "${2:-}" ;;
    revenue) cmd_revenue ;;
    claim) cmd_claim ;;
    announce) shift; cmd_announce "$@" ;;
    set-validators) cmd_set_validators "${2:-}" ;;
    *) fail "usage: bash deploy/testnet.sh preflight|deploy|agent-configs|fees [--json]|dusk-igp [--apply]|evm-igp [--apply]|revenue|claim|announce <location> [address]|set-validators [--apply]" ;;
esac
