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
CONTRACTS := mailbox merkle-tree-hook aggregation-hook ism-multisig validator-announce test-recipient test-mock protocol-fee igp warp-drc20 warp-drc20-collateral warp-native
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
	cargo check --target $(WASM_TARGET) --features $(CONTRACT_FEATURE) \
		-p hyperlane-dusk-types $(CONTRACT_CHECK_PACKAGES)

# Run clippy over the production contract/type surface.
#
# This intentionally avoids a workspace-wide wasm clippy pass because host/test
# dependencies pull wasm-unsupported getrandom paths before reaching the
# production contracts.
.PHONY: clippy-contracts
clippy-contracts:
	cargo clippy --target $(WASM_TARGET) --features $(CONTRACT_FEATURE) \
		$(PRODUCTION_CLIPPY_PACKAGES)

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

# Extract durable evidence archives and scan their contents for secrets.
.PHONY: archive-hygiene
archive-hygiene:
	bash scripts/archive-hygiene-check.sh

# Regression-test the archive hygiene scanner on safe and unsafe archives.
.PHONY: archive-hygiene-test
archive-hygiene-test:
	bash scripts/archive-hygiene-self-test.sh

# Export GitHub PR/issue review text and scan it for stale evidence/secrets.
.PHONY: review-hygiene
review-hygiene:
	bash scripts/github-review-hygiene.sh

.PHONY: report-hygiene
report-hygiene:
	bash scripts/report-hygiene-check.sh

.PHONY: fail-closed-self-test
fail-closed-self-test:
	bash scripts/fail-closed-self-test.sh

.PHONY: dependency-alert-status
dependency-alert-status:
	bash scripts/dependency-alert-status.sh

.PHONY: completion-audit-status
completion-audit-status:
	bash scripts/completion-audit-status.sh

.PHONY: dispatcher-merge-order-smoke
dispatcher-merge-order-smoke:
	bash scripts/dispatcher-merge-order-smoke.sh

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

.PHONY: gate-status-fresh
gate-status-fresh:
	bash scripts/release-gate-status.sh --fetch-upstream

# Run the lightweight reviewer gate bundle. This does not run the heavy repro
# or E2E scripts; it verifies preservation evidence, dependency-alert triage,
# reviewer-facing handoff text, dispatcher merge-order safety, and live
# PR/sign-off gate state.
.PHONY: review-gates
review-gates: completion-audit-status archive-hygiene-test fail-closed-self-test archive-hygiene dependency-alert-status report-hygiene review-hygiene dispatcher-merge-order-smoke gate-status-fresh

.PHONY: production-readiness-guard
production-readiness-guard:
	bash scripts/production-readiness-guard.sh

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
	@echo "  data-driver        Build data-driver WASM (for explorer)"
	@echo "  dusk-tx            Build dusk-tx CLI tool"
	@echo "  test               Run all tests"
	@echo "  secret-hygiene     Check source secret-handling guardrails"
	@echo "  archive-hygiene    Check extracted evidence archives for secrets"
	@echo "  archive-hygiene-test"
	@echo "                     Regression-test archive hygiene scanner rejection paths"
	@echo "  review-hygiene     Check GitHub review text for stale refs/secrets"
	@echo "  report-hygiene     Check local reports for stale evidence refs"
	@echo "  fail-closed-self-test"
	@echo "                     Regression-test guard scan-error failure paths"
	@echo "  dependency-alert-status"
	@echo "                     Compare open Dependabot Cargo.lock alerts to the local lockfile"
	@echo "  completion-audit-status"
	@echo "                     Verify preservation refs, backup hashes, and untracked source state"
	@echo "  dispatcher-merge-order-smoke"
	@echo "                     Check PR #3 and PR #1 land cleanly in either order"
	@echo "  repro-check        Run repeatable local pre-review checks"
	@echo "  repro-check-agent  Run repro-check plus Hyperlane agent cargo check"
	@echo "                     Set RUSK_DIR=/path/to/rusk-private to use a clean checkout"
	@echo "  gate-status        Print current review/sign-off gate status"
	@echo "  gate-status-fresh  Fetch Hyperlane upstream/main, then print gate status"
	@echo "  review-gates       Run lightweight review gates without E2E/repro"
	@echo "  production-readiness-guard"
	@echo "                     Fail while machine-checkable production blockers remain"
	@echo "  demo               Run cross-chain demo (Dusk <-> EVM)"
	@echo "  clean              Remove build artifacts"
