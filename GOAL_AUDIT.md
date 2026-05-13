# Dusk Hyperlane Revival Goal Audit

Date: 2026-05-13

This audit maps the original revival/hardening goal to concrete artifacts and
current gates. It is intentionally not a production-readiness sign-off. Items
marked `Gated` need Dusk reviewer acceptance or additional release evidence
before upstream submission or production claims.

## Completion Audit Summary

Objective restated as concrete success criteria:

1. Preserve the existing local prototype before any rebase or cleanup.
2. Put all Dusk-specific source under Dusk org GitHub repositories.
3. Re-port the Hyperlane agent integration on top of current upstream
   Hyperlane `main` without developing on `hyperlane-xyz` directly.
4. Harden the Dusk contracts against the known Dusk/OpenZeppelin-equivalent and
   DuskEVM audit patterns.
5. Prove contract, agent, bidirectional bridge, fault-injection, and stress
   behavior with repeatable commands and artifact paths.
6. Prepare internal Dusk PRs and stop before upstream Hyperlane PRs until Dusk
   review/sign-off gates are closed.

Audit result: criteria 1-5 have concrete repository, command, and artifact
evidence. Criterion 6 has internal review PRs prepared, but the overall goal is
not complete because internal Dusk review, production signer/CI policy, CI
runner strategy, and soak acceptance are still open in
`dusk-network/hyperlane-dusk#2` and split decision issues #4 through #9.

## Branches And Evidence Commits

| Component | Branch | Evidence | State |
|---|---|---|---|
| `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | Live PR head is checked through GitHub and `make gate-status`; latest clean-layout `make repro-check-agent` evidence is run `1778671118`, which tested Dusk source ref `93ed3b07b26d8784610d2ba754a851490de15e21`, monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`. Previous clean-layout evidence run `1778669495` tested Dusk source ref `0be9fbfa91ef39ecd912c79d360d036530fb524d`, monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base `2b7db706023806b36a57e446205ae443537ae9ec`, and the same clean Rusk ref. Earlier clean-layout evidence run `1778615349` tested Dusk source ref `016eaa89e1afce0ef9a7534fe285d9aa16e26183`, monorepo `006e49dd7041097384683a78b1c1973c83e90de8`, upstream base `c6bce706316206ac7b5652155c9ea92e96f78c39`, and the same clean Rusk ref. Post-rebase clean-Rusk E2E evidence passed at Dusk `b1ccdc9d1e7797bba4939405200aa4cc5aff2ea8`, monorepo `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`, and the same upstream/Rusk refs: TestMock run `1778609411` and MessageIdMultisig run `1778609697`. After Rust dependency advisory remediation, focused checks run `1778613424` and clean-Rusk E2E runs `1778613709` and `1778613956` passed on the updated `Cargo.toml`/`Cargo.lock` graph with the same monorepo/upstream/Rusk refs. Earlier clean-layout evidence run `1778604592` tested Dusk source ref `aa278208b2c2b5f4abc38c32ec792295080014a9`, monorepo `09e62be2e55ecd87d3931f6f10623086befd7848`, and the same clean Rusk ref. Earlier review-head evidence tested Dusk `2ac225175b15aac465d100e748ba68f8b14bd545`, monorepo `a44020dc998b7fe868254a5d1a349b9eb8ded899`, and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`: non-E2E repro run `1778586371`, TestMock E2E run `1778587094`, and MessageIdMultisig E2E run `1778587351`. Earlier commits cover implementation, cleanup regression guards, audit, CI/repro, signer, default-branch, cross-repo handoff docs, generated signer key cleanup, gate-status reporting, split decision routing, reviewer evidence links, Mailbox fee-overflow hardening, Mailbox nonce-overflow hardening, fee-accounting overflow hardening, Dusk-side RUES client error propagation, targeted wasm clippy evidence for the production contract/type surface, repeatable `make clippy-contracts` wrapper coverage, and repeatable GitHub review-hygiene guardrails including live PR-head validation, stale clean-layout evidence detection, and Historical/Superseded review-comment filtering | Internal PR ready for review, open, blocked on review/sign-off; review requested from `moCello`; labels `need:feedback`, `type:feature`; `Dusk review policy gate` passing and required by default-branch protection; `Production readiness guard` failing as expected while production blockers remain |
| `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | Live PR head is checked through GitHub and `make gate-status`; upstream freshness is checked with `git fetch upstream main`, `git merge-base HEAD upstream/main`, and `git rev-list --left-right --count HEAD...upstream/main`; Dusk signer test cleanup evidence `b989bbcfbb2a427d3a538c5201f5d7214de6ba84`; upstream compatibility and upstream PR gate plan documented in `docs/dusk-upstream-compatibility-review.md` and `docs/dusk-upstream-pr-plan.md`; companion `TEST_REPORT.md` records latest clean-layout `make repro-check-agent` run `1778671118`, post-rebase clean-Rusk E2E runs `1778609411` and `1778609697`, and dependency-remediated clean-Rusk E2E runs `1778613709` and `1778613956`. Latest clean-layout repro passed for monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20` after rebasing onto upstream `2b7db706023806b36a57e446205ae443537ae9ec`, including the Dusk agent panic-path hardening, Dusk-fork image-publish guards, inherited Depot-backed workflow guards, and refreshed upstream base notes. | Internal PR ready for review, open, blocked on review/provisioning; review requested from `Neotamandua`; labels `need:feedback`, `type:feature`; `Dusk review policy gate` passing and required by default-branch protection; `Dusk agent cargo check` failing at the private companion-repo preflight until `DUSK_ORG_READ_TOKEN` is provisioned; inherited Depot-backed checks now skip instead of staying queued in the Dusk fork; merge-base equals upstream `2b7db706023806b36a57e446205ae443537ae9ec` |
| `dusk-network/hyperlane-dusk` workflow dispatcher | `ci/manual-repro-workflow` | Live PR head is checked through GitHub and `make gate-status`; contains only `.github/workflows/manual-repro-check.yml` and `.github/actionlint.yaml`; workflow logs requested refs and resolved checkout heads, then runs the final repro step under explicit `shell: bash` with `set -euo pipefail` before `make repro-check-agent` | Narrow default-branch PR #3 ready for review, open, blocked on review; review requested from `moCello` and `Neotamandua`; labels `need:feedback`, `type:docs`; `Manual repro dispatcher gate` and `Dusk review policy gate` passing |
| Clean Rusk reference | detached HEAD | `c0c64db4659500d077bb253ad13acba0e347d3fc` | Used for clean E2E evidence |

Post-repro monorepo hardening on 2026-05-13 moved
`dusk-network/hyperlane-monorepo` to
`48dcc0c87efc12584904d841d5088c1ae4acef20` after rebasing onto upstream
`2b7db706023806b36a57e446205ae443537ae9ec`. That line removed Dusk agent
`expect(...)` paths from RUES client construction and rkyv serialization,
propagated fallible Dusk provider construction through `hyperlane-base`, and
hardened `.github/workflows/dusk-agent-gate.yml` to use `git grep` for
`todo!`, `unimplemented!`, `panic!`, and `expect\(` in
`rust/main/chains/hyperlane-dusk/src` instead of relying on runner-local `rg`.
It also guards the inherited `rust-docker.yml` and `monorepo-docker.yml`
image-publishing workflows so Dusk-fork PRs do not attempt Hyperlane-org
Depot/GHCR publishing, guards inherited Depot-backed PR workflows so
Dusk-fork checks skip instead of staying queued on Hyperlane-owned runner
labels, and refreshes the upstream compatibility docs to record
`2b7db706023806b36a57e446205ae443537ae9ec` as the current base.
Local `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p
relayer -p scraper -p lander`, incremental `cargo check -p hyperlane-dusk -p
hyperlane-base`, workflow `actionlint`, and the
local Dusk agent panic/placeholder scan passed. The Dusk-side
`make review-hygiene` gate now also fails if the adjacent monorepo Dusk agent
source contains `todo!`, `unimplemented!`, `panic!`, or `expect\(`. CI
`Dusk review policy gate` passed on that head; CI `Dusk agent cargo check`
still fails at the expected private companion-repo preflight until
`DUSK_ORG_READ_TOKEN` is provisioned. The same live monorepo PR rollup now has
no queued checks: 18 completed successes, 22 completed skips, and the one
expected Dusk agent failure.

Dusk-side runtime hardening on 2026-05-13 moved
`dusk-network/hyperlane-dusk` to
`93ed3b07b26d8784610d2ba754a851490de15e21`. That line makes the `dusk-tx`
and `e2e` RUES clients propagate `reqwest::Client` build errors instead of
panicking during client construction. Focused `cargo check -p dusk-tx -p
hyperlane-dusk-e2e`, `cargo test -p dusk-tx`, `make secret-hygiene`, and
`git diff --check` passed before clean-layout repro run `1778671118` covered
the full non-E2E contract/type/tooling and Hyperlane Rust agent path.

Live verification on 2026-05-13:

```bash
gh pr view 1 --repo dusk-network/hyperlane-dusk \
  --json state,mergeable,headRefOid,reviewRequests,statusCheckRollup
gh pr view 1 --repo dusk-network/hyperlane-monorepo \
  --json state,mergeable,headRefOid,reviewRequests,statusCheckRollup
gh pr view 3 --repo dusk-network/hyperlane-dusk \
  --json state,mergeable,headRefOid,reviewRequests,statusCheckRollup
gh issue list --repo dusk-network/hyperlane-dusk --state open \
  --json number,title,assignees,labels,updatedAt --limit 20
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo rev-parse upstream/main
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo merge-base HEAD upstream/main
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo rev-list --left-right --count HEAD...upstream/main
gh workflow list --repo dusk-network/hyperlane-dusk --all
make completion-audit-status
make review-gates
make production-readiness-guard
make gate-status-fresh
```

Observed:

- Dusk PR is ready for review and open; review is requested from `moCello`;
  `Dusk review policy gate` is passing and `Production readiness guard` is
  failing as expected while production blockers remain. Recent CI guard runs
  completed with the same expected failure at Dusk heads
  `746935a2e8e4f0a2dc9e216b7b91a510e3a41e20`
  (https://github.com/dusk-network/hyperlane-dusk/actions/runs/25790821003/job/75755608816)
  and `cffcacc39b3408af891cd1c12c59e388540f2c81`
  (https://github.com/dusk-network/hyperlane-dusk/actions/runs/25791389384/job/75757564450).
  The failed-job summaries were limited to the known external blockers:
  unmerged/unapproved internal PRs, unknown branch-protection visibility from
  the Actions integration token, unchecked sign-off items, open split decision
  issues, unavailable Dependabot alert triage in Actions, unknown
  `dusk-hyperlane` runner visibility, and unknown repo-level Actions secret
  visibility.
- Monorepo PR is ready for review and open; review is requested from
  `Neotamandua`; `Dusk review policy gate` is passing and
  `Dusk agent cargo check` is failing at the private companion-repo preflight
  until `DUSK_ORG_READ_TOKEN` is provisioned.
- The live PR head SHAs are intentionally checked through GitHub instead of
  pinned in this file; documentation-only commits on this branch would
  otherwise make the file stale immediately.
- Manual workflow dispatcher PR #3 is ready for review, open, has
  `Manual repro dispatcher gate` and `Dusk review policy gate` passing, and
  contains only the workflow and actionlint files needed to make manual
  `workflow_dispatch` available from the default branch after Dusk review.
  A 2026-05-13 merge-order smoke check from `origin/main`
  `c5ce2135407dad6420d010bdafe82a0b9b4bb78d` verified that current dispatcher
  head `e54d6d49588ac24908c22dde2ac2345ddb983291` and current implementation
  head `c39273d918c7abcf506d2d2795b5f51bfea14b96` merge cleanly in either
  order, with no workflow/actionlint file drift after the combined merge.
- Split decision issues #4 through #9 are all open with `need:feedback` and
  `type:rfc` labels.
- Upstream Hyperlane `main` and the monorepo branch merge-base both resolve to
  `2b7db706023806b36a57e446205ae443537ae9ec`.
- The monorepo Dusk branch is checked live with `git rev-list --left-right
  --count HEAD...upstream/main` and `make gate-status`; it is not behind
  current upstream as of the latest gate refresh.
- Both local worktrees are clean and track their pushed origin branches.
- `git ls-files --others --exclude-standard` is empty in both active repos, so
  the Dusk contract/tooling repo and the Hyperlane monorepo fork do not have
  untracked source paths as their source of truth.
- `make completion-audit-status` verifies the preserved prototype archive
  branch refs, local backup artifact hashes, active branch refs, and untracked
  source state in both active repos.
- `make review-gates` wraps the lightweight non-E2E review gates:
  `make completion-audit-status`, `make archive-hygiene-test`,
  `make archive-hygiene`,
  `make dependency-alert-status`, `make review-hygiene`, and
  `make gate-status-fresh`.
- `make production-readiness-guard` is a negative guard that must fail while
  known machine-checkable production blockers remain open. Current blockers are
  open/unapproved internal PRs, unchecked sign-off items, open split decision
  issues, missing `DUSK_ORG_READ_TOKEN` visibility in both internal repos, and
  missing or unconfirmed `dusk-hyperlane` self-hosted runner visibility. It
  also reports non-completed PR status-check counts, runs
  `make dependency-alert-status` in summary mode, and blocks if open Cargo
  alert triage is unavailable or includes vulnerable locked versions, unparsed
  vulnerable ranges, or missing patched-version data. In CI, it ignores its
  own current `GITHUB_RUN_ID` and briefly polls other concurrently-started
  status checks before counting them, so the gate does not self-block while
  still catching genuinely incomplete companion checks. It is not a
  production-readiness proof.
- `dusk-network/hyperlane-dusk` default branch is `main`. The preserved
  prototype archive branch remains available as
  `archive/dusk-hyperlane-prototype-20260511`.
- `gh workflow list --repo dusk-network/hyperlane-dusk --all` and
  `make gate-status-fresh` now report the Dusk review policy gate, manual
  repro dispatcher gate, and production-readiness gate as active workflows.
  The manual repro workflow still needs the default-branch dispatcher PR,
  runner label, and read-only token provisioning before it can replace local
  repro evidence.
- Default-branch dispatcher PR dusk-network/hyperlane-dusk#3 now exists with
  only the manual workflow and actionlint config. It can make the workflow
  visible independently from the full Dusk Hyperlane implementation PR once
  Dusk accepts the runner/token setup.
- `.github/workflows/production-readiness-gate.yml` is now proposed on the
  implementation branch as a lightweight status-check candidate. It checks out
  a full-history Dusk copy, reports the Dusk head, uses GitHub's compare API
  for monorepo upstream freshness, then runs `make production-readiness-guard`;
  it is expected to fail until the known machine-checkable production blockers
  close.
- `make gate-status` prints the current machine-checkable gate status:
  7 unchecked sign-off items, 1 checked item, status checks on all internal
  PRs, active-repo untracked source status, the manual workflow dispatcher PR
  state, all six split decision issues open, workflow visibility,
  default-branch protection and merge method settings, required status-check
  policy enabled for `Dusk review policy gate`, repo-level Actions secret
  visibility, exact `DUSK_ORG_READ_TOKEN` visibility in both internal repos,
  optional `DUSK_STATUS_READ_TOKEN` visibility for the production-readiness
  workflow, self-hosted runner visibility, exact `dusk-hyperlane` runner-label
  visibility, Dusk Dependabot open-alert visibility with local `Cargo.lock`
  vulnerable-range comparison, current upstream drift, and no placeholder macro
  matches in the Dusk repo runtime paths or monorepo Dusk agent crate. It also
  reports whether active
  reviewer-facing PR/issue bodies include post-rebase E2E/archive links,
  dependency-remediated E2E, latest clean-layout repro, reviewer-routing links,
  `make gate-status-fresh`, `make dependency-alert-status`,
  `make completion-audit-status`, `make review-gates`,
  review-gates archive hygiene self-test coverage, review-gates extracted
  archive scan coverage, `make production-readiness-guard`, required
  status-check policy enabled, and latest clean-layout repro path delta.

## Prompt-To-Artifact Deliverable Checklist

| Requirement | Evidence | Status |
|---|---|---|
| Preserve current local prototype before rebasing | Backup directory: `/home/hein_/projects/hyperlane/.codex-backups/dusk-hyperlane-20260511T133907Z`; artifacts: `hyperlane-monorepo-tracked.diff`, `dusk-tree.tgz`, `hyperlane-dusk-untracked.tgz`; monorepo archive branch: `origin/archive/dusk-prototype-20260511` commit `8e399103b24673f837c04f4227e49c45c8366e7c`; Dusk archive branch: `origin/archive/dusk-hyperlane-prototype-20260511` commit `c5ce2135407dad6420d010bdafe82a0b9b4bb78d`; `make completion-audit-status` verifies these refs and hashes | Done |
| Fork `hyperlane-xyz/hyperlane-monorepo` into Dusk org | `dusk-network/hyperlane-monorepo`, PR #1: https://github.com/dusk-network/hyperlane-monorepo/pull/1 | Done |
| Create Dusk-specific Hyperlane contract/tooling repo | `dusk-network/hyperlane-dusk`, PR #1: https://github.com/dusk-network/hyperlane-dusk/pull/1 | Done |
| Put local `~/projects/hyperlane/dusk` under git and push | Dusk repo branch `feat/dusk-hardening-v2`; live PR head is checked through GitHub and `make gate-status`; `TEST_REPORT.md` records the latest clean-layout repro evidence and exact tested SHAs | Done |
| Re-port Hyperlane integration on current upstream main | Monorepo branch `feat/dusk-support-v2`; upstream freshness is verified with the live merge-base and ahead/behind checks recorded above; branch contains the clean Dusk chain crate/config/protocol integration commit `feat: re-port Dusk Hyperlane agent support` and is based on upstream `2b7db706023806b36a57e446205ae443537ae9ec` | Done for internal review |
| Keep work off `hyperlane-xyz` origin/main | Work is in Dusk forks and Dusk feature branches; both PRs target Dusk org repos | Done |
| Reapply integration in reviewable slices | Monorepo PR includes Dusk chain crate wiring, parser/config/protocol support, relayer/validator/scraper/lander checks; Dusk PR separates contract/tooling/security/test evidence | Done for internal review |
| Harden Mailbox, MerkleTreeHook, ValidatorAnnounce, MessageIdMultisigISM, IGP/protocol-fee, WarpDrc20, WarpDrc20Collateral, and WarpNative | `SECURITY_REVIEW.md` lists issue-by-issue fixes, assumptions, Solidity deviations, and file-level changes | Done for internal review |
| Address arithmetic, replay/domain separation, malformed metadata, duplicate delivery, admin paths, dirty redeploys, and token accounting | `SECURITY_REVIEW.md`; `TEST_REPORT.md`; `Cargo.toml` release `overflow-checks = true`; 70 integration tests; E2E/fault-injection runs listed below | Done for internal review |
| Avoid runtime `todo!`, `unimplemented!`, and direct `panic!` paths | `TEST_REPORT.md` records the Dusk contract/runtime/tooling scan command and result for `rg -n "todo!|unimplemented!|panic!" contracts types data-driver dusk-tx e2e wasm-bindings demo -g '!target'` with no matches in scoped production paths. `make gate-status` now also scans the tracked Dusk repo paths `contracts types data-driver dusk-tx e2e wasm-bindings demo` plus the monorepo `rust/main/chains/hyperlane-dusk/src` crate for direct placeholder/panic macros and `expect\(` in Dusk agent runtime source. The monorepo head `48dcc0c87efc12584904d841d5088c1ae4acef20` contains the Dusk agent `expect(...)` removals from `rust/main/chains/hyperlane-dusk/src`; local `git grep -n -E 'todo!|unimplemented!|panic!|expect\(' -- rust/main/chains/hyperlane-dusk/src`, `make review-hygiene`, and the updated CI scan report no matches before the CI job reaches the expected private Dusk checkout token preflight. The monorepo `docs/dusk-upstream-compatibility-review.md` records no added placeholder macros in the Dusk diff against `upstream/main`. | Done for internal review |
| Avoid leaking Dusk secrets through committed configs and process argv | Demo/E2E configs use ignored local files and `/tmp` artifacts; `duskKey` now supports `keyFile`/`keyEnv` so Dusk raw keys do not need to be embedded in generated JSON; `keyFile` rejects non-regular files and, on Unix, group/world-readable files before reading key material; demo agent configs reference `0600`-style `/tmp/*.key` files through `keyFile`; E2E wrappers remove generated Dusk signer key files on exit after stopping agents, and `make secret-hygiene` now guards future `gen-agent-configs.sh` consumers for that cleanup tracking; `dusk-tx` supports password file/env lookup and stdin raw-key loading; demo scripts avoid Dusk consensus passwords in CLI argv; exported GitHub-facing PR/issue text passed `scripts/secret-hygiene-check.sh`; `SECRET_HANDLING.md` and `make secret-hygiene` provide source/artifact guardrails; `SECURITY_REVIEW.md` keeps production signer/config handling as a release gate | Gated |
| Dusk VM contract/type checks | `make all`; `make clippy-contracts` for the production contract/type surface -> passed; `cargo test -p hyperlane-dusk-types` -> 28 passed; `cargo test -p hyperlane-dusk-integration-tests` -> 70 passed; `cargo test -p dusk-tx` -> 3 passed; latest clean-layout `make repro-check-agent` result `1778671118` with exact tested SHAs is recorded in `TEST_REPORT.md` | Done |
| Dusk Rust dependency advisories | GitHub Dependabot alerts were triaged against the active dependency graph. `Cargo.toml`/`Cargo.lock` now move the reported vulnerable Rust packages to patched versions where applicable: `wasmtime` 36.0.9, `openssl` 0.10.79, `rustls-webpki` 0.103.13, `quinn-proto` 0.11.14, `rand` 0.8.6/0.9.3, and `keccak` 0.1.6; `lru` was already 0.16.4. Dependency remediation run `1778613424` passed 28 type tests, 3 `dusk-tx` tests, 70 VM integration tests, CLI/E2E compile checks, dependency-tree probes, and secret hygiene. Clean-Rusk E2E runs `1778613709` and `1778613956` passed the TestMock and MessageIdMultisig bidirectional bridge paths on the updated graph. `make dependency-alert-status` compares open GitHub Dependabot `Cargo.lock` alerts against the current local lockfile and confirms no local locked package versions fall within the alert vulnerable ranges; `make gate-status` also reports current Dusk Dependabot open-alert visibility so reviewers can distinguish the patched local graph from GitHub's rescan/default-branch state. | Done for internal review |
| Hyperlane Rust agent checks | From monorepo `rust/main`: `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander` -> passed after rebasing onto upstream `2b7db706`; latest clean-layout `make repro-check-agent` result `1778671118` with exact tested SHAs is recorded in `TEST_REPORT.md` | Done |
| Local EVM <-> Dusk with TestMock/null-style ISM | Clean Rusk run `1778520709`: bidirectional E2E passed; review-head clean-Rusk run `1778587094` passed; post-rebase clean-Rusk run `1778609411` passed; dependency-remediated clean-Rusk run `1778613709` passed with monorepo `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`; current-head clean-Rusk run `1778672800` passed with Dusk PR head `fd2ec8fe996dd28f85259e6aebd8db25182c20de` and monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20` | Done |
| Local EVM <-> Dusk with MessageIdMultisigISM | Clean Rusk run `1778521018`: bidirectional E2E with validator/checkpoint passed; review-head clean-Rusk run `1778587351` passed; post-rebase clean-Rusk run `1778609697` passed; dependency-remediated clean-Rusk run `1778613956` passed with monorepo `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`; current-head clean-Rusk run `1778673269` passed with Dusk PR head `fd2ec8fe996dd28f85259e6aebd8db25182c20de` and monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20` | Done |
| Bidirectional DUSK/wDUSK bridge through relayer/validator, no manual process calls | `TEST_REPORT.md` records relayer/validator-driven EVM -> Dusk and Dusk -> EVM flows for TestMock and MessageIdMultisig; current-head clean-Rusk runs `1778672800` and `1778673269` passed both directions after the RUES client hardening and docs/guard-only head updates | Done |
| E2E freshness against current runtime source | `TEST_REPORT.md` records current-head clean-Rusk TestMock and MessageIdMultisig E2E runs `1778672800` and `1778673269` at Dusk PR head `fd2ec8fe996dd28f85259e6aebd8db25182c20de`, latest covered Dusk implementation ref `93ed3b07b26d8784610d2ba754a851490de15e21`, monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc` | Done |
| Stress/reliability: relayer restart/backlog | Clean Rusk run `1778524643`: 50 transfers each direction; clean soak runs `1778528713`, `1778529143`, `1778530903`, and `1778541618`; the latest run completed 7 cycles, 20 transfers each direction per cycle, 280 total transfers, in 7282 seconds | Done for internal review; Dusk reviewers still decide soak acceptance for release |
| Stress/reliability: dirty redeploy guard | Clean Rusk run `1778522551` | Done |
| Stress/reliability: validator delay/checkpoint backoff | Clean Rusk run `1778522740` | Done |
| Stress/reliability: metadata corruption | Clean Rusk run `1778523252` | Done |
| Stress/reliability: low signer balance | Clean Rusk run `1778523531` | Done |
| Stress/reliability: origin RPC failure | Clean Rusk run `1778523772` | Done |
| Stress/reliability: destination RPC failure | Clean Rusk run `1778524014` | Done |
| Stress/reliability: duplicate relayer/no double-delivery | Clean Rusk run `1778524257` | Done |
| Prepare internal Dusk PR for contracts/tooling/tests/audit notes | Dusk PR #1 is ready for review, open, and mergeable; body, handoff comments, and `PRODUCTION_REVIEW_DECISIONS.md` include current evidence and remaining gates | Done |
| Prepare internal Dusk PR for Hyperlane agent/protocol integration | Monorepo PR #1 is ready for review, open, and mergeable; body, handoff comments, and `docs/dusk-upstream-compatibility-review.md` include companion Dusk evidence and remaining gates | Done |
| Prepare upstream Hyperlane draft PRs only after internal review | Not started by design; PR bodies explicitly state upstream prep waits for internal Dusk review | Gated |

## Evidence Index

| Evidence | Location |
|---|---|
| Prototype archive branches | `dusk-network/hyperlane-monorepo` `archive/dusk-prototype-20260511` at `8e399103b24673f837c04f4227e49c45c8366e7c`; `dusk-network/hyperlane-dusk` `archive/dusk-hyperlane-prototype-20260511` at `c5ce2135407dad6420d010bdafe82a0b9b4bb78d` |
| Prototype backup artifact hashes | Reverified on 2026-05-12. `dusk-tree.tgz`: `8b38b322f807735944501c280f554f83c278a4cebe62e2282478e03da6bdd6ee`; `hyperlane-dusk-untracked.tgz`: `52942cc44b0c87e86b7a1ed8533273dc664eb27b5cea1ddf8d62ee46b9ab8900`; `hyperlane-monorepo-tracked.diff`: `2fe0dc466de389167f756eae23d926234c9df31c1de435a4e68006355bfc7d0e` |
| Security assumptions, fixes, Solidity deviations, reviewer decisions | `SECURITY_REVIEW.md` |
| Dusk reference traceability | `REFERENCE_TRACEABILITY.md` |
| Reviewer decision record | `PRODUCTION_REVIEW_DECISIONS.md`; split contract-policy issues #4 through #6 must keep current clean-layout repro evidence https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895 tied to their test coverage |
| Advisory reviewer routing | `REVIEWERS.md`; routes Dusk contract/tooling/security review to `moCello`, Hyperlane agent/runtime review to `Neotamandua`, split decision issues #4 through #9 to the named Dusk reviewers, and keeps production sign-off in dusk-network/hyperlane-dusk#2 |
| Secret handling policy, signer custody proposal, and source/artifact guardrail | `SECRET_HANDLING.md`, `PRODUCTION_SIGNER_POLICY.md`, `scripts/secret-hygiene-check.sh`, `scripts/github-review-hygiene.sh`, `make secret-hygiene`, `make review-hygiene`; dusk-network/hyperlane-dusk#7 must keep current signer-custody evidence links to latest clean-layout repro https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895 and dependency-remediated E2E https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389 |
| Extracted evidence archive hygiene | `make archive-hygiene-test`; `make archive-hygiene`; on 2026-05-13 all durable `.tgz` evidence archives in `/home/hein_/projects/hyperlane/.codex-backups` were extracted into a temporary directory and scanned with `bash scripts/secret-hygiene-check.sh "$scan_root"`; the extracted-content scan covered repro, E2E, dependency-remediation, and soak handoff archives and found no secret-like filenames or signer/password command text. The wrapper rejects unsafe member paths, non-regular/non-directory archive members, and extracted symlinks or special files; the self-test covers safe archives plus traversal, symlink, and secret-bearing rejection paths; the exact command shape is recorded in `TEST_REPORT.md`. |
| Reviewer-facing link guardrails | `scripts/github-review-hygiene.sh`, `scripts/release-gate-status.sh`, `scripts/dependency-alert-status.sh`, `scripts/completion-audit-status.sh`, `scripts/production-readiness-guard.sh`, `make review-hygiene`, `make dependency-alert-status`, `make completion-audit-status`, `make gate-status`, `make gate-status-fresh`, `make review-gates`, `make production-readiness-guard`; Dusk PR #1, monorepo PR #1, and sign-off issue #2 bodies must include or visibly report latest clean-layout repro https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895, dependency-remediated E2E https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389, post-rebase E2E/archive links, the advisory reviewer routing URL https://github.com/dusk-network/hyperlane-dusk/blob/feat/dusk-hardening-v2/REVIEWERS.md, `make gate-status-fresh`, `make dependency-alert-status`, `make completion-audit-status`, `make review-gates`, archive hygiene self-tests, extracted evidence archive hygiene scans, `make production-readiness-guard`, required status-check policy enabled, and latest clean-layout repro path delta; workflow PR #3 must include or visibly report the advisory reviewer routing URL and the #8 CI provisioning runbook https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4435830841; active bodies must not include the superseded `1778607202`/`4433179148` clean-layout repro evidence, the stale pre-archive-hygiene `make review-gates` bundle description, or stale monorepo queued-check status from before the inherited Depot workflow guards |
| CI/repro runner proposal | `CI_REPRO_STRATEGY.md`, including the `Admin Provisioning Runbook`; `SECRET_HANDLING.md`; `PRODUCTION_REVIEW_DECISIONS.md`; `.github/workflows/manual-repro-check.yml`; `.github/workflows/production-readiness-gate.yml`; `make repro-check-agent`; `make gate-status`; `make gate-status-fresh`; dusk-network/hyperlane-dusk#3; dusk-network/hyperlane-dusk#8; latest #8 runbook comment https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4435830841. The docs scope `DUSK_ORG_READ_TOKEN` to read-only source checkout for `dusk-network/hyperlane-dusk`, `dusk-network/hyperlane-monorepo`, and `dusk-network/rusk-private`, and explicitly exclude signer/runtime/image-publishing uses. Branch protection, Actions-secret, runner-admin, and Dependabot-alert visibility are separated as production-readiness status gates that need approved admin/security-read visibility rather than broader source-checkout token scope. The production-readiness workflow can use optional `DUSK_STATUS_READ_TOKEN` for status visibility only; the manual repro checkout workflow continues to use `DUSK_ORG_READ_TOKEN`. |
| Commands, clean Rusk commit, run IDs, artifact paths, pass/fail notes | `TEST_REPORT.md` |
| Durable local evidence archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-evidence-20260512T1245Z.tgz`; SHA256 `53bc99b30624471b145731b92896b442c3a88c97f8217c8a7cd12be4ab7476fc`; contains the latest review-head repro/E2E logs plus the 7282-second soak logs; `scripts/secret-hygiene-check.sh` passed over the archive staging directory and tarball |
| Soak acceptance evidence guardrail | dusk-network/hyperlane-dusk#9 and `PRODUCTION_REVIEW_DECISIONS.md`; both record clean-Rusk soak run `1778541618`, 7 completed cycles, 280 completed transfers, 7282 seconds, clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`, and evidence archive https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430670295 |
| Post-rebase clean-Rusk E2E archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-live-head-e2e-1778609411-1778609697.tgz`; SHA256 `3e5303799e0def227988e0ac66db9d021e94bd39df054629137896c5d7b07ccc`; contains the post-rebase TestMock and MessageIdMultisig safe text logs for runs `1778609411` and `1778609697`; `scripts/secret-hygiene-check.sh` passed over the archive staging directory and tarball |
| Dependency advisory remediation archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-1778613424.tgz`; SHA256 `507bd4d4dfc7bfd6c6fd517f3f8d6edc83e662d2b81530e4be933ddd53ac6433`; contains focused remediation test logs and dependency-tree probes; `scripts/secret-hygiene-check.sh` passed over the log directory and tarball |
| Dependency-remediated clean-Rusk E2E archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956.tgz`; SHA256 `36df8403ac20fa2b62e1e69780229222520102457a9d3f24165f6a4dbd924f13`; contains TestMock and MessageIdMultisig safe text logs for runs `1778613709` and `1778613956`; `scripts/secret-hygiene-check.sh` passed over the archive staging directory and tarball |
| Current-head clean-Rusk E2E archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-e2e-1778672800-1778673269.tgz`; SHA256 `2fab09b0a4196129bbc32084cc3664ed81c5f78222cde75d47fdb314a1c825f8`; contains TestMock and MessageIdMultisig safe text logs for runs `1778672800` and `1778673269`; `scripts/secret-hygiene-check.sh` passed over the archive staging directory and tarball; evidence comment https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440708799 |
| Previous clean-layout repro evidence archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-final-head-repro-20260512T144314Z.tgz`; SHA256 `e8c17cb43da130091a52b2c711aa64d0fef000b599997a372c968970c52dfa7e`; contains clean-layout repro run `1778596710`; `scripts/secret-hygiene-check.sh` passed over the archive staging directory and tarball |
| Latest clean-layout repro evidence archive | `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-rues-hardening-repro-1778671118.tgz`; SHA256 `7a4a981422718ca081ce87c09af52ca4f2a2f0893c7ffcee3e19484739f8c460`; contains clean-layout repro run `1778671118` for Dusk source ref `93ed3b07b26d8784610d2ba754a851490de15e21` and monorepo ref `48dcc0c87efc12584904d841d5088c1ae4acef20`; `scripts/secret-hygiene-check.sh` passed over the repro log, archive staging directory, and tarball |
| Live-head delta after latest clean-layout repro | The live Dusk and monorepo PR heads are checked through GitHub and `make gate-status`. Latest clean-layout repro run `1778671118` covers Dusk implementation source ref `93ed3b07b26d8784610d2ba754a851490de15e21` and current monorepo ref `48dcc0c87efc12584904d841d5088c1ae4acef20`; subsequent Dusk docs/guard-only commits are outside `DUSK_REPRO_COVERED_PATHS`, and the production-readiness guard reports `coveredPathDelta: none`. |
| Repeatable local E2E scripts | `demo/e2e-agents.sh`, `demo/e2e-relayer-restart-stress.sh`, `demo/e2e-soak-restart-stress.sh`, `demo/e2e-*.sh` |
| Demo and E2E command documentation | `demo/README.md` |
| Dusk contract/tooling PR | https://github.com/dusk-network/hyperlane-dusk/pull/1 |
| Hyperlane monorepo integration PR | https://github.com/dusk-network/hyperlane-monorepo/pull/1 |
| Upstream compatibility review | `dusk-network/hyperlane-monorepo` `docs/dusk-upstream-compatibility-review.md` |
| Monorepo dependency advisory scope | `dusk-network/hyperlane-monorepo` `docs/dusk-upstream-compatibility-review.md`; PR note https://github.com/dusk-network/hyperlane-monorepo/pull/1#issuecomment-4434167729 |
| Production sign-off tracker | https://github.com/dusk-network/hyperlane-dusk/issues/2 |
| Consolidated review entry points | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427052555 |
| Recent head and gate refresh | Active Dusk PR #1, monorepo PR #1, and sign-off issue #2 bodies each include a `head and gate refresh:` handoff entry; latest posted gate comment before this audit refresh: https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4435213587 |
| Latest clean-layout repro evidence | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895 |
| Historical clean-layout repro evidence | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427472641 |
| Review-head clean-layout repro evidence | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430201984 |
| Review-head clean-Rusk E2E evidence | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430343619 |
| Focused decision evidence comments | #4 https://github.com/dusk-network/hyperlane-dusk/issues/4#issuecomment-4427024964; #5 https://github.com/dusk-network/hyperlane-dusk/issues/5#issuecomment-4427030597; #6 https://github.com/dusk-network/hyperlane-dusk/issues/6#issuecomment-4427030602; #7 https://github.com/dusk-network/hyperlane-dusk/issues/7#issuecomment-4427035690; #8 https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4427017481; #9 https://github.com/dusk-network/hyperlane-dusk/issues/9#issuecomment-4427046636 |

## Remaining Gates

1. Dusk reviewers must accept or change the open production review decisions in
   `SECURITY_REVIEW.md`:
   - Mailbox transfer-contract sender resolution.
   - Immutable `registered_accounts` registration.
   - No admin drain for native/collateral pending escrow.
2. Soak acceptance remains a reviewer/release-policy gate. Current evidence
   includes a 50-transfer restart/backlog run, 1-cycle and 2-cycle clean-Rusk
   soak runs, a 3102-second 3-cycle clean-Rusk soak with 20 transfers each
   direction per cycle, and a 7282-second 7-cycle clean-Rusk soak with 20
   transfers each direction per cycle.
3. Production secret handling needs an operational/CI sign-off. Current scripts
   keep dev configs and Dusk signer key files in ignored `/tmp` paths, generated
   Dusk agent configs use `duskKey.keyFile` instead of inline Dusk raw keys,
   the agent rejects non-regular and group/world-readable key files on Unix,
   Dusk consensus passwords are no longer passed to `dusk-tx` in process argv,
   and `make secret-hygiene` provides source/artifact guardrails.
   `PRODUCTION_SIGNER_POLICY.md` records the current `duskKey` file/env support
   plus acceptable v1 custody choices for Dusk review. Production deployment
   must still accept or replace that signer custody and CI artifact policy.
4. Upstream Hyperlane draft PRs should not be prepared until the internal Dusk
   PRs complete review.
5. The Hyperlane monorepo fork has upstream `.github/CODEOWNERS` routing
   `rust/` to Hyperlane's `@tkporter`. The companion Dusk repo now has
   advisory reviewer routing in `REVIEWERS.md`, but no enforced `CODEOWNERS`
   or branch-protection ownership. Internal Dusk agent/runtime review is still
   requested from `Neotamandua`, based on recent Rusk HTTP/RUES/GraphQL route
   ownership, because the remaining decisions are Dusk/Rusk-specific and not
   covered by upstream Hyperlane ownership alone.
6. CI/repro runner strategy remains a release gate. The internal PRs now have
   status-check rollups and both Dusk org default branches require the shared
   `Dusk review policy gate`; the Dusk workspace still depends on an adjacent
   private `rusk-private` checkout for the heavy repro path. `make repro-check` and
   `make repro-check-agent` now wrap the repeatable local non-E2E verification
   subset, and `.github/workflows/manual-repro-check.yml` defines a manual
   self-hosted workflow that preserves the private dependency checkout layout
   while accepting explicit `dusk_ref`, `rusk_ref`, and `monorepo_ref` inputs.
   The final workflow repro step uses explicit `shell: bash` and
   `set -euo pipefail` before invoking `make repro-check-agent`.
   `CI_REPRO_STRATEGY.md`, `SECRET_HANDLING.md`, and
   `PRODUCTION_REVIEW_DECISIONS.md` record the proposed runner labels, checkout
   layout, read-only source-checkout token scope for the Dusk, monorepo, and
   Rusk repos, artifact policy, promotion path, branch protection/status-check
   policy, and default-branch visibility caveat. The docs also state that
   Hyperlane image-publishing/GitHub App credentials are not required for
   internal Dusk review because the inherited Rust image workflow is skipped on
   the Dusk fork and remains enabled for the later upstream PR path. The
   workflow still needs a Dusk runner/secret decision and must land on the
   default branch before it can replace local evidence. The default branches
   now have protected-branch review baselines and required status-check policy
   enabled. The narrow default-branch dispatcher PR is
   https://github.com/dusk-network/hyperlane-dusk/pull/3.
7. The remaining production sign-off work is tracked in
   https://github.com/dusk-network/hyperlane-dusk/issues/2 and summarized for
   PR review in `PRODUCTION_REVIEW_DECISIONS.md`. The individual decision
   issues are dusk-network/hyperlane-dusk#4 through
   dusk-network/hyperlane-dusk#9:
   Mailbox sender resolution, immutable account registration, pending escrow
   recovery, production signer custody, CI/repro runner policy, and soak
   acceptance. Each split issue now has a source/evidence comment linked from
   https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427052555
   for reviewer routing.

## Completion Decision

The revival branch is ready for internal Dusk review, but the overall objective
is not complete. The remaining gates above must be closed before claiming
production readiness or preparing upstream Hyperlane PRs.
