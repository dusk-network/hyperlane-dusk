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

# Contract WASMs must be byte-for-byte reproducible: a contract ID is derived
# from its bytecode, so anyone should be able to rebuild a release and check a
# deployed ID. Panic locations embed source paths, so map the machine-specific
# prefixes (Cargo home, the toolchain's library sources and this checkout) to
# fixed ones. The Rusk crates are inside this checkout, through the `rusk`
# link, so their paths are relative already and don't depend on where
# rusk-private is (see Cargo.toml).
CARGO_HOME ?= $(HOME)/.cargo
RUST_SYSROOT := $(shell rustc --print sysroot 2>/dev/null)
WASM_REMAP := \
	--remap-path-prefix=$(CARGO_HOME)/registry/src/= \
	--remap-path-prefix=$(CARGO_HOME)/git/checkouts/=git/ \
	--remap-path-prefix=$(RUST_SYSROOT)/lib/rustlib/src/rust/=rust/ \
	--remap-path-prefix=$(CURDIR)/=
CONTRACTS := mailbox merkle-tree-hook aggregation-hook ism-multisig validator-announce test-recipient plain-recipient test-mock reentrant-hook redteam-attacker callback-fee-token protocol-fee igp warp-drc20 warp-drc20-collateral warp-native
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

# Build all contract WASMs. Symbols are stripped (the WASM name section is
# debug-only, and deployment is charged per byte), and each contract is built
# as one codegen unit with LTO, which is smaller and doesn't depend on how
# the build is scheduled.
.PHONY: all
all: $(CONTRACTS) warp-drc20-collateral-unguarded

# Refuse to build against a rusk-private checkout other than RUSK_VERSION.
.PHONY: rusk-version
rusk-version:
	@bash scripts/check-rusk-version.sh

.PHONY: $(CONTRACTS)
$(CONTRACTS): rusk-version
	@echo "Building $@..."
	@CARGO_TARGET_DIR=$(TARGET_DIR) \
		CARGO_PROFILE_RELEASE_STRIP=symbols \
		CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
		CARGO_PROFILE_RELEASE_LTO=true \
		RUSTFLAGS="$(RUSTFLAGS) -C link-args=-zstack-size=$(STACK_SIZE) $(WASM_REMAP)" \
		cargo build -Z build-std=core,alloc \
			--locked \
			--release \
			--features $(CONTRACT_FEATURE) \
			--target $(WASM_TARGET) \
			-p hyperlane-dusk-$@
	@echo "  -> $(TARGET_DIR)/$(WASM_TARGET)/release/hyperlane_dusk_$(subst -,_,$@).wasm"

# Test only: the collateral route without its reentrancy guard, so the VM
# tests can show the attack the guard stops (docs/security.md, MEDIUM-13).
# Built with the same flags in its own target directory. Never deploy it.
UNGUARDED_TARGET_DIR := target/contract-unguarded
.PHONY: warp-drc20-collateral-unguarded
warp-drc20-collateral-unguarded: rusk-version
	@echo "Building $@ (test only)..."
	@CARGO_TARGET_DIR=$(UNGUARDED_TARGET_DIR) \
		CARGO_PROFILE_RELEASE_STRIP=symbols \
		CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
		CARGO_PROFILE_RELEASE_LTO=true \
		RUSTFLAGS="$(RUSTFLAGS) -C link-args=-zstack-size=$(STACK_SIZE) $(WASM_REMAP) --cfg hyperlane_dusk_unguarded_test" \
		cargo build -Z build-std=core,alloc \
			--locked \
			--release \
			--features $(CONTRACT_FEATURE) \
			--target $(WASM_TARGET) \
			-p hyperlane-dusk-warp-drc20-collateral
	@mkdir -p $(TARGET_DIR)/$(WASM_TARGET)/release
	@cp $(UNGUARDED_TARGET_DIR)/$(WASM_TARGET)/release/hyperlane_dusk_warp_drc20_collateral.wasm \
		$(TARGET_DIR)/$(WASM_TARGET)/release/hyperlane_dusk_warp_drc20_collateral_unguarded.wasm
	@echo "  -> $(TARGET_DIR)/$(WASM_TARGET)/release/hyperlane_dusk_warp_drc20_collateral_unguarded.wasm"

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
	rm -rf $(TARGET_DIR) $(UNGUARDED_TARGET_DIR) target/data-driver

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
	@echo "  warp-drc20-collateral-unguarded"
	@echo "                     Build the test-only collateral route without its"
	@echo "                     reentrancy guard (VM tests only, never deploy)"
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
	@echo "  test-deploy-scripts"
	@echo "                     Run the deploy-script tests against stubbed tools"
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

# deploy/testnet.sh and demo/gen-agent-configs.sh against stubbed tools. Needs
# no network, chain or Rust build; with target/release/dusk-tx (or
# DUSK_TX_BIN) it also runs the real key export, and with Anvil, Foundry and
# ../hyperlane-monorepo/solidity (or HYPERLANE_SOLIDITY_DIR) it also deploys
# the EVM side on a local Anvil.
.PHONY: test-deploy-scripts
test-deploy-scripts:
	python3 -B scripts/test-deploy-scripts.py
