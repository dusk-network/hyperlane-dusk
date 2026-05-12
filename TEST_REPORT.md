# Dusk Hyperlane Test Report

Date: 2026-05-11
Last updated: 2026-05-12

This report captures the current local verification for the revived Dusk
Hyperlane branches. It is not a production-readiness sign-off; the remaining
production-review gates and useful follow-up test areas are listed at the end.

## Repository State

| Component | Repository | Branch | Evidence commit |
|---|---|---|---|
| Dusk contracts/tooling | `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | Latest clean-layout repro run `1778599935` tested Dusk source ref `de9b7fa3c832fb60982d3dbd22c8a59114d37732`, monorepo `a2db5731e385634268071d39b0554883d11d8ac5`, and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc` |
| Hyperlane agent integration | `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | Latest clean-layout repro run `1778599935` tested monorepo `a2db5731e385634268071d39b0554883d11d8ac5`, with Dusk source ref `de9b7fa3c832fb60982d3dbd22c8a59114d37732` and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`. The branch was later rebased onto upstream `7a362a093d622b69d6c55d47992c9490ec33fb1a`; post-rebase targeted Rust agent check passed, and the diff from the clean-layout repro monorepo ref to the live PR head is Dusk upstream docs plus upstream TypeScript infra/config only |
| Review-head clean-layout repro and E2E evidence before this report update | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Dusk `2ac225175b15aac465d100e748ba68f8b14bd545`; monorepo `a44020dc998b7fe868254a5d1a349b9eb8ded899`; clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`; non-E2E repro run `1778586371`; TestMock E2E run `1778587094`; MessageIdMultisig E2E run `1778587351` |
| Supplemental clean-layout review-branch local repro | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Historical evidence: Dusk `06e9bd2c05607eb922ea476ea25feb334f0656a6`; monorepo `ecb11359747dce240a24c50fa229afd4479919b5` |
| Recorded clean-layout local repro evidence | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Historical evidence: Dusk `8e629da55e5e5a804d625ebb8b44173b4d96dab9`; monorepo `dea286bd364a9268413fd5b1cfc51bd983d443be` |
| Supplemental clean-layout repro after audit docs refresh | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Historical evidence: Dusk `24d42c3ed0aaa289f32a22f6a7bf7f7c068bc2ae`; monorepo `dea286bd364a9268413fd5b1cfc51bd983d443be` |
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
- A 2026-05-12 supplemental `make repro-check-agent` run passed on the Dusk
  and monorepo review-branch heads at the time of the run, using `RUSK_DIR` to
  point at the clean detached Rusk worktree. `scripts/local-repro-check.sh`
  created the temporary compatible layout automatically, so the non-E2E repro
  command no longer depends on the dirty local
  `/home/hein_/projects/rusk-private` checkout.
- A later 2026-05-12 review-head evidence pass tested Dusk
  `2ac225175b15aac465d100e748ba68f8b14bd545` and monorepo
  `a44020dc998b7fe868254a5d1a349b9eb8ded899` against the clean detached Rusk
  worktree. It passed the repeatable non-E2E repro command, the TestMock E2E
  path, and the MessageIdMultisig E2E path. The GitHub evidence comments are
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430201984
  and
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430343619.
- After upstream Hyperlane advanced to
  `66e8c1f4644cea0392b33007225e6611b8f06804`, the monorepo PR branch was
  rebased and pushed at `a2db5731e385634268071d39b0554883d11d8ac5`. The
  clean-layout repro command passed at that monorepo head, including Dusk WASM
  builds, targeted contract clippy, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check.
  The clean-Rusk E2E evidence above remains evidence for the pre-rebase
  monorepo ref `a44020dc998b7fe868254a5d1a349b9eb8ded899`; E2E was not rerun
  for the docs-only post-rebase head.
- After upstream Hyperlane advanced to
  `7a362a093d622b69d6c55d47992c9490ec33fb1a`, the monorepo PR branch was
  rebased again and pushed. The upstream change was TypeScript infra/config
  only (`typescript/infra/config/environments/mainnet3/agent.ts`), and the
  Dusk branch changes after the clean-layout repro monorepo ref are Dusk
  upstream compatibility/PR-plan docs. `git diff --check`, the Dusk agent
  placeholder scan, and the targeted Rust agent check passed after the rebase.
- The latest review-head repro/E2E logs and the 7282-second high-volume soak
  logs were copied from `/tmp` into durable local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-evidence-20260512T1245Z.tgz`.
  SHA256:
  `53bc99b30624471b145731b92896b442c3a88c97f8217c8a7cd12be4ab7476fc`.
  `scripts/secret-hygiene-check.sh` passed over both the archive staging
  directory and the tarball.
- The previous clean-layout repro log was copied into durable local
  handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-final-head-repro-20260512T144314Z.tgz`.
  SHA256:
  `e8c17cb43da130091a52b2c711aa64d0fef000b599997a372c968970c52dfa7e`.
  `scripts/secret-hygiene-check.sh` passed over both the archive staging
  directory and the tarball.
- The latest clean-layout repro for Dusk source ref `de9b7fa` was copied into
  durable local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778599935.tgz`.
  SHA256:
  `4ac0ba327024358bed5278fd276f796d71cbb6940c8417fdc76125cfedfdfa44`.
  `scripts/secret-hygiene-check.sh` passed over the repro log, archive staging
  directory, and tarball.

## Commands Run

### Contract and Type Checks

```bash
cd /home/hein_/projects/hyperlane/dusk
make all
make clippy-contracts
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
cargo test -p dusk-tx
make secret-hygiene
make gate-status
make repro-check-agent
bash scripts/local-repro-check.sh --agent-check
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-current-head-1778599935 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-current-head-1778599935.log
bash scripts/secret-hygiene-check.sh /tmp/hyperlane-relayer-testMock-1778530398.log
if bash scripts/secret-hygiene-check.sh /tmp/hyperlane-relayer-testMock-1778530398.json \
  >/tmp/hyperlane-secret-hygiene-negative.log 2>&1; then
  exit 1
fi
gh api repos/dusk-network/hyperlane-dusk/pulls/1 --jq .body \
  >/tmp/hyperlane-gh-review-text-1778551437/dusk-pr-1-body.txt
gh api repos/dusk-network/hyperlane-monorepo/pulls/1 --jq .body \
  >/tmp/hyperlane-gh-review-text-1778551437/monorepo-pr-1-body.txt
gh api repos/dusk-network/hyperlane-dusk/issues/2 --jq .body \
  >/tmp/hyperlane-gh-review-text-1778551437/dusk-issue-2-body.txt
gh api repos/dusk-network/hyperlane-dusk/issues/1/comments --paginate --jq '.[].body' \
  >/tmp/hyperlane-gh-review-text-1778551437/dusk-pr-1-comments.txt
gh api repos/dusk-network/hyperlane-monorepo/issues/1/comments --paginate --jq '.[].body' \
  >/tmp/hyperlane-gh-review-text-1778551437/monorepo-pr-1-comments.txt
gh api repos/dusk-network/hyperlane-dusk/issues/2/comments --paginate --jq '.[].body' \
  >/tmp/hyperlane-gh-review-text-1778551437/dusk-issue-2-comments.txt
bash scripts/secret-hygiene-check.sh /tmp/hyperlane-gh-review-text-1778551437

gh api repos/dusk-network/hyperlane-dusk/pulls/1 --jq .body \
  >/tmp/hyperlane-gh-review-text-audit-1778551686/dusk-pr-1-body.txt
gh api repos/dusk-network/hyperlane-monorepo/pulls/1 --jq .body \
  >/tmp/hyperlane-gh-review-text-audit-1778551686/monorepo-pr-1-body.txt
gh api repos/dusk-network/hyperlane-dusk/issues/2 --jq .body \
  >/tmp/hyperlane-gh-review-text-audit-1778551686/dusk-issue-2-body.txt
gh api repos/dusk-network/hyperlane-dusk/issues/1/comments --paginate --jq '.[].body' \
  >/tmp/hyperlane-gh-review-text-audit-1778551686/dusk-pr-1-comments.txt
gh api repos/dusk-network/hyperlane-monorepo/issues/1/comments --paginate --jq '.[].body' \
  >/tmp/hyperlane-gh-review-text-audit-1778551686/monorepo-pr-1-comments.txt
gh api repos/dusk-network/hyperlane-dusk/issues/2/comments --paginate --jq '.[].body' \
  >/tmp/hyperlane-gh-review-text-audit-1778551686/dusk-issue-2-comments.txt
rg -n -e 'Current-head' -e 'current-head' -e '1778551243' \
  /tmp/hyperlane-gh-review-text-audit-1778551686 || true
bash scripts/secret-hygiene-check.sh /tmp/hyperlane-gh-review-text-audit-1778551686
bash -n demo/e2e-agents.sh demo/e2e-low-dusk-signer-balance.sh \
  demo/e2e-origin-rpc-failure.sh demo/e2e-destination-rpc-failure.sh \
  demo/e2e-duplicate-relayer-attempt.sh demo/e2e-validator-delay.sh \
  demo/e2e-corrupt-checkpoint-metadata.sh demo/e2e-relayer-restart-stress.sh
```

Result:

- `make all`: passed, all contract WASMs built.
- `make clippy-contracts`: passed for `hyperlane-dusk-types` and the production contract crates:
  Mailbox, MerkleTreeHook, MessageIdMultisigISM, ValidatorAnnounce,
  ProtocolFee, IGP, WarpDrc20, WarpDrc20Collateral, and WarpNative. The broader
  workspace wasm clippy command is intentionally not used because non-contract
  host/test dependencies pull wasm-unsupported `getrandom` paths before
  reaching the contract surface.
- `cargo test -p hyperlane-dusk-types`: passed, 28 tests.
- `cargo test -p hyperlane-dusk-integration-tests`: passed, 70 tests in the
  latest run. Earlier runs in this section passed 67 tests before the Mailbox
  fee-overflow and fee-accounting overflow regressions were added.
- `cargo test -p dusk-tx`: passed, 3 tests.
- `make secret-hygiene`: passed.
- `make gate-status`: passed; reported the implementation PRs and manual
  workflow dispatcher PR, zero status checks on the implementation PRs,
  7 unchecked production sign-off items, all six split decision issues open,
  active-repo untracked source status, default-branch protection and merge
  method settings, no visible GitHub workflow, repo-level Actions secret and
  self-hosted runner visibility for CI gate #8, current Hyperlane upstream
  drift, and no placeholder matches in
  `rust/main/chains/hyperlane-dusk`.
- `make repro-check-agent`: added as a Makefile wrapper for the full local
  non-E2E repro command, including the Hyperlane Rust agent check. Latest
  current-head run `1778599935` passed against Dusk source ref
  `de9b7fa3c832fb60982d3dbd22c8a59114d37732`, monorepo
  `a2db5731e385634268071d39b0554883d11d8ac5`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- `bash scripts/local-repro-check.sh --agent-check`: passed. This wraps the
  same repeatable non-E2E checks plus the Hyperlane Rust agent check. It still
  requires local/private Rusk path dependencies and does not replace the
  E2E/fault-injection runs below.
- `.github/workflows/manual-repro-check.yml`: added after the local repro run as
  a manual self-hosted workflow template for the same `make repro-check-agent`
  command. It requires Dusk to provide a `dusk-hyperlane` self-hosted runner and
  `DUSK_ORG_READ_TOKEN` secret for private cross-repo checkout. The manual
  inputs include `dusk_ref`, `rusk_ref`, and `monorepo_ref` so reviewers can run
  the workflow from the default branch against exact review heads.
  `CI_REPRO_STRATEGY.md` includes the `gh workflow run` command shape and says
  to record both requested refs and resolved heads for release evidence. The
  workflow logs the resolved checkout heads before running the repro command.
- dusk-network/hyperlane-dusk#3: opened as a narrow default-branch dispatcher
  PR containing only `.github/workflows/manual-repro-check.yml` and
  `.github/actionlint.yaml`, so Dusk can make the manual workflow visible
  independently from the full implementation PR.
- `actionlint .github/workflows/manual-repro-check.yml`: passed after adding
  `.github/actionlint.yaml` for the custom self-hosted `dusk-hyperlane` label.
- `make secret-hygiene`: re-run after manual workflow secret-scope docs and
  checkout credential hardening; passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-relayer-testMock-1778530398.log`:
  passed.
- Negative artifact scan against `/tmp/hyperlane-relayer-testMock-1778530398.json`:
  failed as expected after detecting generated signer config material. Current
  generated configs use `duskKey.keyFile` for the Dusk side, while the local EVM
  side still uses Anvil `hexKey` material.
- Exported GitHub-facing PR and sign-off tracker text under
  `/tmp/hyperlane-gh-review-text-1778551437` and scanned it with
  `scripts/secret-hygiene-check.sh`: passed after removing literal password
  flag text from PR body prose and correcting tested-SHA wording for repro `1778550420`.
- Re-exported GitHub-facing text under
  `/tmp/hyperlane-gh-review-text-audit-1778551686` after editing older comments
  to avoid stale "current-head" wording and the obsolete export path
  `1778551243`; stale-wording scan found no matches and
  `scripts/secret-hygiene-check.sh` passed.
- Re-exported the current GitHub-facing PR and decision issue text under
  `/tmp/hyperlane-gh-review-text-current-1778556292` after adding consolidated
  review-map links to the PR bodies and focused decision evidence comments to
  issues #4 through #9. The export included Dusk PR #1, monorepo PR #1,
  workflow PR #3, umbrella issue #2, split issues #4-#9, and their comments.
  `scripts/secret-hygiene-check.sh` passed for the exported directory.
- `make review-hygiene`: repeatable wrapper around
  `scripts/github-review-hygiene.sh`; passed with export directory
  `/tmp/hyperlane-review-export-1778603374`. It exports Dusk PR #1,
  workflow PR #3, monorepo PR #1, sign-off issue #2, split issues #4-#9, and
  their comments, scans for known stale evidence refs/wording, validates
  explicit current-head claims for Dusk PR #1, monorepo PR #1, and workflow
  PR #3 against live GitHub PR heads while ignoring comments explicitly marked
  Historical or Superseded, rejects active stale clean-layout evidence pointers
  to the superseded `4431719578` comment and Dusk source ref `6ac9a2bc`, requires
  old status-snapshot comments to be marked superseded, and then runs
  `scripts/secret-hygiene-check.sh` over the export. The latest guard updates
  were committed as `3296586` and `76ca8ce`; after editing the PR evidence note
  to avoid pinning a moving PR head, the wrapper was rerun and passed against
  the updated GitHub review surface.
- E2E wrappers that call `demo/gen-agent-configs.sh` now track generated Dusk
  signer key files and remove them on exit after stopping agents. `bash -n`
  passed for the touched E2E scripts listed above. `make secret-hygiene` now
  fails if a future E2E wrapper calls `gen-agent-configs.sh` without both the
  cleanup tracking variable and the generated-key `rm -f` cleanup call.
- Review-head clean-layout repro run `1778586371` passed against Dusk
  `2ac225175b15aac465d100e748ba68f8b14bd545`, monorepo
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-head-1778586371.log`.
- Review-head clean-Rusk E2E runs passed against the same Dusk, monorepo, and
  Rusk refs. TestMock run `1778587094` passed EVM -> Dusk and Dusk -> EVM
  through relayer-driven delivery. MessageIdMultisig run `1778587351` started
  the validator, consumed checkpoint metadata through the relayer path, and
  passed both directions. Generated Dusk signer key files were removed on exit.
  A secret-hygiene scan over the safe text logs passed.
- Previous clean-layout repro run `1778596710` passed against Dusk source ref
  `6ac9a2bc0c3cbdb1d9994335b23f06189d355f40`, monorepo
  `a2db5731e385634268071d39b0554883d11d8ac5`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-final-head-1778596710.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-final-head-repro-20260512T144314Z.tgz`.
- Current-head clean-layout repro run `1778599935` passed against Dusk source ref
  `de9b7fa3c832fb60982d3dbd22c8a59114d37732`, monorepo
  `a2db5731e385634268071d39b0554883d11d8ac5`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-head-1778599935.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778599935.tgz`.
- E2E freshness check on 2026-05-12: the Dusk diff from the review-head
  clean-Rusk E2E source ref `2ac225175b15aac465d100e748ba68f8b14bd545` to
  the latest tested source ref `de9b7fa3c832fb60982d3dbd22c8a59114d37732`
  touched only docs, workflow, reference, and hygiene-script files. The
  monorepo diff from the review-head clean-Rusk E2E ref
  `a44020dc998b7fe868254a5d1a349b9eb8ded899` to the live monorepo PR head
  touched only Dusk upstream-plan docs and upstream TypeScript infra/config
  files. No Dusk contract, Dusk type, Dusk transaction, or Rust Dusk agent
  runtime files changed after the review-head clean-Rusk E2E runs.

### Supplemental Clean-Layout Review-Branch Local Repro

```bash
cd /home/hein_/projects/hyperlane/dusk
run_dir=/tmp/hyperlane-dusk-repro-rusk-dir-1778540326
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-rusk-dir-current-1778541170
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-keyfile-1778549721
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-keyperms-1778550420
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778552618
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778558278
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778559809
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778560831
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778570601
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778572385
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778574482
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778576530
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778577847
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"

run_dir=/tmp/hyperlane-dusk-repro-current-1778582787
HYPERLANE_DUSK_REPRO_WORKDIR="$run_dir" \
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
make repro-check-agent 2>&1 | tee "$run_dir.log"
```

Refs:

- Earlier run Dusk PR branch:
  `b0fdfffd1cdb5e7ee77809be52c450500216c12c`
- Latest run Dusk PR branch:
  `ba6c02d06b22705daa22ae513924eeb9c2767a79`
- File-backed signer run Dusk PR branch:
  `7de48ea3897d6d7956cedc9c3f32fc7062cf39c6`
- Key-file permission enforcement run Dusk PR branch:
  `06e9bd2c05607eb922ea476ea25feb334f0656a6`
- Earlier clean-layout repro tested Dusk PR branch:
  `fc9ed45f9661a843d053ebddcc89666ef187e5c2`
- Previous clean-layout repro tested Dusk PR branch:
  `efb6fd80bf199ef15c88ceb49b0fe23a25c12271`
- Clean-layout repro tested Dusk PR branch:
  `8e629da55e5e5a804d625ebb8b44173b4d96dab9`
- Supplemental clean-layout repro tested Dusk PR branch:
  `24d42c3ed0aaa289f32a22f6a7bf7f7c068bc2ae`
- Mailbox fee-overflow repro tested Dusk PR branch:
  `b4ace8073147f17f67593c3a471d8bf33ca8dce3`
- Mailbox nonce-overflow repro tested Dusk PR branch:
  `17e9b6219239d2ffa72a0de0c019c5476d7f9b70`
- Fee-accounting overflow repro tested Dusk PR branch:
  `28d07e01d1bbc0cf59575a811cde55e844a2abb7`
- Wasm clippy coverage repro tested Dusk PR branch:
  `c0036501b26cc98fb64259807e4cbca929487aec`
- Repeatable clippy wrapper repro tested Dusk PR branch:
  `06dbf75e2d67b0bbc5aa450066bfb5743f79bdd2`
- Historical clean-layout repro tested Dusk PR branch:
  `889a00bb11d589d268ee928d0855e3724dfab0fe`
- Current-head clean-layout repro tested Dusk PR branch:
  `de9b7fa3c832fb60982d3dbd22c8a59114d37732`
- Earlier Hyperlane monorepo branch:
  `09e32b7c2f04503b75b3527e0f8c6f5a6c8e42a2`
- File-backed signer run Hyperlane monorepo branch:
  `760efedeb2d93729d853d5f577be89e27ea8f22d`
- Key-file permission enforcement run Hyperlane monorepo branch:
  `ecb11359747dce240a24c50fa229afd4479919b5`
- Earlier clean-layout repro tested Hyperlane monorepo branch:
  `ecb11359747dce240a24c50fa229afd4479919b5`
- Earlier clean-layout repro tested Hyperlane monorepo branch:
  `dea286bd364a9268413fd5b1cfc51bd983d443be`
- Mailbox fee-overflow repro tested Hyperlane monorepo branch:
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`
- Mailbox nonce-overflow repro tested Hyperlane monorepo branch:
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`
- Fee-accounting overflow repro tested Hyperlane monorepo branch:
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`
- Wasm clippy coverage repro tested Hyperlane monorepo branch:
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`
- Repeatable clippy wrapper repro tested Hyperlane monorepo branch:
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`
- Historical clean-layout repro tested Hyperlane monorepo branch:
  `a44020dc998b7fe868254a5d1a349b9eb8ded899`
- Current-head clean-layout repro tested Hyperlane monorepo branch:
  `a2db5731e385634268071d39b0554883d11d8ac5`
- Earlier run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-rusk-dir-1778540326/rusk-private`, symlinked to clean
  detached worktree `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db`
  at `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Earlier run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-rusk-dir-current-1778541170/rusk-private`,
  symlinked to clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Latest run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-current-head-1778599935/rusk-private`, symlinked
  to clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- File-backed signer run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-keyfile-1778549721/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Key-file permission enforcement run Rusk path dependency checkout in the
  temporary layout:
  `/tmp/hyperlane-dusk-repro-keyperms-1778550420/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Earlier clean-layout repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778552618/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Previous clean-layout repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778558278/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Clean-layout repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778559809/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Supplemental clean-layout repro Rusk path dependency checkout in the
  temporary layout:
  `/tmp/hyperlane-dusk-repro-current-1778560831/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Mailbox fee-overflow repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778570601/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Mailbox nonce-overflow repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778572385/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Fee-accounting overflow repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778574482/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Wasm clippy coverage repro Rusk path dependency checkout in the temporary
  layout:
  `/tmp/hyperlane-dusk-repro-current-1778576530/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Repeatable clippy wrapper repro Rusk path dependency checkout in the
  temporary layout:
  `/tmp/hyperlane-dusk-repro-current-1778577847/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Current-head clean-layout repro Rusk path dependency checkout in the
  temporary layout:
  `/tmp/hyperlane-dusk-repro-current-1778582787/rusk-private`, symlinked to
  clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Logs:
  `/tmp/hyperlane-dusk-repro-rusk-dir-1778540326.log`,
  `/tmp/hyperlane-dusk-repro-rusk-dir-current-1778541170.log`,
  `/tmp/hyperlane-dusk-repro-keyfile-1778549721.log`,
  `/tmp/hyperlane-dusk-repro-keyperms-1778550420.log`,
  `/tmp/hyperlane-dusk-repro-current-1778552618.log`,
  `/tmp/hyperlane-dusk-repro-current-1778558278.log`,
  `/tmp/hyperlane-dusk-repro-current-1778559809.log`,
  `/tmp/hyperlane-dusk-repro-current-1778560831.log`,
  `/tmp/hyperlane-dusk-repro-current-1778570601.log`,
  `/tmp/hyperlane-dusk-repro-current-1778572385.log`,
  `/tmp/hyperlane-dusk-repro-current-1778574482.log`,
  `/tmp/hyperlane-dusk-repro-current-1778576530.log`,
  `/tmp/hyperlane-dusk-repro-current-1778577847.log`,
  `/tmp/hyperlane-dusk-repro-current-1778582787.log`

Result:

- Passed in all listed successful runs.
- Contract WASM build via `make all`: passed.
- `cargo test -p hyperlane-dusk-types`: passed, 28 tests.
- `cargo test -p hyperlane-dusk-integration-tests`: passed, 70 tests in the
  latest run; earlier runs passed 67 tests before the Mailbox fee-overflow
  and fee-accounting overflow regressions were added.
- `cargo test -p dusk-tx`: passed, 3 tests.
- `make secret-hygiene`: passed.
- `make repro-check-agent`: passed after the wrapper was updated to include
  `make clippy-contracts`.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-rusk-dir-current-1778541170.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-keyfile-1778549721.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-keyperms-1778550420.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778552618.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778558278.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778559809.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778560831.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778570601.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778572385.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778574482.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778576530.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778577847.log`:
  passed.
- `bash scripts/secret-hygiene-check.sh /tmp/hyperlane-dusk-repro-current-1778582787.log`:
  passed.
- Hyperlane Rust agent check from the adjacent monorepo passed:

```bash
cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
```

Caveat:

- This is still a local non-E2E repro command, not a replacement for the live
  clean-Rusk E2E/stress runs below or the pending Dusk CI/runner decision.
- The latest monorepo run includes Unix `duskKey.keyFile` regular-file and
  group/world-permission rejection before reading key material.

### Hyperlane Rust Agent Checks

```bash
cd /home/hein_/projects/hyperlane/hyperlane-monorepo/rust/main
cargo test -p hyperlane-base dusk
cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander
```

Result:

- `cargo test -p hyperlane-base dusk`: passed, 4 focused signer/parser tests.
- Passed. Re-run after Dusk event/type changes also passed.
- Passed again after rebasing `feat/dusk-support-v2` onto current upstream
  Hyperlane `main` at `66e8c1f4644cea0392b33007225e6611b8f06804`; post-rebase
  head `a2db5731e385634268071d39b0554883d11d8ac5`.
- Passed again after rebasing `feat/dusk-support-v2` onto upstream Hyperlane
  `main` at `7a362a093d622b69d6c55d47992c9490ec33fb1a`; post-rebase head
  `09e62be2e55ecd87d3931f6f10623086befd7848`.
- Passed after adding file/env-backed `duskKey` signer sources.

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

### Review-Head Clean Rusk E2E: TestMock ISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
bash demo/e2e-agents.sh --only testMock --timeout 300 \
  2>&1 | tee /tmp/hyperlane-current-head-e2e-testMock-1778587094.outer.log
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Tested Dusk repo ref `2ac225175b15aac465d100e748ba68f8b14bd545`.
- Tested monorepo ref `a44020dc998b7fe868254a5d1a349b9eb8ded899`.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.
- Generated Dusk signer key file was removed on script exit.

Artifacts:

- `/tmp/hyperlane-current-head-e2e-testMock-1778587094.outer.log`
- `/tmp/hyperlane-start-env-testMock-1778587094.log`
- `/tmp/hyperlane-deploy-testMock-1778587094.log`
- `/tmp/hyperlane-relayer-testMock-1778587094.log`
- `/tmp/hyperlane-db-relayer-testMock-1778587094/`
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
  after switching demo `dusk-tx` invocations from CLI password-flag handling
  to `DUSK_CONSENSUS_PASSWORD`.
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

### Review-Head Clean Rusk E2E: MessageIdMultisigISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
bash demo/e2e-agents.sh --only messageIdMultisig --timeout 300 \
  2>&1 | tee /tmp/hyperlane-current-head-e2e-messageIdMultisig-1778587351.outer.log
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Tested Dusk repo ref `2ac225175b15aac465d100e748ba68f8b14bd545`.
- Tested monorepo ref `a44020dc998b7fe868254a5d1a349b9eb8ded899`.
- Validator and relayer started successfully.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.
- Generated Dusk signer key file was removed on script exit.

Artifacts:

- `/tmp/hyperlane-current-head-e2e-messageIdMultisig-1778587351.outer.log`
- `/tmp/hyperlane-start-env-messageIdMultisig-1778587351.log`
- `/tmp/hyperlane-deploy-messageIdMultisig-1778587351.log`
- `/tmp/hyperlane-validator-messageIdMultisig-1778587351.log`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778587351.log`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778587351/index.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778587351/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778587351/announcement.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778587351/metadata_latest.json`
- `/tmp/hyperlane-db-relayer-messageIdMultisig-1778587351/`
- `/tmp/hyperlane-db-validator-anvil-messageIdMultisig-1778587351/`
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

Two-hour/high-volume clean Rusk soak:

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
SKIP_OTTERSCAN=true \
SKIP_DUSK_EXPLORER=true \
SOAK_CYCLES=8 \
SOAK_MINUTES=120 \
TRANSFERS=20 \
TRANSFER_AMOUNT_WEI=500000000000000000 \
TIMEOUT_SECS=600 \
bash demo/e2e-soak-restart-stress.sh
```

- Passed on the same clean detached Rusk commit
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Completed 7 full restart/backlog cycles in 7282 seconds.
- The wrapper stopped before cycle 8 because the 120-minute time budget had
  been reached.
- Each completed cycle delivered 20 EVM -> Dusk transfers, stopped the relayer,
  queued 20 Dusk -> EVM transfers, restarted the relayer with the same
  config/DB, and returned EVM and Dusk accounting to the starting state.
- Total completed transfers: 280.
- Artifact hygiene scan passed for the outer log, summary log, and per-cycle
  logs:

```bash
bash scripts/secret-hygiene-check.sh \
  /tmp/hyperlane-soak-hours-1778541618.outer.log \
  /tmp/hyperlane-soak-restart-stress-1778541618.log \
  /tmp/hyperlane-soak-restart-stress-1778541618-cycle-*.log
```

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
- `/tmp/hyperlane-soak-hours-1778541618.outer.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-1.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-2.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-3.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-4.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-5.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-6.log`
- `/tmp/hyperlane-soak-restart-stress-1778541618-cycle-7.log`

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
- Changed Mailbox `quote_dispatch` to reject combined required-hook plus
  default/custom-hook fee overflow instead of returning a wrapped `u64`; added
  `test_mailbox_quote_dispatch_rejects_fee_overflow`.
- Changed Mailbox dispatch nonce increment to `checked_add` so release WASM
  rejects nonce exhaustion instead of wrapping the fixed-width Hyperlane nonce.
- Changed ProtocolFee and IGP lifetime accounting counters from saturating to
  checked additions so impossible totals reject instead of silently pinning at
  `u64::MAX`; added `test_protocol_fee_rejects_collected_fee_overflow` and
  `test_igp_rejects_total_gas_payment_overflow`.
- Changed Mailbox `processed_count` and IGP `gas_payment_count` to use checked
  `u32` conversions instead of truncating query lengths.
- Confirmed the workspace release profile sets `overflow-checks = true` in
  `Cargo.toml`; the explicit checked arithmetic above is kept so the safety
  invariants do not depend only on a build-profile setting.

Event annotation verification:

```bash
rg -n "todo!|unimplemented!|panic!" contracts types data-driver dusk-tx e2e wasm-bindings demo -g '!target'
make all
make clippy-contracts
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
```

Result:

- No `todo!`, `unimplemented!`, or direct `panic!` invocations remain in the
  scanned production contract/runtime/tooling paths.
- `make all` passed for all contract WASM builds after adding explicit event
  annotations.
- Targeted wasm clippy passed for the production contract/type surface listed
  above.
- `cargo test -p hyperlane-dusk-types` passed:
  `28 passed; 0 failed; 0 ignored`.
- `cargo test -p hyperlane-dusk-integration-tests` passed:
  `70 passed; 0 failed; 0 ignored`.

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
  repeatable restart/backlog soak cycles and has passed 1-cycle, 2-cycle,
  3-cycle/20-transfer, and 7282-second/7-cycle/280-transfer clean-Rusk runs.
  Dusk reviewers should decide whether that hours-long soak evidence is enough
  for the intended release gate.
- Have Dusk reviewers accept or change the open production review decisions
  recorded in `SECURITY_REVIEW.md`.
- Production secret handling remains an operational gate. The local scripts use
  ignored dev configs and `/tmp` runtime artifacts, and Dusk consensus
  passwords are no longer passed through `dusk-tx` process argv. `make
  secret-hygiene` provides a source and artifact-scan guardrail, but production
  signer/key handling and CI artifact policy still need explicit review.
- Re-run the full report on a clean Rusk checkout or a dedicated Rusk branch
  containing the exact required changes if new Rusk-dependent scenarios are
  added. The review-head TestMock and MessageIdMultisig E2E paths listed above,
  50-transfer relayer restart/backlog stress test, and documented
  fault-injection scenarios pass on a clean detached
  `c0c64db4659500d077bb253ad13acba0e347d3fc` worktree.
