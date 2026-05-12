# Dusk Hyperlane Revival Goal Audit

Date: 2026-05-12

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
| `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | Live PR head is checked through GitHub and `make gate-status`; implementation/test evidence through `e4d3f2ab704286fe89e43b24543f8104b8838633`; cleanup regression guard commit `fd5269a6983fc920b5f2d1201b162ff7c11bbef0`; later commits refresh audit, CI/repro, signer, default-branch, cross-repo handoff docs, generated signer key cleanup, cleanup regression guards, gate-status reporting, and split decision issue routing; `TEST_REPORT.md` records supplemental clean-layout `make repro-check-agent` runs with exact tested SHAs, including the latest clean-layout repro evidence at tested Dusk `efb6fd80bf199ef15c88ceb49b0fe23a25c12271` and monorepo `dea286bd364a9268413fd5b1cfc51bd983d443be` | Internal PR ready for review, open, mergeable; review requested from `moCello`; labels `need:feedback`, `type:feature`; no status checks configured |
| `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | Live PR head is checked through GitHub and `make gate-status`; rebase/check evidence `f0df7aa522c65c4a7cf94c677c9573bd353c9b72`; Dusk signer test cleanup evidence `662d8b850b8903d15e1d7a24cfef1862d9a2f9ea`; upstream compatibility and upstream PR gate plan documented in `docs/dusk-upstream-compatibility-review.md` and `docs/dusk-upstream-pr-plan.md`; companion `TEST_REPORT.md` records supplemental clean-layout `make repro-check-agent` runs with exact tested SHAs, including the file-backed Dusk signer parser/builder and key-file permission update | Internal PR ready for review, open, mergeable; review requested from `Neotamandua`; labels `need:feedback`, `type:feature`; no status checks configured; merge-base equals upstream `f758a70630fd72d4749c3afb79454e725b8081a8` |
| `dusk-network/hyperlane-dusk` workflow dispatcher | `ci/manual-repro-workflow` | Current pushed head `6f0a921acb809261f393a3e70eb33f056078a33d`; contains only `.github/workflows/manual-repro-check.yml` and `.github/actionlint.yaml`; workflow logs requested refs and resolved checkout heads before `make repro-check-agent` | Narrow default-branch PR #3 ready for review, open, mergeable; review requested from `moCello` and `Neotamandua`; labels `need:feedback`, `type:docs`; no status checks configured |
| Clean Rusk reference | detached HEAD | `c0c64db4659500d077bb253ad13acba0e347d3fc` | Used for clean E2E evidence |

Live verification on 2026-05-12:

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
make gate-status
```

Observed:

- Dusk PR is ready for review, open, and mergeable; review is requested from
  `moCello`; it has zero status-check rollup entries.
- Monorepo PR is ready for review, open, and mergeable; review is requested
  from `Neotamandua`; it has zero status-check rollup entries.
- The live PR head SHAs are intentionally checked through GitHub instead of
  pinned in this file; documentation-only commits on this branch would
  otherwise make the file stale immediately.
- Manual workflow dispatcher PR #3 is ready for review, open, mergeable, and
  contains only the workflow and actionlint files needed to make manual
  `workflow_dispatch` available from the default branch after Dusk review.
- Split decision issues #4 through #9 are all open with `need:feedback` and
  `type:rfc` labels.
- Upstream Hyperlane `main` and the monorepo branch merge-base both resolve to
  `f758a70630fd72d4749c3afb79454e725b8081a8`.
- The monorepo Dusk branch is `20 0` relative to `upstream/main`, so it is not
  behind current upstream as of this check.
- Both local worktrees are clean and track their pushed origin branches.
- `git ls-files --others --exclude-standard` is empty in both active repos, so
  the Dusk contract/tooling repo and the Hyperlane monorepo fork do not have
  untracked source paths as their source of truth.
- `dusk-network/hyperlane-dusk` default branch is `main`. The preserved
  prototype archive branch remains available as
  `archive/dusk-hyperlane-prototype-20260511`.
- `gh workflow list --repo dusk-network/hyperlane-dusk --all` currently
  returns no discoverable workflows, matching the documented caveat that
  `.github/workflows/manual-repro-check.yml` must land on the default branch
  before it can replace local repro evidence.
- Default-branch dispatcher PR dusk-network/hyperlane-dusk#3 now exists with
  only the manual workflow and actionlint config. It can make the workflow
  visible independently from the full Dusk Hyperlane implementation PR once
  Dusk accepts the runner/token setup.
- `make gate-status` prints the current machine-checkable gate status:
  7 unchecked sign-off items, 1 checked item, zero status checks on the
  implementation PRs, the manual workflow dispatcher PR state, all six split
  decision issues open, no visible workflow, upstream drift `20 0`, and no
  placeholder macro matches in `rust/main/chains/hyperlane-dusk`.

## Prompt-To-Artifact Deliverable Checklist

| Requirement | Evidence | Status |
|---|---|---|
| Preserve current local prototype before rebasing | Backup directory: `/home/hein_/projects/hyperlane/.codex-backups/dusk-hyperlane-20260511T133907Z`; artifacts: `hyperlane-monorepo-tracked.diff`, `dusk-tree.tgz`, `hyperlane-dusk-untracked.tgz`; monorepo archive branch: `origin/archive/dusk-prototype-20260511` commit `8e399103b24673f837c04f4227e49c45c8366e7c`; Dusk archive branch: `origin/archive/dusk-hyperlane-prototype-20260511` commit `c5ce2135407dad6420d010bdafe82a0b9b4bb78d` | Done |
| Fork `hyperlane-xyz/hyperlane-monorepo` into Dusk org | `dusk-network/hyperlane-monorepo`, PR #1: https://github.com/dusk-network/hyperlane-monorepo/pull/1 | Done |
| Create Dusk-specific Hyperlane contract/tooling repo | `dusk-network/hyperlane-dusk`, PR #1: https://github.com/dusk-network/hyperlane-dusk/pull/1 | Done |
| Put local `~/projects/hyperlane/dusk` under git and push | Dusk repo branch `feat/dusk-hardening-v2`; implementation/test evidence commit `e4d3f2a`; later commits refresh review/audit handoff docs | Done |
| Re-port Hyperlane integration on current upstream main | Monorepo branch `feat/dusk-support-v2`; rebase/check evidence commit `f0df7aa`; branch contains the clean Dusk chain crate/config/protocol integration commit `feat: re-port Dusk Hyperlane agent support` and is based on upstream `f758a706` | Done for internal review |
| Keep work off `hyperlane-xyz` origin/main | Work is in Dusk forks and Dusk feature branches; both PRs target Dusk org repos | Done |
| Reapply integration in reviewable slices | Monorepo PR includes Dusk chain crate wiring, parser/config/protocol support, relayer/validator/scraper/lander checks; Dusk PR separates contract/tooling/security/test evidence | Done for internal review |
| Harden Mailbox, MerkleTreeHook, ValidatorAnnounce, MessageIdMultisigISM, IGP/protocol-fee, WarpDrc20, WarpDrc20Collateral, and WarpNative | `SECURITY_REVIEW.md` lists issue-by-issue fixes, assumptions, Solidity deviations, and file-level changes | Done for internal review |
| Address arithmetic, replay/domain separation, malformed metadata, duplicate delivery, admin paths, dirty redeploys, and token accounting | `SECURITY_REVIEW.md`; `TEST_REPORT.md`; 67 integration tests; E2E/fault-injection runs listed below | Done for internal review |
| Avoid runtime `todo!`, `unimplemented!`, and direct `panic!` paths | `TEST_REPORT.md` records the Dusk contract/runtime/tooling scan command and result for `rg -n "todo!|unimplemented!|panic!" contracts types data-driver dusk-tx e2e wasm-bindings demo -g '!target'` with no matches in scoped production paths. The monorepo `docs/dusk-upstream-compatibility-review.md` records the Dusk agent scan: no matches in `rust/main/chains/hyperlane-dusk`, and no added placeholder macros in the Dusk diff against `upstream/main`. | Done |
| Avoid leaking Dusk secrets through committed configs and process argv | Demo/E2E configs use ignored local files and `/tmp` artifacts; `duskKey` now supports `keyFile`/`keyEnv` so Dusk raw keys do not need to be embedded in generated JSON; `keyFile` rejects non-regular files and, on Unix, group/world-readable files before reading key material; demo agent configs reference `0600`-style `/tmp/*.key` files through `keyFile`; E2E wrappers remove generated Dusk signer key files on exit after stopping agents, and `make secret-hygiene` now guards future `gen-agent-configs.sh` consumers for that cleanup tracking; `dusk-tx` supports password file/env lookup and stdin raw-key loading; demo scripts avoid Dusk consensus passwords in CLI argv; exported GitHub-facing PR/issue text passed `scripts/secret-hygiene-check.sh`; `SECRET_HANDLING.md` and `make secret-hygiene` provide source/artifact guardrails; `SECURITY_REVIEW.md` keeps production signer/config handling as a release gate | Gated |
| Dusk VM contract/type checks | `make all`; `cargo test -p hyperlane-dusk-types` -> 28 passed; `cargo test -p hyperlane-dusk-integration-tests` -> 67 passed; `cargo test -p dusk-tx` -> 3 passed; latest clean-layout `make repro-check-agent` result with exact tested SHAs is recorded in `TEST_REPORT.md` | Done |
| Hyperlane Rust agent checks | From monorepo `rust/main`: `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander` -> passed after rebasing onto upstream `f758a706`; latest clean-layout `make repro-check-agent` result with exact tested SHAs is recorded in `TEST_REPORT.md` | Done |
| Local EVM <-> Dusk with TestMock/null-style ISM | Clean Rusk run `1778520709`: bidirectional E2E passed | Done |
| Local EVM <-> Dusk with MessageIdMultisigISM | Clean Rusk run `1778521018`: bidirectional E2E with validator/checkpoint passed | Done |
| Bidirectional DUSK/wDUSK bridge through relayer/validator, no manual process calls | `TEST_REPORT.md` records relayer/validator-driven EVM -> Dusk and Dusk -> EVM flows for TestMock and MessageIdMultisig | Done |
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
| Reviewer decision record | `PRODUCTION_REVIEW_DECISIONS.md` |
| Secret handling policy, signer custody proposal, and source/artifact guardrail | `SECRET_HANDLING.md`, `PRODUCTION_SIGNER_POLICY.md`, `scripts/secret-hygiene-check.sh`, `make secret-hygiene` |
| CI/repro runner proposal | `CI_REPRO_STRATEGY.md`, `.github/workflows/manual-repro-check.yml`, `make repro-check-agent`, `make gate-status`, dusk-network/hyperlane-dusk#3 |
| Commands, clean Rusk commit, run IDs, artifact paths, pass/fail notes | `TEST_REPORT.md` |
| Repeatable local E2E scripts | `demo/e2e-agents.sh`, `demo/e2e-relayer-restart-stress.sh`, `demo/e2e-soak-restart-stress.sh`, `demo/e2e-*.sh` |
| Demo and E2E command documentation | `demo/README.md` |
| Dusk contract/tooling PR | https://github.com/dusk-network/hyperlane-dusk/pull/1 |
| Hyperlane monorepo integration PR | https://github.com/dusk-network/hyperlane-monorepo/pull/1 |
| Upstream compatibility review | `dusk-network/hyperlane-monorepo` `docs/dusk-upstream-compatibility-review.md` |
| Production sign-off tracker | https://github.com/dusk-network/hyperlane-dusk/issues/2 |
| Consolidated review entry points | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427052555 |
| Current head and gate refresh | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427263548 |
| Latest clean-layout repro evidence | https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4427222531 |
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
5. The Hyperlane monorepo PR has internal Dusk agent/runtime review requested
   from `Neotamandua`, based on recent Rusk HTTP/RUES/GraphQL route ownership.
   No `CODEOWNERS`, `OWNERS`, or `MAINTAINERS` file was found in either repo.
6. CI/repro runner strategy remains a release gate. Both internal PRs currently
   have empty status-check rollups, and the Dusk workspace depends on an
   adjacent private `rusk-private` checkout. `make repro-check` and
   `make repro-check-agent` now wrap the repeatable local non-E2E verification
   subset, and `.github/workflows/manual-repro-check.yml` defines a manual
   self-hosted workflow that preserves the private dependency checkout layout
   while accepting explicit `dusk_ref`, `rusk_ref`, and `monorepo_ref` inputs.
   `CI_REPRO_STRATEGY.md` records the proposed runner labels, checkout layout,
   read-only token scope, artifact policy, promotion path, and default-branch
   visibility caveat. The workflow still needs a Dusk runner/secret decision
   and must land on the default branch before it can replace local evidence.
   The narrow default-branch dispatcher PR is
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
