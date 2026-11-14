# =============================================================================
# Hyperlane Dusk Contracts Makefile
# =============================================================================
#
# Builds all Hyperlane contract WASMs for the Dusk VM.
#
# Prerequisites:
#   - Rust nightly toolchain from rust-toolchain.toml with wasm32-unknown-unknown target
#   - wasm-opt (optional, for smaller binaries)

WASM_TARGET := wasm32-unknown-unknown
CONTRACT_FEATURE := contract
STACK_SIZE := 65536
CONTRACTS := mailbox merkle-tree-hook aggregation-hook ism-multisig validator-announce test-recipient test-mock reentrant-hook redteam-attacker protocol-fee igp warp-drc20 warp-drc20-collateral warp-native
CONTRACT_CHECK_PACKAGES := $(addprefix -p hyperlane-dusk-,$(CONTRACTS))
TARGET_DIR := target/contract
PRODUCTION_CLIPPY_PACKAGES := \
	-p hyperlane-dusk-types \
	-p hyperlane-dusk-mailbox \
	-p hyperlane-dusk-merkle-tree-hook \
	-p hyperlane-dusk-aggregation-hook \
	-p hyperlane-dusk-ism-multisig \
	-p hyperlane-dusk-validator-announce \
	-p hyperlane-dusk-protocol-fee \
	-p hyperlane-dusk-igp \
	-p hyperlane-dusk-warp-drc20 \
	-p hyperlane-dusk-warp-drc20-collateral \
	-p hyperlane-dusk-warp-native

# Build all contract WASMs. Symbols are stripped: the WASM name section is
# debug-only, and deployment is charged per byte.
.PHONY: all
all: $(CONTRACTS)

# Refuse to build against a rusk-private checkout other than RUSK_VERSION.
.PHONY: rusk-version
rusk-version:
	@bash scripts/check-rusk-version.sh

.PHONY: $(CONTRACTS)
$(CONTRACTS): rusk-version
	@echo "Building $@..."
	@CARGO_TARGET_DIR=$(TARGET_DIR) \
		CARGO_PROFILE_RELEASE_STRIP=symbols \
		RUSTFLAGS="$(RUSTFLAGS) -C link-args=-zstack-size=$(STACK_SIZE)" \
		cargo build -Z build-std=core,alloc \
			--release \
			--features $(CONTRACT_FEATURE) \
			--target $(WASM_TARGET) \
			-p hyperlane-dusk-$@
	@echo "  -> $(TARGET_DIR)/$(WASM_TARGET)/release/hyperlane_dusk_$(subst -,_,$@).wasm"

# Check all contracts compile
.PHONY: check
check: rusk-version
	cargo check --target $(WASM_TARGET) --features $(CONTRACT_FEATURE) \
		-p hyperlane-dusk-types $(CONTRACT_CHECK_PACKAGES)

# Run clippy over the production contract/type surface.
#
# This intentionally avoids a workspace-wide wasm clippy pass because host/test
# dependencies pull wasm-unsupported getrandom paths before reaching the
# production contracts.
.PHONY: clippy-contracts
clippy-contracts: rusk-version
	cargo clippy --target $(WASM_TARGET) --features $(CONTRACT_FEATURE) \
		$(PRODUCTION_CLIPPY_PACKAGES)

# Run types crate unit tests
.PHONY: test-types
test-types: rusk-version
	cargo test -p hyperlane-dusk-types --features drc20

# Run integration tests (requires WASMs to be built first)
.PHONY: canonical-drc20-fixture
canonical-drc20-fixture:
	bash scripts/build-canonical-drc20-fixture.sh

.PHONY: test-integration
test-integration: all canonical-drc20-fixture
	cargo test -p hyperlane-dusk-integration-tests -- --nocapture

# Build data-driver WASM (for explorer integration)
# NOTE: Uses separate target dir and NO -Z build-std because data-driver
# depends on serde_json (std) which conflicts with rebuilt core/alloc.
.PHONY: data-driver
data-driver: rusk-version
	@echo "Building data-driver WASM..."
	@CARGO_TARGET_DIR=target/data-driver \
		cargo build \
			--release \
			--features js \
			--target $(WASM_TARGET) \
			-p hyperlane-dusk-data-driver
	@echo "  -> target/data-driver/$(WASM_TARGET)/release/hyperlane_dusk_data_driver.wasm"

# Build dusk-tx CLI tool
.PHONY: dusk-tx
dusk-tx: rusk-version
	cargo build -p dusk-tx --release

# Run all tests
.PHONY: test
test: test-types test-integration

# Check source and optional CI artifacts for secret-handling regressions.
.PHONY: secret-hygiene
secret-hygiene:
	bash scripts/secret-hygiene-check.sh

# Build and test everything against a Rusk checkout (RUSK_DIR to override).
.PHONY: repro-check
repro-check:
	bash scripts/local-repro-check.sh

# repro-check plus a cargo check of the Hyperlane agents in the monorepo fork.
.PHONY: repro-check-agent
repro-check-agent:
	bash scripts/local-repro-check.sh --agent-check

# Run cross-chain demo (requires rusk-duskevm Docker + Foundry)
.PHONY: demo
demo: all dusk-tx
	bash demo/demo.sh

.PHONY: clean
clean:
	cargo clean
	rm -rf $(TARGET_DIR) target/data-driver

.PHONY: help
help:
	@echo "Hyperlane Dusk Contracts"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "  all                Build all contract WASMs"
	@echo "  mailbox            Build Mailbox contract"
	@echo "  merkle-tree-hook   Build MerkleTreeHook contract"
	@echo "  aggregation-hook   Build AggregationHook contract"
	@echo "  ism-multisig       Build MultisigISM contract"
	@echo "  validator-announce Build ValidatorAnnounce contract"
	@echo "  test-recipient     Build TestRecipient contract"
	@echo "  test-mock          Build TestMock contract"
	@echo "  check              Check all contracts compile"
	@echo "  clippy-contracts   Lint production contract/type crates for wasm"
	@echo "  test-types         Run types crate tests"
	@echo "  test-integration   Run VM integration tests"
	@echo "  canonical-drc20-fixture"
	@echo "                     Build the pinned upstream Dusk DRC20 VM fixture"
	@echo "  data-driver        Build data-driver WASM (for explorer)"
	@echo "  dusk-tx            Build dusk-tx CLI tool"
	@echo "  test               Run all tests"
	@echo "  test-sdk           Build the JS bindings and run the TypeScript SDK tests"
	@echo "  test-demo-lifecycle"
	@echo "                     Run the demo process-lifecycle tests (no network needed)"
	@echo "  secret-hygiene     Check source secret-handling guardrails"
	@echo "  repro-check        Build and test everything in a clean layout"
	@echo "  repro-check-agent  repro-check plus a cargo check of the Hyperlane agents"
	@echo "                     Set RUSK_DIR=/path/to/rusk-private to use another checkout"
	@echo "  demo               Run cross-chain demo (Dusk <-> EVM)"
	@echo "  clean              Remove build artifacts"

# Build the actual JavaScript/WASM interface and run its ABI/HTTP regressions.
# Requires Node.js/npm and wasm-bindgen-cli matching Cargo.lock (0.2.108).
.PHONY: test-sdk
test-sdk:
	CARGO_TARGET_DIR=target/wasm-bindings cargo build --locked --release \
		--target $(WASM_TARGET) -p hyperlane-dusk-wasm-bindings
	wasm-bindgen --target nodejs --out-dir wasm-bindings/pkg \
		target/wasm-bindings/$(WASM_TARGET)/release/hyperlane_dusk_wasm_bindings.wasm
	cd ts-sdk && npm ci --ignore-scripts && npm test

# Isolated process fixtures; does not require the bridge network or signers.
.PHONY: test-demo-lifecycle
test-demo-lifecycle:
	python3 -B scripts/test-demo-lifecycle.py
