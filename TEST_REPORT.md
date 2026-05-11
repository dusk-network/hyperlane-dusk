# Dusk Hyperlane Test Report

Date: 2026-05-11

This report captures the current local verification for the revived Dusk
Hyperlane branches. It is not a production-readiness sign-off; the remaining
stress, fault-injection, and full security-review items are listed at the end.

## Repository State

| Component | Repository | Branch | Commit |
|---|---|---|---|
| Dusk contracts/tooling | `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | `5f160a2ce8eddd1b04b7de22178acfd3209c219e` |
| Hyperlane agent integration | `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | `e4a759c5a60ef01978f49c4aebf0fbe1fe57d639` |
| Local Rusk reference | `/home/hein_/projects/rusk-private` | local checkout | `c0c64db4659500d077bb253ad13acba0e347d3fc` |

Notes:

- The local `rusk-private` checkout was dirty during verification.
- Docker was unavailable in this WSL environment, so the agent E2E runs skipped
  the optional block explorers.
- The E2E scripts used ignored local files for dev keys and runtime config:
  `demo/.env.bridge` and `e2e/consensus.keys`.

## Commands Run

### Contract and Type Checks

```bash
cd /home/hein_/projects/hyperlane/dusk
make all
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
cargo test -p dusk-tx
```

Result:

- `make all`: passed, all contract WASMs built.
- `cargo test -p hyperlane-dusk-types`: passed, 28 tests.
- `cargo test -p hyperlane-dusk-integration-tests`: passed, 53 tests.
- `cargo test -p dusk-tx`: passed, 0 tests.

### Hyperlane Rust Agent Checks

```bash
cd /home/hein_/projects/hyperlane/hyperlane-monorepo/rust/main
cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
```

Result:

- Passed.

### Local EVM <-> Dusk Agent E2E: TestMock ISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
bash demo/e2e-agents.sh --only testMock --timeout 300
```

Result:

- Passed.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.

Artifacts:

- `/tmp/hyperlane-start-env-testMock-1778508966.log`
- `/tmp/hyperlane-deploy-testMock-1778508966.log`
- `/tmp/hyperlane-relayer-testMock-1778508966.log`
- `/tmp/hyperlane-relayer-testMock-1778508966.json`
- `/tmp/rusk-dev.log`

### Local EVM <-> Dusk Agent E2E: MessageIdMultisigISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
bash demo/e2e-agents.sh --only messageIdMultisig --timeout 300
```

Result:

- Passed.
- Validator and relayer started successfully.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.

Artifacts:

- `/tmp/hyperlane-start-env-messageIdMultisig-1778509428.log`
- `/tmp/hyperlane-deploy-messageIdMultisig-1778509428.log`
- `/tmp/hyperlane-validator-messageIdMultisig-1778509428.log`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778509428.log`
- `/tmp/hyperlane-validator-anvil-messageIdMultisig-1778509428.json`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778509428.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778509428/index.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778509428/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778509428/announcement.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778509428/metadata_latest.json`
- `/tmp/rusk-dev.log`

## Compatibility Fixes Applied During E2E

- Updated local EVM token deployment scripts for the current upstream
  `HypERC20` constructor, which now takes both scale numerator and denominator.
- Updated `dusk-tx` contract-existence probing so current Rusk 404 responses for
  missing contracts are treated as "not deployed" during idempotent deploys.

## Remaining Work Before Production Readiness

- Replace or explicitly justify current `#[contract(no_event)]` suppressions in
  production-facing contracts.
- Expand negative/security coverage for malformed messages, wrong domains,
  duplicate delivery, invalid metadata, insufficient signatures, unauthorized
  admin paths, dirty redeploys, and token-accounting failures.
- Run stress and reliability tests for restarts, duplicate messages, RPC
  failures, delayed checkpoints, high message volume, low signer balance, and
  metadata corruption.
- Complete contract hardening review against the Dusk standards references and
  update `SECURITY_REVIEW.md` with final assumptions and deviations.
- Re-run the full report on a clean Rusk checkout or document the exact local
  Rusk changes required for reproduction.
