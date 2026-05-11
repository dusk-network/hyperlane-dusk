# Dusk Hyperlane Test Report

Date: 2026-05-11

This report captures the current local verification for the revived Dusk
Hyperlane branches. It is not a production-readiness sign-off; the remaining
stress, fault-injection, and full security-review items are listed at the end.

## Repository State

| Component | Repository | Branch | Evidence commit |
|---|---|---|---|
| Dusk contracts/tooling | `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | `e4d3f2ab704286fe89e43b24543f8104b8838633` |
| Hyperlane agent integration | `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | `e4a759c5a60ef01978f49c4aebf0fbe1fe57d639` |
| Local Rusk reference | `/home/hein_/projects/rusk-private` | local checkout | `c0c64db4659500d077bb253ad13acba0e347d3fc` |
| Clean Rusk reproduction probe | `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` | detached HEAD | `c0c64db4659500d077bb253ad13acba0e347d3fc` |

Notes:

- Most local agent E2E evidence used the dirty local `rusk-private` checkout.
  A clean detached Rusk worktree at `c0c64db4659500d077bb253ad13acba0e347d3fc`
  has now passed the TestMock and MessageIdMultisig E2E paths plus the
  50-transfer relayer restart/backlog stress test and the currently documented
  fault-injection scenarios.
- Docker was unavailable in this WSL environment, so the agent E2E runs skipped
  the optional block explorers.
- The E2E scripts used ignored local files for dev keys and runtime config:
  `demo/.env.bridge` and `e2e/consensus.keys`.
- `dusk-tx` accepts consensus key passwords through
  `DUSK_CONSENSUS_PASSWORD_FILE`, `DUSK_CONSENSUS_PASSWORD`, or
  `DUSK_CONSENSUS_KEYS_PASS`; demo scripts avoid passing Dusk consensus
  passwords through process argv.
- `SECRET_HANDLING.md` documents the local/production boundary, and
  `make secret-hygiene` checks tracked source plus optional CI artifact paths
  for secret-handling regressions.

## Commands Run

### Contract and Type Checks

```bash
cd /home/hein_/projects/hyperlane/dusk
make all
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
cargo test -p dusk-tx
make secret-hygiene
make repro-check-agent
bash scripts/local-repro-check.sh --agent-check
bash scripts/secret-hygiene-check.sh /tmp/hyperlane-relayer-testMock-1778530398.log
if bash scripts/secret-hygiene-check.sh /tmp/hyperlane-relayer-testMock-1778530398.json \
  >/tmp/hyperlane-secret-hygiene-negative.log 2>&1; then
  exit 1
fi
```

Result:

- `make all`: passed, all contract WASMs built.
- `cargo test -p hyperlane-dusk-types`: passed, 28 tests.
- `cargo test -p hyperlane-dusk-integration-tests`: passed, 67 tests.
- `cargo test -p dusk-tx`: passed, 3 tests.
- `make secret-hygiene`: passed.
- `make repro-check-agent`: added as a Makefile wrapper for the full local
  non-E2E repro command, including the Hyperlane Rust agent check.
- `bash scripts/local-repro-check.sh --agent-check`: passed. This wraps the
  same repeatable non-E2E checks plus the Hyperlane Rust agent check. It still
  requires local/private Rusk path dependencies and does not replace the
  E2E/fault-injection runs below.
- `.github/workflows/manual-repro-check.yml`: added after the local repro run as
  a manual self-hosted workflow template for the same `make repro-check-agent`
  command. It requires Dusk to provide a `dusk-hyperlane` self-hosted runner and
  `DUSK_ORG_READ_TOKEN` secret for private cross-repo checkout.
- `actionlint .github/workflows/manual-repro-check.yml`: passed after adding
  `.github/actionlint.yaml` for the custom self-hosted `dusk-hyperlane` label.
- `make secret-hygiene`: re-run after manual workflow secret-scope docs and
  checkout credential hardening; passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-relayer-testMock-1778530398.log`:
  passed.
- Negative artifact scan against `/tmp/hyperlane-relayer-testMock-1778530398.json`:
  failed as expected after detecting generated `hexKey` and `duskKey` signer
  config entries.

### Hyperlane Rust Agent Checks

```bash
cd /home/hein_/projects/hyperlane/hyperlane-monorepo/rust/main
cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
```

Result:

- Passed. Re-run after Dusk event/type changes also passed.

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

### Clean Rusk Reproduction Probe: TestMock ISM

```bash
cd /home/hein_/projects/hyperlane/rusk-private-clean-c0c64db
cargo build --release -p dusk-rusk --features archive
make prepare-dev

cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
TIMEOUT_SECS=300 \
bash demo/e2e-agents.sh --only testMock --timeout 300
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`
  with a clean worktree and freshly regenerated `/tmp/example.state`.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.

Artifacts:

- `/tmp/hyperlane-start-env-testMock-1778520709.log`
- `/tmp/hyperlane-deploy-testMock-1778520709.log`
- `/tmp/hyperlane-relayer-testMock-1778520709.log`
- `/tmp/hyperlane-relayer-testMock-1778520709.json`
- `/tmp/hyperlane-db-relayer-testMock-1778520709/`
- `/tmp/rusk-dev.log`

### Clean Rusk Secret-Handling Smoke: TestMock ISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
TIMEOUT_SECS=300 \
bash demo/e2e-agents.sh --only testMock --timeout 300
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`
  after switching demo `dusk-tx` invocations from `--password` argv to
  `DUSK_CONSENSUS_PASSWORD`.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.

Artifacts:

- `/tmp/hyperlane-start-env-testMock-1778530398.log`
- `/tmp/hyperlane-deploy-testMock-1778530398.log`
- `/tmp/hyperlane-relayer-testMock-1778530398.log`
- `/tmp/hyperlane-relayer-testMock-1778530398.json`
- `/tmp/hyperlane-db-relayer-testMock-1778530398/`
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
- Re-run after operational event-surface expansion also passed.

Artifacts:

- `/tmp/hyperlane-start-env-messageIdMultisig-1778518318.log`
- `/tmp/hyperlane-deploy-messageIdMultisig-1778518318.log`
- `/tmp/hyperlane-validator-messageIdMultisig-1778518318.log`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778518318.log`
- `/tmp/hyperlane-validator-anvil-messageIdMultisig-1778518318.json`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778518318.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778518318/index.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778518318/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778518318/announcement.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778518318/metadata_latest.json`
- `/tmp/rusk-dev.log`

### Clean Rusk Reproduction Probe: MessageIdMultisigISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
TIMEOUT_SECS=300 \
bash demo/e2e-agents.sh --only messageIdMultisig --timeout 300
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`
  with a clean worktree and the regenerated `/tmp/example.state`.
- Validator and relayer started successfully.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.

Artifacts:

- `/tmp/hyperlane-start-env-messageIdMultisig-1778521018.log`
- `/tmp/hyperlane-deploy-messageIdMultisig-1778521018.log`
- `/tmp/hyperlane-validator-messageIdMultisig-1778521018.log`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778521018.log`
- `/tmp/hyperlane-validator-anvil-messageIdMultisig-1778521018.json`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778521018.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778521018/index.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778521018/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778521018/announcement.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778521018/metadata_latest.json`
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
- Clean Rusk rerun:
  `/tmp/hyperlane-dirty-redeploy-start-testMock-1778522551.log`,
  `/tmp/hyperlane-dirty-redeploy-first-testMock-1778522551.log`,
  `/tmp/hyperlane-dirty-redeploy-second-testMock-1778522551.log`

### Relayer Restart and Backlog Stress

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/rusk-private \
TIMEOUT_SECS=600 \
TRANSFERS=20 \
TRANSFER_AMOUNT_WEI=500000000000000000 \
bash demo/e2e-relayer-restart-stress.sh
```

Result:

- Passed.
- Submitted 20 EVM -> Dusk transfers through a live relayer
  (`500000000000000000` wei each).
- Stopped the relayer after the EVM -> Dusk burst was delivered.
- Submitted 20 Dusk -> EVM transfers while the relayer was down, waiting for
  Dusk Mailbox nonce inclusion after each submission.
- Restarted the relayer with the same config/database.
- Verified EVM balance returned to `10000000000000000000` and Dusk supply
  returned to `0`.
- A prior `TRANSFERS=20` attempt using the default 1 DUSK amount failed at
  EVM -> Dusk transfer 11 with `ERC20: burn amount exceeds balance`, which was
  a local test-account funding limit. The script now supports
  `TRANSFER_AMOUNT_WEI` so high-count runs can stay within the funded balance.

Clean Rusk rerun:

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
TIMEOUT_SECS=600 \
TRANSFERS=20 \
TRANSFER_AMOUNT_WEI=500000000000000000 \
bash demo/e2e-relayer-restart-stress.sh
```

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`
  with the regenerated clean `/tmp/example.state`.
- The EVM -> Dusk burst hit repeated Rusk mempool pressure:
  `spendId exists in the mempool`, then recovered and reached the target
  Dusk supply.
- The Dusk -> EVM backlog was queued with the relayer stopped and delivered
  after relayer restart with the same config and DB.
- Final balances matched the starting state:
  EVM account returned to 10 wDUSK and Dusk wrapped supply returned to 0.

Larger clean Rusk rerun:

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
TIMEOUT_SECS=1200 \
TRANSFERS=50 \
TRANSFER_AMOUNT_WEI=100000000000000000 \
bash demo/e2e-relayer-restart-stress.sh
```

- Passed on the same clean detached Rusk commit
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- EVM -> Dusk burst: 50 transfers at 0.1 wDUSK each delivered, reaching
  5 wDUSK Dusk wrapped supply.
- Dusk -> EVM backlog: 50 transfers queued while the relayer was stopped and
  delivered after restart with the same config and DB.
- The EVM -> Dusk burst again hit repeated `spendId exists in the mempool`
  preverify responses under high fan-out, then recovered.
- Final balances matched the starting state:
  EVM account returned to 10 wDUSK and Dusk wrapped supply returned to 0.

Soak wrapper smoke on clean Rusk:

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
SOAK_CYCLES=1 \
TRANSFERS=1 \
TRANSFER_AMOUNT_WEI=100000000000000000 \
TIMEOUT_SECS=300 \
bash demo/e2e-soak-restart-stress.sh
```

- Passed on the same clean detached Rusk commit
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Completed 1 full restart/backlog cycle in 277 seconds.
- EVM -> Dusk delivered 0.1 wDUSK, the relayer stopped, Dusk -> EVM queued
  0.1 wDUSK, the relayer restarted with the same config/DB, and final
  accounting returned to the starting state.

Two-cycle clean Rusk soak:

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
SOAK_CYCLES=2 \
TRANSFERS=2 \
TRANSFER_AMOUNT_WEI=100000000000000000 \
TIMEOUT_SECS=300 \
bash demo/e2e-soak-restart-stress.sh
```

- Passed on the same clean detached Rusk commit
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Completed 2 full restart/backlog cycles in 557 seconds.
- Each cycle delivered 2 EVM -> Dusk transfers, stopped the relayer, queued
  2 Dusk -> EVM transfers, restarted the relayer with the same config/DB, and
  returned EVM and Dusk accounting to the starting state.

Three-cycle/high-volume clean Rusk soak:

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
SOAK_CYCLES=3 \
TRANSFERS=20 \
TRANSFER_AMOUNT_WEI=500000000000000000 \
TIMEOUT_SECS=600 \
bash demo/e2e-soak-restart-stress.sh
```

- Passed on the same clean detached Rusk commit
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Completed 3 full restart/backlog cycles in 3102 seconds.
- Each cycle delivered 20 EVM -> Dusk transfers, stopped the relayer, queued
  20 Dusk -> EVM transfers, restarted the relayer with the same config/DB, and
  returned EVM and Dusk accounting to the starting state.
- The EVM -> Dusk bursts encountered transient Dusk `spendId exists in the
  mempool` preverify responses under load, and relayer retry/backoff recovered
  inside the configured checkpoints.

Artifacts:

- `/tmp/hyperlane-restart-stress-start-testMock-1778518816.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778518816.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778518816.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778518816.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778518816/`
- `/tmp/hyperlane-restart-stress-start-testMock-1778521340.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778521340.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778521340.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778521340.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778521340/`
- `/tmp/hyperlane-restart-stress-start-testMock-1778524643.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778524643.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778524643.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778524643.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778524643/`
- `/tmp/hyperlane-soak-restart-stress-1778528713.log`
- `/tmp/hyperlane-soak-restart-stress-1778528713-cycle-1.log`
- `/tmp/hyperlane-restart-stress-start-testMock-1778528713.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778528713.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778528713.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778528713.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778528713/`
- `/tmp/hyperlane-soak-restart-stress-1778529143.log`
- `/tmp/hyperlane-soak-restart-stress-1778529143-cycle-1.log`
- `/tmp/hyperlane-soak-restart-stress-1778529143-cycle-2.log`
- `/tmp/hyperlane-restart-stress-start-testMock-1778529143.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778529143.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778529143.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778529143.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778529143/`
- `/tmp/hyperlane-restart-stress-start-testMock-1778529420.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778529420.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778529420.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778529420.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778529420/`
- `/tmp/hyperlane-soak-restart-stress-1778530903.log`
- `/tmp/hyperlane-soak-restart-stress-1778530903-cycle-1.log`
- `/tmp/hyperlane-soak-restart-stress-1778530903-cycle-2.log`
- `/tmp/hyperlane-soak-restart-stress-1778530903-cycle-3.log`
- `/tmp/hyperlane-restart-stress-start-testMock-1778530903.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778530903.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778530903.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778530903.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778530903/`
- `/tmp/hyperlane-restart-stress-start-testMock-1778531937.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778531937.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778531937.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778531937.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778531937/`
- `/tmp/hyperlane-restart-stress-start-testMock-1778532971.log`
- `/tmp/hyperlane-restart-stress-deploy-testMock-1778532971.log`
- `/tmp/hyperlane-restart-stress-relayer-a-testMock-1778532971.log`
- `/tmp/hyperlane-restart-stress-relayer-b-testMock-1778532971.log`
- `/tmp/hyperlane-restart-stress-dusk-transfers-testMock-1778532971/`

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
- Clean Rusk rerun:
  `/tmp/hyperlane-validator-delay-start-messageIdMultisig-1778522740.log`,
  `/tmp/hyperlane-validator-delay-deploy-messageIdMultisig-1778522740.log`,
  `/tmp/hyperlane-validator-delay-relayer-messageIdMultisig-1778522740.log`,
  `/tmp/hyperlane-validator-delay-validator-messageIdMultisig-1778522740.log`

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
- Clean Rusk rerun:
  `/tmp/hyperlane-corrupt-metadata-start-messageIdMultisig-1778523252.log`,
  `/tmp/hyperlane-corrupt-metadata-deploy-messageIdMultisig-1778523252.log`,
  `/tmp/hyperlane-corrupt-metadata-relayer-messageIdMultisig-1778523252.log`,
  `/tmp/hyperlane-corrupt-metadata-validator-messageIdMultisig-1778523252.log`,
  `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778523252/0_with_id.json`

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
- Clean Rusk rerun:
  `/tmp/hyperlane-low-signer-start-testMock-1778523531.log`,
  `/tmp/hyperlane-low-signer-deploy-testMock-1778523531.log`,
  `/tmp/hyperlane-low-signer-relayer-low-testMock-1778523531.log`,
  `/tmp/hyperlane-low-signer-relayer-funded-testMock-1778523531.log`

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
- Clean Rusk rerun:
  `/tmp/hyperlane-rpc-failure-start-testMock-1778523772.log`,
  `/tmp/hyperlane-rpc-failure-deploy-testMock-1778523772.log`,
  `/tmp/hyperlane-rpc-failure-relayer-bad-rpc-testMock-1778523772.log`,
  `/tmp/hyperlane-rpc-failure-relayer-healthy-testMock-1778523772.log`

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
- Clean Rusk rerun:
  `/tmp/hyperlane-destination-rpc-failure-start-testMock-1778524014.log`,
  `/tmp/hyperlane-destination-rpc-failure-deploy-testMock-1778524014.log`,
  `/tmp/hyperlane-destination-rpc-failure-relayer-bad-rpc-testMock-1778524014.log`,
  `/tmp/hyperlane-destination-rpc-failure-relayer-healthy-testMock-1778524014.log`

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
- Clean Rusk rerun:
  `/tmp/hyperlane-duplicate-relayer-start-testMock-1778524257.log`,
  `/tmp/hyperlane-duplicate-relayer-deploy-testMock-1778524257.log`,
  `/tmp/hyperlane-duplicate-relayer-a-testMock-1778524257.log`,
  `/tmp/hyperlane-duplicate-relayer-b-testMock-1778524257.log`

## Rusk Checkout Reproduction Caveat

Most local agent E2E evidence in this report used:

- Rusk checkout: `/home/hein_/projects/rusk-private`
- Branch: `hein/boreas-wallet-transfer-gas-50m`
- Base commit: `c0c64db4659500d077bb253ad13acba0e347d3fc`

That checkout was not clean during the earlier E2E runs. At the time of the
latest dirty-checkout TestMock rerun, `git diff --name-only` reported 49
modified tracked files and `git ls-files --others --exclude-standard` reported
14263 untracked paths.
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
earlier E2E evidence should be treated as **dirty-Rusk evidence**.

A clean detached worktree at the same base commit has since passed the TestMock
and MessageIdMultisig E2E paths plus the 50-transfer relayer restart/backlog
stress test and the documented fault-injection scenarios with a regenerated
genesis state. This suggests the baseline bidirectional bridge,
validator/relayer multisig path, restart/backlog stress path, and currently
documented recovery/failure modes do not depend on the dirty local Rusk patch.

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
  native/collateral pending-transfer claims, and WarpDrc20 transfer/mint/burn
  accounting.
- Added tests for WarpDrc20 owner/non-owner admin resolution, WarpNative
  zero-amount remote sends, and WarpDrc20Collateral unlock attempts without
  sufficient locked wrapped-token balance.
- Added WarpDrc20Collateral escrow tests for unregistered recipients and
  post-registration pending-claim release.
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
  `67 passed; 0 failed; 0 ignored`.

## Remaining Work Before Production Readiness

- Continue expanding negative/security coverage; current coverage includes
  malformed mailbox messages, wrong domains, duplicate delivery, invalid
  multisig metadata, insufficient signatures, unauthorized multisig admin
  paths, dirty redeploy refusal, and several token-accounting/cross-route
  failures, but broader route-matrix coverage remains useful.
- Continue stress and reliability testing. Current coverage includes dirty
  redeploy refusal, 50-message relayer burst delivery, relayer restart/backlog
  recovery, and delayed validator checkpoint recovery through relayer metadata
  backoff. No currently listed reliability scenario remains untested in this
  report. The currently documented E2E and fault-injection paths have also
  passed on a clean Rusk worktree. `demo/e2e-soak-restart-stress.sh` provides
  repeatable restart/backlog soak cycles and has passed 1-cycle, 2-cycle, and
  3-cycle/20-transfer clean-Rusk runs. An hours-long soak is still reasonable
  before production readiness, but the documented restart/backlog path now has
  a 3102-second high-volume run.
- Have Dusk reviewers accept or change the open production review decisions
  recorded in `SECURITY_REVIEW.md`.
- Production secret handling remains an operational gate. The local scripts use
  ignored dev configs and `/tmp` runtime artifacts, and Dusk consensus
  passwords are no longer passed through `dusk-tx` process argv. `make
  secret-hygiene` provides a source and artifact-scan guardrail, but production
  signer/key handling and CI artifact policy still need explicit review.
- Re-run the full report on a clean Rusk checkout or a dedicated Rusk branch
  containing the exact required changes if new Rusk-dependent scenarios are
  added. The current TestMock and MessageIdMultisig E2E paths, 50-transfer
  relayer restart/backlog stress test, and documented fault-injection scenarios
  now pass on a clean detached
  `c0c64db4659500d077bb253ad13acba0e347d3fc` worktree.
