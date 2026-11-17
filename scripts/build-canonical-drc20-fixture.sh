#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTRACTS_REVISION="bc1b00ee0af059975e158b7b580b4d0c0f1bdf9f"
# Where the pinned revision comes from: CANONICAL_DRC20_REPOSITORY if set,
# else a repository in Cargo's git cache that has it (the types crate's
# dev-dependency on dusk-network/contracts at the same revision puts it
# there), else GitHub. Whichever it is, the revision is checked below.
CONTRACTS_REPOSITORY="${CANONICAL_DRC20_REPOSITORY:-}"
if [ -z "$CONTRACTS_REPOSITORY" ]; then
    for cached in "${CARGO_HOME:-$HOME/.cargo}"/git/db/contracts-*; do
        if git -C "$cached" cat-file -e "$CONTRACTS_REVISION^{commit}" 2>/dev/null; then
            CONTRACTS_REPOSITORY="$cached"
            break
        fi
    done
fi
CONTRACTS_REPOSITORY="${CONTRACTS_REPOSITORY:-https://github.com/dusk-network/contracts}"
OUTPUT="$ROOT/target/contract/wasm32-unknown-unknown/release/canonical_drc20_roles_pausable.wasm"
BUILD_TARGET="$ROOT/target/canonical-drc20-fixture"
SOURCE_DIR="$(mktemp -d -t hyperlane-canonical-drc20.XXXXXXXX)"

cleanup() {
    rm -rf -- "$SOURCE_DIR"
}
trap cleanup EXIT

git -C "$SOURCE_DIR" init --quiet
git -C "$SOURCE_DIR" remote add origin "$CONTRACTS_REPOSITORY"
git -C "$SOURCE_DIR" fetch --quiet --depth=1 origin "$CONTRACTS_REVISION"
git -C "$SOURCE_DIR" checkout --quiet --detach FETCH_HEAD

actual_revision="$(git -C "$SOURCE_DIR" rev-parse HEAD)"
if [ "$actual_revision" != "$CONTRACTS_REVISION" ]; then
    echo "Canonical DRC20 source revision mismatch: expected $CONTRACTS_REVISION, got $actual_revision" >&2
    exit 1
fi

(
    cd "$SOURCE_DIR/standards"
    CARGO_TARGET_DIR="$BUILD_TARGET" \
        cargo build --release -Z build-std=core,alloc \
        --target wasm32-unknown-unknown \
        --features contract \
        -p drc20-roles-pausable
)

artifact="$BUILD_TARGET/wasm32-unknown-unknown/release/drc20_roles_pausable.wasm"
if [ ! -s "$artifact" ]; then
    echo "Canonical DRC20 build did not produce $artifact" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUTPUT")"
cp "$artifact" "$OUTPUT"
echo "Canonical DRC20 fixture: $OUTPUT ($CONTRACTS_REVISION)"
