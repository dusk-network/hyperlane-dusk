# Dusk Hyperlane Test Report

Date: 2026-05-11

This report captures the current local verification for the revived Dusk
Hyperlane branches. It is not a production-readiness sign-off; the remaining
stress, fault-injection, and full security-review items are listed at the end.

## Repository State

| Component | Repository | Branch | Commit |
|---|---|---|---|
| Dusk contracts/tooling | `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | `3e1f8c06f4310db4b2723c37cf937357c9fff4e0` |
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
- `cargo test -p hyperlane-dusk-integration-tests`: passed, 61 tests.
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
- Re-run after operational event-surface expansion also passed.

Artifacts:

- `/tmp/hyperlane-start-env-testMock-1778517850.log`
- `/tmp/hyperlane-deploy-testMock-1778517850.log`
- `/tmp/hyperlane-relayer-testMock-1778517850.log`
- `/tmp/hyperlane-relayer-testMock-1778517850.json`
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

- `/tmp/hyperlane-start-env-messageIdMultisig-1778510216.log`
- `/tmp/hyperlane-deploy-messageIdMultisig-1778510216.log`
- `/tmp/hyperlane-validator-messageIdMultisig-1778510216.log`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778510216.log`
- `/tmp/hyperlane-validator-anvil-messageIdMultisig-1778510216.json`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778510216.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778510216/index.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778510216/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778510216/announcement.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778510216/metadata_latest.json`
- `/tmp/rusk-dev.log`

### Dirty Redeploy Guard

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
bash demo/e2e-dirty-redeploy.sh
```

Result:

- Passed.
- Initial deterministic Dusk deployment succeeded.
- Second deployment against the same non-reset Rusk state failed as expected.
- Failure log contained `Refusing to deploy: contract IDs already exist on-chain`
  and recovery guidance to restart Rusk with fresh state.

Artifacts:

- `/tmp/hyperlane-dirty-redeploy-start-testMock-1778510813.log`
- `/tmp/hyperlane-dirty-redeploy-first-testMock-1778510813.log`
- `/tmp/hyperlane-dirty-redeploy-second-testMock-1778510813.log`

### Relayer Restart and Backlog Stress

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=420 \
TRANSFERS=5 \
bash demo/e2e-relayer-restart-stress.sh
```

Result:

- Passed.
- Submitted 5 EVM -> Dusk transfers through a live relayer.
- Stopped the relayer after the EVM -> Dusk burst was delivered.
- Submitted 5 Dusk -> EVM transfers while the relayer was down, waiting for
  Dusk Mailbox nonce inclusion after each submission.
- Restarted the relayer with the same config/database.
- Verified EVM balance returned to `10000000000000000000` and Dusk supply
  returned to `0`.

Artifacts:

- `/tmp/hyperlane-restart-stress-start-testMock-1778512771.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778512771.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778512771.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778512771.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778512771/`

### Validator Delay and Checkpoint Backoff

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=360 \
VALIDATOR_DELAY_SECS=35 \
bash demo/e2e-validator-delay.sh
```

Result:

- Passed.
- Deployed a fresh MessageIdMultisig environment.
- Started the relayer before the validator.
- Submitted 1 EVM -> Dusk transfer while no validator checkpoint metadata was
  available.
- Verified Dusk token supply did not change during the 35 second validator
  delay.
- Started the validator, allowed the relayer to recover through its normal
  retry/backoff path, and verified the delayed message was delivered.

Artifacts:

- `/tmp/hyperlane-validator-delay-start-messageIdMultisig-1778513634.log`
- `/tmp/hyperlane-validator-delay-deploy-messageIdMultisig-1778513634.log`
- `/tmp/hyperlane-validator-delay-relayer-messageIdMultisig-1778513634.log`
- `/tmp/hyperlane-validator-delay-validator-messageIdMultisig-1778513634.log`

### Corrupt Checkpoint Metadata Recovery

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=420 \
CORRUPT_METADATA_SECS=35 \
bash demo/e2e-corrupt-checkpoint-metadata.sh
```

Result:

- Passed.
- Deployed a fresh MessageIdMultisig environment.
- Started the validator, submitted 1 EVM -> Dusk transfer, and waited for a
  valid checkpoint at index 0.
- Stopped the validator, saved the valid checkpoint, and corrupted
  `signature.r`, `signature.s`, `signature.v`, and `serialized_signature` in
  the local checkpoint file.
- Started the relayer and verified Dusk token supply did not change during the
  35 second corruption window.
- Restored the valid checkpoint file and verified the relayer delivered the
  message.

Artifacts:

- `/tmp/hyperlane-corrupt-metadata-start-messageIdMultisig-1778514463.log`
- `/tmp/hyperlane-corrupt-metadata-deploy-messageIdMultisig-1778514463.log`
- `/tmp/hyperlane-corrupt-metadata-relayer-messageIdMultisig-1778514463.log`
- `/tmp/hyperlane-corrupt-metadata-validator-messageIdMultisig-1778514463.log`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778514463/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778514463/0_with_id.json.valid`

### Low Dusk Relayer Signer Balance

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
LOW_SIGNER_SECS=35 \
bash demo/e2e-low-dusk-signer-balance.sh
```

Result:

- Passed.
- Deployed a fresh TestMock environment.
- Generated relayer configs with separate databases for the low-balance and
  funded-signer phases.
- Rewrote the first relayer config to use a deterministic unfunded Dusk test
  key (`0x11...11`) for the destination signer.
- Submitted 1 EVM -> Dusk transfer and verified Dusk token supply did not
  change during the 35 second low-balance window.
- The low-balance relayer log showed Dusk preverification rejecting `process`
  transactions with `Value spent larger than account holds`; the relayer stayed
  alive and retried.
- Restarted the relayer with the funded local dev signer and verified the same
  message delivered.

Artifacts:

- `/tmp/hyperlane-low-signer-start-testMock-1778514942.log`
- `/tmp/hyperlane-low-signer-deploy-testMock-1778514942.log`
- `/tmp/hyperlane-low-signer-relayer-low-testMock-1778514942.log`
- `/tmp/hyperlane-low-signer-relayer-funded-testMock-1778514942.log`
- `/tmp/hyperlane-relayer-low-signer-testMock-1778514942.json`
- `/tmp/hyperlane-relayer-funded-signer-testMock-1778514942.json`

### Origin RPC Failure Recovery

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
RPC_FAILURE_SECS=35 \
bash demo/e2e-origin-rpc-failure.sh
```

Result:

- Passed.
- Deployed a fresh TestMock environment.
- Generated relayer configs with separate databases for the bad-origin-RPC and
  healthy-origin-RPC phases.
- Rewrote the first relayer config to point Anvil RPC reads at
  `http://127.0.0.1:18545`, an unreachable local port.
- Submitted 1 EVM -> Dusk transfer through the healthy Anvil RPC and verified
  Dusk token supply did not change during the 35 second bad-RPC window.
- The bad-RPC relayer log showed repeated `Connection refused` errors against
  `127.0.0.1:18545` and critical chain build errors for the Anvil origin and
  destination views.
- Restarted the relayer with the healthy Anvil RPC config and verified the same
  message delivered.

Artifacts:

- `/tmp/hyperlane-rpc-failure-start-testMock-1778515388.log`
- `/tmp/hyperlane-rpc-failure-deploy-testMock-1778515388.log`
- `/tmp/hyperlane-rpc-failure-relayer-bad-rpc-testMock-1778515388.log`
- `/tmp/hyperlane-rpc-failure-relayer-healthy-testMock-1778515388.log`
- `/tmp/hyperlane-relayer-bad-origin-rpc-testMock-1778515388.json`
- `/tmp/hyperlane-relayer-healthy-origin-rpc-testMock-1778515388.json`

### Destination RPC Failure Recovery

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
RPC_FAILURE_SECS=35 \
bash demo/e2e-destination-rpc-failure.sh
```

Result:

- Passed.
- Deployed a fresh TestMock environment.
- Generated relayer configs with separate databases for the bad-destination-RPC
  and healthy-destination-RPC phases.
- Rewrote the first relayer config to point Dusk RUES reads/submissions at
  `http://127.0.0.1:18080`, an unreachable local port.
- Submitted 1 EVM -> Dusk transfer and verified Dusk token supply did not
  change during the 35 second bad-RPC window.
- The bad-RPC relayer log showed repeated `Connection refused` errors against
  `127.0.0.1:18080` when checking Dusk Mailbox delivery status.
- Restarted the relayer with the healthy Dusk RPC config and verified the same
  message delivered.

Artifacts:

- `/tmp/hyperlane-destination-rpc-failure-start-testMock-1778515791.log`
- `/tmp/hyperlane-destination-rpc-failure-deploy-testMock-1778515791.log`
- `/tmp/hyperlane-destination-rpc-failure-relayer-bad-rpc-testMock-1778515791.log`
- `/tmp/hyperlane-destination-rpc-failure-relayer-healthy-testMock-1778515791.log`
- `/tmp/hyperlane-relayer-bad-destination-rpc-testMock-1778515791.json`
- `/tmp/hyperlane-relayer-healthy-destination-rpc-testMock-1778515791.json`

### Duplicate Relayer Attempt

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=300 \
STABILITY_SECS=30 \
bash demo/e2e-duplicate-relayer-attempt.sh
```

Result:

- Passed.
- Deployed a fresh TestMock environment.
- Started two relayer instances with separate RocksDB paths and metrics ports.
- Submitted 1 EVM -> Dusk transfer.
- Verified first delivery by waiting for Dusk token supply to increase by the
  transferred amount.
- Kept both relayers running for a 30 second stability window and verified Dusk
  token supply did not increase again.
- Relayer B log showed it also picked up and attempted the same message; Dusk
  rejected repeated transaction submissions with `this transaction's spendId
  exists in the mempool`.

Artifacts:

- `/tmp/hyperlane-duplicate-relayer-start-testMock-1778516191.log`
- `/tmp/hyperlane-duplicate-relayer-deploy-testMock-1778516191.log`
- `/tmp/hyperlane-duplicate-relayer-a-testMock-1778516191.log`
- `/tmp/hyperlane-duplicate-relayer-b-testMock-1778516191.log`
- `/tmp/hyperlane-relayer-duplicate-a-testMock-1778516191.json`
- `/tmp/hyperlane-relayer-duplicate-b-testMock-1778516191.json`

## Rusk Checkout Reproduction Caveat

The local agent E2E evidence in this report used:

- Rusk checkout: `/home/hein_/projects/rusk-private`
- Branch: `hein/boreas-wallet-transfer-gas-50m`
- Base commit: `c0c64db4659500d077bb253ad13acba0e347d3fc`

The checkout was not clean during the E2E runs. At the time of the latest
TestMock rerun, `git diff --name-only` reported 49 modified tracked files and
`git ls-files --others --exclude-standard` reported 14263 untracked paths.
The untracked paths are dominated by generated `dedup-*`/state/database
artifacts and local target/output directories, but the checkout also contains
untracked source/docs/scripts such as:

- `.github/CODEOWNERS`
- `docs/`
- `node/src/chain/acceptor/`
- `node/src/database/rocksdb/`
- `rusk-wallet/src/wallet/metadata.rs`
- `rusk/src/lib/http/graphql_http.rs`
- `rusk/src/lib/http/rues_http.rs`
- `rusk/src/lib/node/policy.rs`
- `scripts/*startup*`
- `startup-slowdown-report.md`
- `wallet-core/src/keys/{eip2334,legacy,phoenix_hd}.rs`
- `wallet-core/src/keystore.rs`

The tracked modifications span Rusk node/archive/VM configuration, wallet,
wallet-core, node-data, CI, and example genesis/wallet files. Because this is
an extensive local working tree rather than a small reproducibility patch, the
current E2E evidence should be treated as **dirty-Rusk evidence**. A clean Rusk
checkout rerun, or a dedicated Rusk branch containing the exact required
changes, remains a production-readiness gate.

## Compatibility Fixes Applied During E2E

- Updated local EVM token deployment scripts for the current upstream
  `HypERC20` constructor, which now takes both scale numerator and denominator.
- Updated `dusk-tx` contract-existence probing so current Rusk 404 responses for
  missing contracts are treated as "not deployed" during idempotent deploys.
- Tightened `MessageIdMultisigISM` verification to reject uninitialized state
  and malformed signature metadata with trailing partial-signature bytes.
- Added corrupted fixed-width multisig signature metadata coverage; the ISM
  rejects corrupt bytes at `secp256k1_recover`.
- Replaced the misleading `#[contract(no_event)]` marker on Mailbox
  `dispatch_default` and added explicit `#[contract(emits = ...)]`
  annotations to protocol entrypoints that already emit Hyperlane events:
  Mailbox dispatch/process/admin hook setters, MerkleTreeHook, ProtocolFee,
  IGP, ValidatorAnnounce, and warp-route send/receive paths.
- Added operational events for initialization, ownership/configuration changes,
  account registration, validator-set updates, IGP domain gas config updates,
  native pending-transfer claims, and WarpDrc20 transfer/mint/burn accounting.
- Reviewed remaining `#[contract(no_event)]` uses. They are now limited to
  test-only contracts (`TestMock` and `TestRecipient`); production contracts no
  longer use `#[contract(no_event)]`.
- Removed the remaining direct `panic!` invocation from production contract
  code by rewriting Mailbox sender resolution to use the same explicit
  `expect(...)` revert style used elsewhere.

Event annotation verification:

```bash
rg -n "todo!|unimplemented!|panic!" contracts types data-driver dusk-tx e2e wasm-bindings demo -g '!target'
make all
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
```

Result:

- No `todo!`, `unimplemented!`, or direct `panic!` invocations remain in the
  scanned production contract/runtime/tooling paths.
- `make all` passed for all contract WASM builds after adding explicit event
  annotations.
- `cargo test -p hyperlane-dusk-types` passed:
  `28 passed; 0 failed; 0 ignored`.
- `cargo test -p hyperlane-dusk-integration-tests` passed:
  `61 passed; 0 failed; 0 ignored`.

## Remaining Work Before Production Readiness

- Continue expanding negative/security coverage; current coverage includes
  malformed mailbox messages, wrong domains, duplicate delivery, invalid
  multisig metadata, insufficient signatures, unauthorized multisig admin
  paths, dirty redeploy refusal, and several token-accounting failures, but
  more cross-route failure cases remain.
- Continue stress and reliability testing. Current coverage includes dirty
  redeploy refusal, 5-message relayer burst delivery, relayer restart/backlog
  recovery, and delayed validator checkpoint recovery through relayer metadata
  backoff. No currently listed reliability scenario remains untested in this
  report, but longer-duration and larger-volume runs are still needed before
  production readiness.
- Complete contract hardening review against the Dusk standards references and
  update `SECURITY_REVIEW.md` with final assumptions and deviations.
- Re-run the full report on a clean Rusk checkout or a dedicated Rusk branch
  containing the exact required changes. The current report documents the
  dirty local Rusk state, but does not convert it into a minimal reproduction
  branch.
