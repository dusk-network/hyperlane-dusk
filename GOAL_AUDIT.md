# Dusk Hyperlane Revival Goal Audit

Date: 2026-05-12

This audit maps the original revival/hardening goal to concrete artifacts. It
is intentionally not a production-readiness sign-off. Items marked `Gated`
need Dusk reviewer acceptance or additional release evidence before upstream
submission or production claims.

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
evidence. Criterion 6 is intentionally incomplete because internal Dusk review,
production signer/CI policy, CI runner strategy, and soak acceptance are still
open in `dusk-network/hyperlane-dusk#2`.

## Branches And Evidence Commits

| Component | Branch | Evidence | State |
|---|---|---|---|
| `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | Implementation/test evidence through `e4d3f2ab704286fe89e43b24543f8104b8838633`; later commits refresh audit, CI/repro, signer, and cross-repo handoff docs | Internal PR open, mergeable; review requested from `moCello`; no status checks configured |
| `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | Rebase/check evidence `f0df7aa522c65c4a7cf94c677c9573bd353c9b72`; upstream compatibility review documented in `docs/dusk-upstream-compatibility-review.md` | Internal PR open, mergeable; review requested from `Neotamandua`; no status checks configured; merge-base equals upstream `f758a70630fd72d4749c3afb79454e725b8081a8` |
| Clean Rusk reference | detached HEAD | `c0c64db4659500d077bb253ad13acba0e347d3fc` | Used for clean E2E evidence |

Live verification on 2026-05-12:

```bash
gh pr view 1 --repo dusk-network/hyperlane-dusk \
  --json state,mergeable,headRefOid,reviewRequests,statusCheckRollup
gh pr view 1 --repo dusk-network/hyperlane-monorepo \
  --json state,mergeable,headRefOid,reviewRequests,statusCheckRollup
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo rev-parse upstream/main
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo merge-base HEAD upstream/main
```

Observed:

- Dusk PR is open and mergeable, review requested from `moCello`, with zero
  status-check rollup entries.
- Monorepo PR is open and mergeable, review requested from `Neotamandua`, with
  zero status-check rollup entries.
- Upstream Hyperlane `main` and the monorepo branch merge-base both resolve to
  `f758a70630fd72d4749c3afb79454e725b8081a8`.
- Both local worktrees are clean and track their pushed origin branches.

## Deliverable Checklist

| Requirement | Evidence | Status |
|---|---|---|
| Preserve current local prototype before rebasing | Backup directory: `/home/hein_/projects/hyperlane/.codex-backups/dusk-hyperlane-20260511T133907Z`; artifacts: `hyperlane-monorepo-tracked.diff`, `dusk-tree.tgz`, `hyperlane-dusk-untracked.tgz`; archive branch: `archive/dusk-prototype-20260511` commit `8e399103b` | Done |
| Fork `hyperlane-xyz/hyperlane-monorepo` into Dusk org | `dusk-network/hyperlane-monorepo`, PR #1: https://github.com/dusk-network/hyperlane-monorepo/pull/1 | Done |
| Create Dusk-specific Hyperlane contract/tooling repo | `dusk-network/hyperlane-dusk`, PR #1: https://github.com/dusk-network/hyperlane-dusk/pull/1 | Done |
| Put local `~/projects/hyperlane/dusk` under git and push | Dusk repo branch `feat/dusk-hardening-v2`, current pushed head `8329fb1`, latest implementation/test evidence commit `e4d3f2a` | Done |
| Re-port Hyperlane integration on current upstream main | Monorepo branch `feat/dusk-support-v2`; rebase/check evidence commit `f0df7aa`; branch contains the clean Dusk chain crate/config/protocol integration commit `feat: re-port Dusk Hyperlane agent support` and is based on upstream `f758a706` | Done for internal review |
| Keep work off `hyperlane-xyz` origin/main | Work is in Dusk forks and Dusk feature branches; both PRs target Dusk org repos | Done |
| Reapply integration in reviewable slices | Monorepo PR includes Dusk chain crate wiring, parser/config/protocol support, relayer/validator/scraper/lander checks; Dusk PR separates contract/tooling/security/test evidence | Done for internal review |
| Harden Mailbox, MerkleTreeHook, ValidatorAnnounce, MessageIdMultisigISM, IGP/protocol-fee, WarpDrc20, WarpDrc20Collateral, and WarpNative | `SECURITY_REVIEW.md` lists issue-by-issue fixes, assumptions, Solidity deviations, and file-level changes | Done for internal review |
| Address arithmetic, replay/domain separation, malformed metadata, duplicate delivery, admin paths, dirty redeploys, and token accounting | `SECURITY_REVIEW.md`; `TEST_REPORT.md`; 67 integration tests; E2E/fault-injection runs listed below | Done for internal review |
| Avoid runtime `todo!`, `unimplemented!`, and direct `panic!` paths | `TEST_REPORT.md` records scan command and result for `rg -n "todo!|unimplemented!|panic!" contracts types data-driver dusk-tx e2e wasm-bindings demo -g '!target'` with no matches in scoped production paths | Done |
| Avoid leaking Dusk secrets through committed configs and process argv | Demo/E2E configs use ignored local files and `/tmp` artifacts; `dusk-tx` supports password file/env lookup and stdin raw-key loading; demo scripts avoid Dusk consensus passwords in CLI argv; `SECRET_HANDLING.md` and `make secret-hygiene` provide source/artifact guardrails; `SECURITY_REVIEW.md` keeps production signer/config handling as a release gate | Gated |
| Dusk VM contract/type checks | `make all`; `cargo test -p hyperlane-dusk-types` -> 28 passed; `cargo test -p hyperlane-dusk-integration-tests` -> 67 passed; `cargo test -p dusk-tx` -> 3 passed | Done |
| Hyperlane Rust agent checks | From monorepo `rust/main`: `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander` -> passed after rebasing onto upstream `f758a706` | Done |
| Local EVM <-> Dusk with TestMock/null-style ISM | Clean Rusk run `1778520709`: bidirectional E2E passed | Done |
| Local EVM <-> Dusk with MessageIdMultisigISM | Clean Rusk run `1778521018`: bidirectional E2E with validator/checkpoint passed | Done |
| Bidirectional DUSK/wDUSK bridge through relayer/validator, no manual process calls | `TEST_REPORT.md` records relayer/validator-driven EVM -> Dusk and Dusk -> EVM flows for TestMock and MessageIdMultisig | Done |
| Stress/reliability: relayer restart/backlog | Clean Rusk run `1778524643`: 50 transfers each direction; clean soak runs `1778528713`, `1778529143`, and `1778530903`; the latest run completed 3 cycles, 20 transfers each direction per cycle, in 3102 seconds | Done, hours-long soak still reasonable before production claims |
| Stress/reliability: dirty redeploy guard | Clean Rusk run `1778522551` | Done |
| Stress/reliability: validator delay/checkpoint backoff | Clean Rusk run `1778522740` | Done |
| Stress/reliability: metadata corruption | Clean Rusk run `1778523252` | Done |
| Stress/reliability: low signer balance | Clean Rusk run `1778523531` | Done |
| Stress/reliability: origin RPC failure | Clean Rusk run `1778523772` | Done |
| Stress/reliability: destination RPC failure | Clean Rusk run `1778524014` | Done |
| Stress/reliability: duplicate relayer/no double-delivery | Clean Rusk run `1778524257` | Done |
| Prepare internal Dusk PR for contracts/tooling/tests/audit notes | Dusk PR #1 is open and mergeable at `8329fb1`; body and latest handoff comment include current evidence and remaining gates | Done |
| Prepare internal Dusk PR for Hyperlane agent/protocol integration | Monorepo PR #1 is open and mergeable at `30652aa`; body and latest handoff comment include companion Dusk evidence and remaining gates | Done |
| Prepare upstream Hyperlane draft PRs only after internal review | Not started by design; PR bodies explicitly state upstream prep waits for internal Dusk review | Gated |

## Evidence Index

| Evidence | Location |
|---|---|
| Security assumptions, fixes, Solidity deviations, reviewer decisions | `SECURITY_REVIEW.md` |
| Secret handling policy, signer custody proposal, and source/artifact guardrail | `SECRET_HANDLING.md`, `PRODUCTION_SIGNER_POLICY.md`, `scripts/secret-hygiene-check.sh`, `make secret-hygiene` |
| CI/repro runner proposal | `CI_REPRO_STRATEGY.md`, `.github/workflows/manual-repro-check.yml`, `make repro-check-agent` |
| Commands, clean Rusk commit, run IDs, artifact paths, pass/fail notes | `TEST_REPORT.md` |
| Repeatable local E2E scripts | `demo/e2e-agents.sh`, `demo/e2e-relayer-restart-stress.sh`, `demo/e2e-soak-restart-stress.sh`, `demo/e2e-*.sh` |
| Demo and E2E command documentation | `demo/README.md` |
| Dusk contract/tooling PR | https://github.com/dusk-network/hyperlane-dusk/pull/1 |
| Hyperlane monorepo integration PR | https://github.com/dusk-network/hyperlane-monorepo/pull/1 |
| Upstream compatibility review | `dusk-network/hyperlane-monorepo` `docs/dusk-upstream-compatibility-review.md` |
| Production sign-off tracker | https://github.com/dusk-network/hyperlane-dusk/issues/2 |

## Remaining Gates

1. Dusk reviewers must accept or change the open production review decisions in
   `SECURITY_REVIEW.md`:
   - Mailbox transfer-contract sender resolution.
   - Immutable `registered_accounts` registration.
   - No admin drain for native/collateral pending escrow.
2. Longer soak testing remains recommended before production claims if Dusk
   wants an hours-long release gate. Current evidence includes a 50-transfer
   restart/backlog run, 1-cycle and 2-cycle clean-Rusk soak runs, and a
   3102-second 3-cycle clean-Rusk soak with 20 transfers each direction per
   cycle.
3. Production secret handling needs an operational/CI sign-off. Current scripts
   keep dev configs in ignored files and `/tmp`, Dusk consensus passwords are
   no longer passed to `dusk-tx` in process argv, and `make secret-hygiene`
   provides source/artifact guardrails. `PRODUCTION_SIGNER_POLICY.md` records
   the current `duskKey` raw-key implementation constraint plus acceptable v1
   custody choices for Dusk review. Production deployment must still accept or
   replace that signer custody and CI artifact policy.
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
   self-hosted workflow that preserves the private dependency checkout layout.
   `CI_REPRO_STRATEGY.md` records the proposed runner labels, checkout layout,
   read-only token scope, artifact policy, and promotion path. The workflow
   still needs a Dusk runner/secret decision before it can replace local
   evidence.
7. The remaining production sign-off work is tracked in
   https://github.com/dusk-network/hyperlane-dusk/issues/2.

## Completion Decision

The revival branch is ready for internal Dusk review, but the overall objective
is not complete. The remaining gates above must be closed before claiming
production readiness or preparing upstream Hyperlane PRs.
