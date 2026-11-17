#!/usr/bin/env bash
# The VM tests run the genesis transfer and stake contracts committed in
# tests/tests/genesis-contracts/. Check they are byte for byte the ones
# rusk-private ships at RUSK_COMMIT (RUSK_VERSION) in rusk-recovery/assets,
# which `rusk recovery state` deploys in the genesis state the demo node
# starts from. Local only: reads the rusk-private checkout's git objects.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=/dev/null
source <(grep -E '^RUSK_(TAG|COMMIT)=' "$ROOT/RUSK_VERSION")
# Cargo builds the Rusk crates through this link, ../../rusk-private.
RUSK_DIR="$ROOT/rusk"

git -C "$RUSK_DIR" cat-file -e "$RUSK_COMMIT^{commit}" 2>/dev/null || {
    echo "error: the rusk-private checkout at $RUSK_DIR doesn't have $RUSK_TAG ($RUSK_COMMIT)" >&2
    exit 1
}

status=0
for name in transfer_contract stake_contract; do
    committed="$(sha256sum <"$ROOT/tests/tests/genesis-contracts/$name.wasm" | cut -d' ' -f1)"
    pinned="$(git -C "$RUSK_DIR" show "$RUSK_COMMIT:rusk-recovery/assets/$name.wasm" | sha256sum | cut -d' ' -f1)"
    if [ "$committed" = "$pinned" ]; then
        echo "ok: tests/tests/genesis-contracts/$name.wasm is $RUSK_TAG's ($committed)"
    else
        echo "error: tests/tests/genesis-contracts/$name.wasm ($committed) isn't $RUSK_TAG's rusk-recovery/assets/$name.wasm ($pinned)" >&2
        status=1
    fi
done
exit "$status"
