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
CONTRACTS := mailbox merkle-tree-hook ism-multisig validator-announce test-recipient test-mock protocol-fee igp warp-drc20 warp-drc20-collateral warp-native
TARGET_DIR := target/contract

# Build all contract WASMs
.PHONY: all
all: $(CONTRACTS)

.PHONY: $(CONTRACTS)
$(CONTRACTS):
	@echo "Building $@..."
	@CARGO_TARGET_DIR=$(TARGET_DIR) \
		RUSTFLAGS="$(RUSTFLAGS) -C link-args=-zstack-size=$(STACK_SIZE)" \
		cargo build -Z build-std=core,alloc \
			--release \
			--features $(CONTRACT_FEATURE) \
			--target $(WASM_TARGET) \
			-p hyperlane-dusk-$@
	@echo "  -> $(TARGET_DIR)/$(WASM_TARGET)/release/hyperlane_dusk_$(subst -,_,$@).wasm"

# Check all contracts compile
.PHONY: check
check:
	cargo check --target $(WASM_TARGET) --features $(CONTRACT_FEATURE) --workspace \
		--exclude hyperlane-dusk-integration-tests

# Run types crate unit tests
.PHONY: test-types
test-types:
	cargo test -p hyperlane-dusk-types

# Run integration tests (requires WASMs to be built first)
.PHONY: test-integration
test-integration: all
	cargo test -p hyperlane-dusk-integration-tests -- --nocapture

# Build data-driver WASM (for explorer integration)
# NOTE: Uses separate target dir and NO -Z build-std because data-driver
# depends on serde_json (std) which conflicts with rebuilt core/alloc.
.PHONY: data-driver
data-driver:
	@echo "Building data-driver WASM..."
	@CARGO_TARGET_DIR=target/data-driver \
		cargo build \
			--release \
			--features js \
			--target $(WASM_TARGET) \
			-p hyperlane-dusk-data-driver
	@echo "  -> target/data-driver/$(WASM_TARGET)/release/hyperlane_dusk_data_driver.wasm"
	@# Auto-copy to explorer if it exists
	@if [ -d "$(HOME)/projects/explorer/src/lib/assets" ]; then \
		cp target/data-driver/$(WASM_TARGET)/release/hyperlane_dusk_data_driver.wasm \
			$(HOME)/projects/explorer/src/lib/assets/; \
		echo "  -> copied to explorer/src/lib/assets/"; \
	fi

# Build dusk-tx CLI tool
.PHONY: dusk-tx
dusk-tx:
	cargo build -p dusk-tx --release

# Run all tests
.PHONY: test
test: test-types test-integration

# Check source and optional CI artifacts for secret-handling regressions.
.PHONY: secret-hygiene
secret-hygiene:
	bash scripts/secret-hygiene-check.sh

# Run the repeatable local verification subset used before review.
.PHONY: repro-check
repro-check:
	bash scripts/local-repro-check.sh

.PHONY: repro-check-agent
repro-check-agent:
	bash scripts/local-repro-check.sh --agent-check

.PHONY: gate-status
gate-status:
	bash scripts/release-gate-status.sh

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
	@echo "  ism-multisig       Build MultisigISM contract"
	@echo "  validator-announce Build ValidatorAnnounce contract"
	@echo "  test-recipient     Build TestRecipient contract"
	@echo "  test-mock          Build TestMock contract"
	@echo "  check              Check all contracts compile"
	@echo "  test-types         Run types crate tests"
	@echo "  test-integration   Run VM integration tests"
	@echo "  data-driver        Build data-driver WASM (for explorer)"
	@echo "  dusk-tx            Build dusk-tx CLI tool"
	@echo "  test               Run all tests"
	@echo "  secret-hygiene     Check source secret-handling guardrails"
	@echo "  repro-check        Run repeatable local pre-review checks"
	@echo "  repro-check-agent  Run repro-check plus Hyperlane agent cargo check"
	@echo "                     Set RUSK_DIR=/path/to/rusk-private to use a clean checkout"
	@echo "  gate-status        Print current review/sign-off gate status"
	@echo "  demo               Run cross-chain demo (Dusk <-> EVM)"
	@echo "  clean              Remove build artifacts"
