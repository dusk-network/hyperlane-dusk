# Dusk Hyperlane Test Report

Date: 2026-05-11
Last updated: 2026-07-21

This report captures the current local verification for the revived Dusk
Hyperlane branches. It is not a production-readiness sign-off; the remaining
production-review gates and useful follow-up test areas are listed at the end.

## 2026-07-21 Final Clean-Current-Rusk Gate

The stacked withdrawal implementation at
`183b56a875e5c2962ef621937258b8e497baef2a`, based on the final contract
implementation `4d8f5da013d56e5d3fa036ab924de6a6729b5f4f`, was reproduced in a detached
clean layout against Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` (Dusk Core/VM 1.7.1).

The gate passed:

- all 12 contract WASM builds and production contract/type clippy;
- 29 type tests and 100 VM integration tests;
- 7 data-driver tests and its release WASM build;
- 18 `dusk-tx` tests;
- the standalone E2E host build; and
- tracked-source secret hygiene.

The durable local log is
`/tmp/hyperlane-dusk-withdrawal-repro-183b56a.log`, SHA256
`4b70209aeddd30fe161a71d5b83110d3b7c5a7de9a42d02e6b4e1d1fcb2f2e69`.
The later `137ce09e19ffd30a36027ba417ebf1992521613f` commit changes only the live E2E
harness: it withdraws one unit from a route's actual contract-keyed Mailbox
credit, asserts the exact decrement, and then requires the remaining credit to
fund the later bidirectional route exercise.

That harness passed from clean state with agent implementation
`37e24eed2c7ad7aed63e3fa033d1fe8a28355ec0` in both topologies:

- TestMock run `1784592169` delivered synthetic, native, and collateral routes
  in both directions, asserted exact custody/allowance changes, confirmed the
  live withdrawal, and observed successful Rusk process simulation. Harness
  log SHA256:
  `1cac650a1ba192eb314984c5003169ee4767b4f840ee6f71ecf0a304efcaf190`.
- MessageIdMultisig run `1784592942` repeated that matrix through a real
  validator, signed checkpoints, threshold metadata, and successful Rusk
  process simulation. Harness log SHA256:
  `802d61c3df233ab25dcfbebc58d8b6facf2f5282fbb86c4981e738a9e643363c`.

The associated relayer logs are
`/tmp/hyperlane-relayer-testMock-1784592169.log` (SHA256
`e6a96a464a9497fb39b759fe2037cddbb5cbfac66bd4630a971871eb14557e7c`) and
`/tmp/hyperlane-relayer-messageIdMultisig-1784592942.log` (SHA256
`e098460937f2c151b10e3d1ba703169d7af31253b14c25e6dad18cdba06d479a`).
The multisig validator log SHA256 is
`e8e108a242221d2f0f4ba9ad011d30a41de0dfdb64cfd0621b0cc93d1abc073c`.

## 2026-07-20 Final Base-PR Compatibility Validation

The base implementation commit
`4d8f5da013d56e5d3fa036ab924de6a6729b5f4f` was reproduced from a detached,
clean worktree against current clean Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` (Dusk 1.7.1). The exact command
was:

```bash
RUSK_DIR=/tmp/rusk-private-hyperlane-current-20260720 \
  bash scripts/local-repro-check.sh
```

Durable local log: `/tmp/hyperlane-dusk-base-repro-4d8f5da.log` (SHA-256
`28bfda04709a62d12ebaa26a2351707a638408d0b2ce12b65e5bb5c97b58f948`).

Result:

- all 12 contract WASMs built and the production contract/type clippy surface
  passed;
- `hyperlane-dusk-types`: 29 passed, 0 failed;
- `hyperlane-dusk-integration-tests`: 94 passed, 0 failed;
- `dusk-tx`: 16 passed, 0 failed;
- `hyperlane-dusk-data-driver`: 5 passed, 0 failed;
- the standalone E2E operator crate compiled; and
- tracked-source secret hygiene passed.

The additional coverage proves aggregate synthetic pending-supply reservation,
coherent multisig policy queries, bounded Merkle/IGP pages, strict simulation
response parsing, exact-hash preservation for ambiguous propagation and
confirmation, full saved-topology validation, generated-agent policy binding,
and fail-closed strict branch-protection checks. The repository-level
fail-closed self-test also passed in the primary clean worktree.

## 2026-07-20 Post-Deep-Review Contract Validation

The custody/runtime remediation commit
`d4a429e1c491bc2e74dad9360eda8015b138792b` was validated in a detached clean
layout against clean Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` (Dusk 1.7.1). This avoids the stale
Dusk 1.6 path dependencies in the developer checkout.

Results:

- All 12 contract WASMs built from source.
- `make clippy-contracts` passed for the production contract/type surface.
- `hyperlane-dusk-integration-tests`: 90 passed, 0 failed. The suite includes
  live custody reservation for pending DRC20 collateral, rejection of an
  immediate delivery that would consume that reserve, zero-mailbox route
  rejection, and the 255-validator structural bound.
- `hyperlane-dusk-types`: 29 passed, 0 failed.
- `hyperlane-dusk-data-driver`: 5 passed, 0 failed, including both Mailbox and
  hook `quote_dispatch` ABI shapes plus current accounting/provenance queries.
- `dusk-tx`: 12 passed, 0 failed, including bounded secret-key stdin, bounded
  RUES bodies, and exact-hash transaction observation.
- `make secret-hygiene archive-hygiene archive-hygiene-test
  fail-closed-self-test` passed in the primary checkout.

This is local clean-layout evidence, not a replacement for the still-blocked
private-runner status check or a fresh bidirectional agent E2E after both
stacked PR heads are finalized.

## 2026-07-20 Dispatch-Credit Withdrawal Validation

The focused stacked branch `feat/dispatch-credit-withdrawal` was validated at
implementation anchor `8064476efa30126186971316f72b2646f0c3b7d2` after rebasing
onto hardened base `b46fda9265e3381203962a65c15b697271fd5dff`, against clean
detached current Rusk `origin/master` at
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` (Dusk Core/VM 1.7.1).

An isolated compatible checkout layout passed:

- all 12 contract WASM builds;
- the production contract/type WASM clippy surface;
- 29 type tests;
- 95 VM integration tests;
- 6 data-driver decoder tests; and
- 13 `dusk-tx` tests.

The rebase preserved the base branch's stricter 256-KiB status-response bound,
exact-hash transaction confirmation, immediate first observation, and absolute
deadline while layering the withdrawal surface on top. A pre-rebase backup is
kept at `backup/dispatch-credit-pre-b46fda9`. The earlier clean repro caught two
overlapping `mod tests` blocks in the data driver; they were consolidated and
the full contract/type/VM portion plus every remaining driver, CLI, release-
WASM, and hygiene layer passed at the implementation anchor above.

The added VM cases prove that permissionless third-party funding does not grant
withdrawal authority, another Moonlight payer cannot withdraw the named
payer's credit, zero withdrawals fail, partial and full withdrawals reduce
both accounting and native custody exactly, and only the configured owner of
each WarpDrc20, WarpNative, or WarpDrc20Collateral route can withdraw that
route's contract-keyed credit. Identity/invalid BLS recipients fail before
credit or custody changes. The successful withdrawal's actual VM receipt event
is decoded through `HyperlaneDataDriver`, joining the contract emission and
explorer decoder in one test. The CLI separately proves valid, prefixed,
wrong-length, and identity recipient-key handling, and exposes an explicit
treasury option while retaining the signer as its default.

The adversarial review found and corrected one tooling defect before handoff:
`fund-dispatch` and `withdraw-dispatch` had treated signer nonce advancement as
successful execution. Dusk spends the Moonlight nonce even when a contract
call is rejected. Both commands now poll Rusk's GraphQL transaction record by
the exact transaction hash and return an error when `SpentTransaction.err` is
set. The confirmation path checks immediately, uses an absolute 60-second
deadline, retries transient observation errors while preserving the hash,
and caps status responses at 256 KiB; generic raw contract calls use the same
execution-success boundary. Focused tests cover exact-hash query construction,
success/failure/not-found parsing, GraphQL error rejection, malformed and
oversized responses, retry/error branches, nonce exhaustion, and the VM
invariant that a rejected withdrawal still advances the Moonlight nonce.

An initial compatibility probe against historical clean Rusk
`c0c64db4659500d077bb253ad13acba0e347d3fc` built every contract and passed
contract clippy, but resolved Dusk VM 1.6 and stopped before integration tests
because that older VM lacks the current host-query and execution-config API.
It is not validation evidence for this branch; the current-Rusk run above is.

## 2026-07-20 Remediation Validation (Current)

This section supersedes “current” wording in the May evidence and the earlier
same-day synthetic-only refresh. Historical runs below remain useful only for
the exact commits they name.

| Component | Validated reference |
|---|---|
| Dusk contracts/tooling | `feat/dusk-hardening-v2`; validated runtime anchor `e8d6596f93c7cb90e87a76ee76126a23608339b5`, including the caller, fee-custody, aggregation-hook, current-DRC20, deployment, and route-matrix changes described in `REASSESSMENT_2026-07-20.md` |
| Hyperlane agent integration | `feat/dusk-support-v2` at `a931f75b3d23d2e15e75f2e064470a1a01289abb`, rebased on upstream `197b1e0d1a7b7ee5539e9ad38a02a23a7eb0a0b3` |
| Rusk | Clean `bc281d2cd1e789db92e99bc59849c92363524e37`, with a fresh state archive and that checkout's consensus keys |
| Forge | `d1e39a16ad5e2cd0675c7aafa6e2c459310bcb1a` (Forge 0.3.0) |

Results:

- `cargo test -p hyperlane-dusk-integration-tests`: 98 passed, 0 failed at the
  reassessed withdrawal head; the reassessed base branch has 93 tests.
- All 12 contract WASMs build and the production contract/type clippy surface
  passes.
- The VM suite validates the shared Moonlight/contract owner model, rejects
  unauthorized admin calls and spoofed value callbacks, and proves exact
  ProtocolFee/IGP native custody and aggregate-hook forwarding.
- The DRC20 collateral route uses the current typed Dusk ABI and allowance
  model. VM coverage proves approval consumption and exact contract custody.
- Fresh TestMock and MessageIdMultisig live agent cases each delivered
  WarpDrc20, WarpNative, and WarpDrc20Collateral in both directions.
- The live native route asserted exact DUSK transfer-contract custody after
  lock and partial return. The live collateral route asserted exact account
  debit/credit and route custody after lock and partial return.
- Each case observed exactly three ProtocolFee collections for the three
  outbound Dusk dispatches. The multisig case used the real validator and
  checkpoint-signature metadata path.

The harness rejects split-Rusk execution before services start. The validated
runs compiled the contracts from the same clean Rusk checkout used by the node.
It now also rebuilds or freshness-checks the release CLI and every contract WASM
instead of accepting an existing, potentially stale artifact. The live cases
executed agent head `eaa43c3c4decdf007085b19ec6b7d586f150457e`; the Dusk-agent covered paths
are byte-identical at the final rebased head, whose six affected Rust packages
also pass `cargo check`.

The required GitHub check now has distinct phases. Pull-request events run a
pre-merge gate that does not require the current PR to have already merged or
self-approved. `workflow_dispatch` retains the full production audit, including
human decision issues and privileged repository/runner/secret visibility. The
same hardening also makes completed failed dependency checks fail the guard;
previously it counted only missing or still-running checks.

## Repository State

| Component | Repository | Branch | Evidence commit |
|---|---|---|---|
| Dusk contracts/tooling | `dusk-network/hyperlane-dusk` | `feat/dusk-hardening-v2` | Latest clean-layout repro run `1778751867` tested Dusk source ref `2dd0d227cf0c33033cd9206151c1cbca6cddfffb`, monorepo `3b7d9f64d7d9465eaa868770d970243d98bc53c6`, upstream base `a8c9430c82f0b66faf4231828f798dcbec95dfab`, and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`; durable archive `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-clean-repro-current-head-1778751867.tgz`, SHA256 `9e08ce22389f4a209d3d1ed79aa90de8d5384ce77ca7c142019c3264b799b7e7`; contained log SHA256 `df88d712f4bfada0b1958a9b4d7c1b96b0b2ec4754482c524dca91b0c83e738d` |
| Hyperlane agent integration | `dusk-network/hyperlane-monorepo` | `feat/dusk-support-v2` | Latest clean-layout repro run `1778751867` tested monorepo `3b7d9f64d7d9465eaa868770d970243d98bc53c6` after rebasing onto upstream `a8c9430c82f0b66faf4231828f798dcbec95dfab`, with Dusk source ref `2dd0d227cf0c33033cd9206151c1cbca6cddfffb` and clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`; it passed the Hyperlane Rust agent check through `make repro-check-agent` |
| Post-repro Dusk agent, CI guard, generated-config cleanup, and checkout hardening | `dusk-network/hyperlane-monorepo` + `dusk-network/hyperlane-dusk` | `feat/dusk-support-v2` + `feat/dusk-hardening-v2` | Monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20` after rebasing onto upstream `2b7db706023806b36a57e446205ae443537ae9ec`; removed Dusk agent `expect(...)` paths from RUES client construction and rkyv serialization, made the Dusk base-provider builder fallible, hardened `.github/workflows/dusk-agent-gate.yml`, guarded inherited Dusk-fork workflows, and refreshed upstream-base notes. Dusk-side RUES clients in `dusk-tx` and `e2e` now propagate HTTP client build errors instead of panicking. Dusk `8d3704e8f5a3ab0976b97fc3a68319e112e8affc` additionally cleans generated Hyperlane agent config files from E2E wrappers and extends `make secret-hygiene` to require that cleanup for future `gen-agent-configs.sh` consumers. Dusk `ef8ee43cd99569299b9744b498ac1bbac69950bc` and monorepo `515fab074024271935bc7795604dbb4f0823a937` update Dusk-controlled workflows to `actions/checkout@v6` after verifying the tag through the GitHub API. Latest clean-layout repro run `1778695627` passed the Dusk contract/type/tooling checks plus the Hyperlane Rust agent check against those heads. Live monorepo PR head and `monorepoCoveredPathDelta` are checked through live PR status and `make gate-status-fresh` rather than pinned in this row. |
| Validated runtime clean-Rusk E2E refresh | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Dusk PR head `fd2ec8fe996dd28f85259e6aebd8db25182c20de`, latest covered Dusk implementation ref `93ed3b07b26d8784610d2ba754a851490de15e21`, monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base `2b7db706023806b36a57e446205ae443537ae9ec`, clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`; TestMock E2E run `1778672800`; MessageIdMultisig E2E run `1778673269`; evidence comment https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440708799 |
| Post-rebase clean-Rusk E2E evidence | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Dusk `b1ccdc9d1e7797bba4939405200aa4cc5aff2ea8`; monorepo `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`; upstream base `c6bce706316206ac7b5652155c9ea92e96f78c39`; clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`; TestMock E2E run `1778609411`; MessageIdMultisig E2E run `1778609697` |
| Dependency-remediated clean-Rusk E2E evidence | `dusk-network/hyperlane-dusk` + `dusk-network/hyperlane-monorepo` | `feat/dusk-hardening-v2` + `feat/dusk-support-v2` | Dusk dependency-remediation worktree with `Cargo.toml`/`Cargo.lock` updates documented below; monorepo `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`; upstream base `c6bce706316206ac7b5652155c9ea92e96f78c39`; clean Rusk `c0c64db4659500d077bb253ad13acba0e347d3fc`; TestMock E2E run `1778613709`; MessageIdMultisig E2E run `1778613956` |
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
- Live monorepo PR status on 2026-05-13 after the checkout-v6 workflow
  hardening at `515fab074024271935bc7795604dbb4f0823a937` shows one expected
  completed failure (`Dusk agent cargo check`) until `DUSK_ORG_READ_TOKEN` is
  provisioned and no queued checks. Fresh CI evidence is recorded in #8 at
  https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4445189878;
  the local job log is `/tmp/dusk-agent-cargo-check-75849383145.log`. The
  earlier 52 queued inherited upstream checks from `test`, `rust`, and
  `Rebalancer E2E Tests` are superseded by monorepo
  `48dcc0c87efc12584904d841d5088c1ae4acef20` and later Dusk-fork workflow
  guards; the `9050143` to `515fab0` covered-path delta is now covered by
  checkout-v6 repro run `1778695627`.
- Dusk commit `cc3d75a2688d4b9d1aed9b7bfc5c87547802aa74` extends
  `scripts/github-review-hygiene.sh` so stale Dusk handoff heads and old Dusk
  check URLs from the prior `54e56fa` gate refresh fail future review-hygiene
  scans. Dusk PR #1 then reported `Dusk review policy gate` success at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25804304358/job/75802609838
  and the expected `Production readiness guard` failure at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25804304353/job/75802609692.
- Dusk commit `13eaca26336820e27b862ecf3cfccc564a47e751` extends
  `scripts/github-review-hygiene.sh` so active PR/issue handoff text must name
  the full dispatcher validation command:
  `actionlint .github/workflows/manual-repro-check.yml .github/workflows/manual-repro-dispatcher-gate.yml .github/workflows/dusk-review-policy-gate.yml`.
  The same guard rejects the stale production-readiness run
  `25797921067`/job `75779855203`. The PR #1 post-push status had the expected
  shape: review policy success plus production-readiness failure while blockers
  remained. Moving PR check URLs are now read from live PR check rollups instead
  of retained in this historical report row. Local `make review-hygiene` passed
  with export
  `/tmp/hyperlane-review-export-1778697862`, and
  `make completion-audit-status` passed with Dusk active ref
  `13eaca26336820e27b862ecf3cfccc564a47e751` and monorepo active ref
  `515fab074024271935bc7795604dbb4f0823a937`.
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
  At that stage, the clean-Rusk E2E evidence above remained evidence for the
  pre-rebase monorepo ref `a44020dc998b7fe868254a5d1a349b9eb8ded899`; later
  post-rebase E2E runs `1778609411` and `1778609697` covered the final
  post-rebase monorepo head.
- After upstream Hyperlane advanced to
  `7a362a093d622b69d6c55d47992c9490ec33fb1a`, the monorepo PR branch was
  rebased again and pushed. The upstream change was TypeScript infra/config
  only (`typescript/infra/config/environments/mainnet3/agent.ts`), and the
  Dusk branch changes after the clean-layout repro monorepo ref are Dusk
  upstream compatibility/PR-plan docs. `git diff --check`, the Dusk agent
  placeholder scan, and the targeted Rust agent check passed after the rebase.
- After upstream Hyperlane advanced to
  `c6bce706316206ac7b5652155c9ea92e96f78c39`, the monorepo PR branch was
  rebased again and pushed at
  `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`. The upstream change reduces a
  Rust relayer CCIP-read retry interval. Dusk still returns explicit
  unsupported errors for CCIP-read ISMs in this branch, so this does not expand
  the supported Dusk behavior. `git diff --check`, the Dusk agent placeholder
  scan, and
  `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p scraper -p lander`
  passed after the rebase. Clean-layout repro run `1778607202` then passed on
  Dusk source ref `836ee7d8d8e95152b3daaeebbc3fb56b0cc8e253` and monorepo
  `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`.
- Post-rebase clean-Rusk E2E runs `1778609411` and `1778609697` then
  passed at Dusk `b1ccdc9d1e7797bba4939405200aa4cc5aff2ea8`, monorepo
  `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`, upstream base
  `c6bce706316206ac7b5652155c9ea92e96f78c39`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. These runs cover the
  post-rebase TestMock and MessageIdMultisig paths in both directions.
- After dependency advisory remediation, clean-Rusk E2E runs `1778613709` and
  `1778613956` passed with the updated `Cargo.toml`/`Cargo.lock` dependency
  graph, monorepo `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`, upstream base
  `c6bce706316206ac7b5652155c9ea92e96f78c39`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. These runs cover the
  TestMock and MessageIdMultisig paths in both directions after the Rust
  dependency updates.
- The monorepo PR later moved to
  `006e49dd7041097384683a78b1c1973c83e90de8` with a documentation-only
  advisory-scope note in `docs/dusk-upstream-compatibility-review.md`. That
  note records that all 173 open Dependabot alerts visible in the monorepo fork
  are npm alerts in manifests outside the Dusk Rust integration diff, and that
  no open non-npm/Cargo alerts were returned by the Dependabot API.
- Historical clean-layout repro run `1778615349` passed at Dusk
  `016eaa89e1afce0ef9a7534fe285d9aa16e26183`, monorepo
  `006e49dd7041097384683a78b1c1973c83e90de8`, upstream base
  `c6bce706316206ac7b5652155c9ea92e96f78c39`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`.
- Historical live-head clean-layout repro run `1778669495` passed at Dusk
  `0be9fbfa91ef39ecd912c79d360d036530fb524d`, monorepo
  `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`.
- RUES hardening clean-layout repro run `1778671118` passed at Dusk
  `93ed3b07b26d8784610d2ba754a851490de15e21`, monorepo
  `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`.
- Generated agent-config cleanup clean-layout repro run `1778683232` passed at
  Dusk `8d3704e8f5a3ab0976b97fc3a68319e112e8affc`, monorepo
  `9050143c1ef12f76d117ee97effa79da8df3e334`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene with generated agent-config cleanup
  enforcement, and the Hyperlane Rust agent check for `hyperlane-dusk`,
  `hyperlane-base`, `validator`, `relayer`, `scraper`, and `lander`.
- Validated runtime clean-Rusk E2E runs `1778672800` and `1778673269` passed at
  Dusk PR head `fd2ec8fe996dd28f85259e6aebd8db25182c20de`, latest covered
  Dusk implementation ref `93ed3b07b26d8784610d2ba754a851490de15e21`,
  monorepo `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. TestMock covered relayer-driven
  EVM -> Dusk and Dusk -> EVM delivery with the null-style ISM path.
  MessageIdMultisig covered the validator/checkpoint metadata path plus
  relayer-driven delivery in both directions.
- The monorepo PR later moved to
  `9e8c7abf054fcf1193abc81a2d984fe59b08eb16` with a focused Dusk agent
  panic-path hardening slice. RUES client construction and rkyv serialization
  now return errors instead of using `expect(...)`; the Dusk base-provider
  builder propagates those errors; and `.github/workflows/dusk-agent-gate.yml`
  uses `git grep` instead of runner-local `rg` to scan
  `rust/main/chains/hyperlane-dusk/src` for `todo!`, `unimplemented!`,
  `panic!`, and `expect\(`. Local `cargo check -p hyperlane-dusk -p
  hyperlane-base -p validator -p relayer -p scraper -p lander` passed in
  1m51s, incremental `cargo check -p hyperlane-dusk -p hyperlane-base` passed
  in 44.55s, `actionlint .github/workflows/dusk-agent-gate.yml` passed, and
  the local `git grep` panic/placeholder scan reported no Dusk agent runtime
  matches. The CI `Dusk review policy gate` passed for this head; the CI
  `Dusk agent cargo check` passed the hardened scan and still fails at the
  expected private companion-repo preflight until `DUSK_ORG_READ_TOKEN` is
  provisioned.
- After upstream Hyperlane advanced to
  `2b7db706023806b36a57e446205ae443537ae9ec`, the monorepo PR branch was
  rebased again and pushed at
  `f90ca7820bf73b9c99eac807fac47e5b9a8ed9e8`. The upstream change is
  TypeScript infra/config for Citrea/Moonpay warp route config getters. Local
  `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p
  scraper -p lander` passed in 7.60s at this head, workflow `actionlint`
  passed for `dusk-agent-gate.yml`, `rust-docker.yml`, and
  `monorepo-docker.yml`, and the Dusk agent `git grep` panic/placeholder scan
  reported no matches. This head also guards `monorepo-docker.yml` with
  `github.repository_owner == 'hyperlane-xyz'`, matching the existing
  `rust-docker.yml` guard so Dusk-fork PRs do not attempt Hyperlane-org
  Depot/GHCR image publishing. Monorepo head `f90ca7820bf73b9c99eac807fac47e5b9a8ed9e8`
  then refreshed the upstream-plan docs to record this base explicitly.
- The post-rebase clean-Rusk E2E logs were copied into durable local
  handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-live-head-e2e-1778609411-1778609697.tgz`.
  SHA256:
  `3e5303799e0def227988e0ac66db9d021e94bd39df054629137896c5d7b07ccc`.
  `scripts/secret-hygiene-check.sh` passed over both the archive staging
  directory and the tarball.
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
- The clean-layout repro for Dusk source ref `aa27820` was copied into
  durable local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778604592.tgz`.
  SHA256:
  `e74c18e7715c07a653abfb4b3e4ba11c2d9caa682fa0a69eedb25cab4100a110`.
  `scripts/secret-hygiene-check.sh` passed over the repro log, archive staging
  directory, and tarball.
- The previous clean-layout repro for Dusk source ref `836ee7d` was copied into
  durable local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778607202.tgz`.
  SHA256:
  `cc34e8e31abcab6ef3ef05587f38ce7963257586c30375d59b46c25296e0b2a1`.
  `scripts/secret-hygiene-check.sh` passed over the repro log and tarball.
- The live Dusk and monorepo PR heads are checked through GitHub and
  `make gate-status`. Latest clean-layout repro run `1778751867` covers Dusk
  source ref `2dd0d227cf0c33033cd9206151c1cbca6cddfffb` and monorepo ref
  `3b7d9f64d7d9465eaa868770d970243d98bc53c6`. `DUSK_REPRO_COVERED_PATHS`
  includes runtime, tooling, demo, and VM integration test source
  (`contracts types data-driver dusk-tx e2e wasm-bindings demo tests Cargo.toml
  Cargo.lock`) so test-source changes are visible to repro-delta reporting.
  The malformed warp-token VM test slice in `tests/tests/integration.rs` is
  covered by the prior run `1778722626`; the current-head run covers the same
  Dusk runtime/test path set plus the rebased monorepo agent head. The gate
  also compares the live monorepo PR head to the latest clean-layout repro
  monorepo ref and reports `monorepoCoveredPathDelta`.
- A 2026-05-12 dependency advisory remediation pass updated the Dusk repo Rust
  dependency graph for GitHub-reported advisories: `wasmtime` 25.0.3 -> 36.0.9,
  `openssl` 0.10.75 -> 0.10.79, `openssl-sys` 0.9.111 -> 0.9.115,
  `rustls-webpki` 0.103.9 -> 0.103.13, `quinn-proto` 0.11.13 -> 0.11.14,
  `rand` 0.8.5 -> 0.8.6, `rand` 0.9.2 -> 0.9.3, and `keccak` 0.1.5 -> 0.1.6.
  The `lru` package was already at 0.16.4. `wasmtime` remains scoped to the
  `hyperlane-dusk-integration-tests` dev-dependency, while `quinn-proto` and
  `rand` 0.9.3 remain lockfile-only in the active target graph. GitHub
  Dependabot alerts may remain visible until GitHub rescans the pushed
  `Cargo.lock`.
- Dependency remediation logs were copied into durable local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-1778613424.tgz`.
  SHA256:
  `507bd4d4dfc7bfd6c6fd517f3f8d6edc83e662d2b81530e4be933ddd53ac6433`.
  `scripts/secret-hygiene-check.sh` passed over the log directory and tarball.
- Dependency-remediated clean-Rusk E2E logs were copied into durable local
  handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956.tgz`.
  SHA256:
  `36df8403ac20fa2b62e1e69780229222520102457a9d3f24165f6a4dbd924f13`.
  `scripts/secret-hygiene-check.sh` passed over the archive staging directory
  and tarball.
- The latest current-runtime clean-layout repro log was copied into durable local
  handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778615349.tgz`.
  SHA256:
  `5da9c5ff7768b1be227f00a5e4f8a7de2f6418057875ecf8e132bc0926a72a04`.
  `scripts/secret-hygiene-check.sh` passed over the repro log, archive staging
  directory, and tarball.
- The latest current-live-head clean-layout repro log was copied into durable
  local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-live-head-repro-1778669495.tgz`.
  SHA256:
  `3ac4436634a6297cf5200ac1ff1c4b0cf45fcf7b61b05f4d606aaa4f7654ad25`.
  `scripts/secret-hygiene-check.sh` passed over the repro log, archive staging
  directory, and tarball.
- The RUES hardening clean-layout repro log was copied into durable local
  handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-rues-hardening-repro-1778671118.tgz`.
  SHA256:
  `7a4a981422718ca081ce87c09af52ca4f2a2f0893c7ffcee3e19484739f8c460`.
  `scripts/secret-hygiene-check.sh` passed over the repro log, archive staging
  directory, and tarball.
- The generated agent-config cleanup clean-layout repro log was copied into
  durable local handoff archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-agent-config-cleanup-repro-1778683232.tgz`.
  SHA256:
  `d7459059cf413d6d6a7c404248429e760aca0a7fd4ab784a5cb068fb63c7df0b`.
  `scripts/secret-hygiene-check.sh` passed over the repro log and
  `scripts/archive-hygiene-check.sh` passed over the evidence archive
  directory after the archive was added.
- The current-runtime clean-Rusk E2E logs were copied into durable local handoff
  archive
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-e2e-1778672800-1778673269.tgz`.
  SHA256:
  `2fab09b0a4196129bbc32084cc3664ed81c5f78222cde75d47fdb314a1c825f8`.
  `scripts/secret-hygiene-check.sh` passed over the E2E log staging directory
  and tarball.
- Archive hygiene audit on 2026-05-13: `make archive-hygiene` extracted all
  durable `.tgz` evidence archives in
  `/home/hein_/projects/hyperlane/.codex-backups` into a temporary directory
  and scanned the extracted contents with
  `bash scripts/secret-hygiene-check.sh "$scan_root"`. The extracted-content
  scan covered repro, E2E, dependency-remediation, and soak handoff archives
  and found no secret-like filenames or signer/password command text. The
  wrapper verifies the expected SHA256 for the latest checkout-v6 repro
  archive, rejects unsafe archive member paths, non-regular/non-directory
  archive entries, and extracted symlinks or special files before treating
  archive contents as reviewer evidence.
- `make archive-hygiene-test` covers the scanner regression cases: safe archive
  acceptance plus rejection for archive SHA256 mismatch, traversal members,
  symlink members, and secret-bearing archive contents.
- `make archive-hygiene` wraps this command shape:

```bash
scan_root="$(mktemp -d -t hyperlane-archive-hygiene.XXXXXX)"
trap 'rm -rf "$scan_root"' EXIT
for archive in /home/hein_/projects/hyperlane/.codex-backups/*.tgz; do
  name="$(basename "$archive" .tgz)"
  dest="$scan_root/$name"
  mkdir -p "$dest"
  tar -xzf "$archive" -C "$dest"
done
bash scripts/secret-hygiene-check.sh "$scan_root"
```

- `make archive-hygiene` now verifies the expected SHA256 for the latest
  checkout-v6 repro archive before extracting evidence archives. It now uses
  tracked manifest `EVIDENCE_ARCHIVES.sha256` to verify every durable `.tgz`
  evidence archive in `/home/hein_/projects/hyperlane/.codex-backups` when the
  manifest is present, rejects unsafe manifest paths, rejects unsafe fallback
  `ARCHIVE_EXPECTED_SHA256S` paths, and fails if the manifest archive set
  differs from the local evidence archive directory. The
  `make archive-hygiene-test` self-test includes `sha-mismatch`,
  `expected-sha-unsafe-path`, `expected-sha-non-archive`,
  `manifest-unsafe-path`, and `manifest-missing-archive` fixtures that fail
  closed on an incorrect archive hash, unsafe expected-SHA path, non-`.tgz`
  expected-SHA name, unsafe manifest entry, or unmanifested archive. A
  post-secret-guard `make review-gates` run against Dusk head
  `28f834501f0f32b0d7333b5a1831059eb80fd4d1` and monorepo head
  `515fab074024271935bc7795604dbb4f0823a937` passed with review-hygiene
  export `/tmp/hyperlane-review-export-1778709581` and dispatcher merge-order
  smoke log `/tmp/hyperlane-merge-order-logs.Vqk0sV`.

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
make archive-hygiene-test
make archive-hygiene
make dependency-alert-status
make completion-audit-status
make gate-status
make gate-status-fresh
make review-gates
make production-readiness-guard
make repro-check-agent
bash scripts/local-repro-check.sh --agent-check
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-current-head-1778604592 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-current-head-1778604592.log
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-current-head-1778607202 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-current-head-1778607202.log
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-current-head-1778615349 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-current-head-1778615349.log
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-current-live-head-1778669495 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-current-live-head-1778669495.log
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-rues-hardening-1778671118 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-rues-hardening-1778671118.log
HYPERLANE_DUSK_REPRO_WORKDIR=/tmp/hyperlane-dusk-repro-agent-config-cleanup-1778683232 \
  RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  make repro-check-agent 2>&1 | tee /tmp/hyperlane-dusk-repro-agent-config-cleanup-1778683232.log
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  bash demo/e2e-agents.sh --only testMock --timeout 300 \
  2>&1 | tee /tmp/hyperlane-current-head-e2e-testMock-1778672800.outer.log
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  bash demo/e2e-agents.sh --only messageIdMultisig --timeout 300 \
  2>&1 | tee /tmp/hyperlane-current-head-e2e-messageIdMultisig-1778673269.outer.log
cargo update -p openssl --precise 0.10.79
cargo update -p rustls-webpki --precise 0.103.13
cargo update -p quinn-proto --precise 0.11.14
cargo update -p rand@0.8.5 --precise 0.8.6
cargo update -p rand@0.9.2 --precise 0.9.3
cargo update -p keccak --precise 0.1.6
cargo update -p wasmtime --precise 36.0.7
LOG_DIR=/tmp/hyperlane-dusk-dependency-remediation-1778613424
cargo test -p hyperlane-dusk-types 2>&1 | tee "$LOG_DIR/types.log"
cargo test -p dusk-tx 2>&1 | tee "$LOG_DIR/dusk-tx.log"
cargo test -p hyperlane-dusk-integration-tests 2>&1 | tee "$LOG_DIR/integration.log"
cargo check -p hyperlane-dusk-e2e -p dusk-tx 2>&1 | tee "$LOG_DIR/e2e-dusk-tx-check.log"
make secret-hygiene 2>&1 | tee "$LOG_DIR/secret-hygiene.log"
cargo tree -i wasmtime
cargo tree -i openssl
cargo tree -i rustls-webpki
cargo tree -i keccak
cargo tree -i rand@0.8.6
cargo tree -p lru
cargo tree --target all -i quinn-proto || true
cargo tree --target all -i rand@0.9.3 || true
bash scripts/secret-hygiene-check.sh "$LOG_DIR"
tar -C /tmp -czf /home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-1778613424.tgz \
  hyperlane-dusk-dependency-remediation-1778613424
sha256sum /home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-1778613424.tgz
bash scripts/secret-hygiene-check.sh \
  /home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-1778613424.tgz
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  bash demo/e2e-agents.sh --only testMock --timeout 300 \
  2>&1 | tee /tmp/hyperlane-dependency-remediation-e2e-testMock-1778613709.outer.log
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
  bash demo/e2e-agents.sh --only messageIdMultisig --timeout 300 \
  2>&1 | tee /tmp/hyperlane-dependency-remediation-e2e-messageIdMultisig-1778613956.outer.log
bash scripts/secret-hygiene-check.sh \
  /tmp/hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956
tar -C /tmp -czf \
  /home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956.tgz \
  hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956
sha256sum \
  /home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956.tgz
bash scripts/secret-hygiene-check.sh \
  /home/hein_/projects/hyperlane/.codex-backups/hyperlane-dusk-dependency-remediation-e2e-1778613709-1778613956.tgz
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
- Dependency remediation run `1778613424`: passed `cargo test -p
  hyperlane-dusk-types` with 28 tests, `cargo test -p dusk-tx` with 3 tests,
  `cargo test -p hyperlane-dusk-integration-tests` with 70 tests,
  `cargo check -p hyperlane-dusk-e2e -p dusk-tx`, and `make secret-hygiene`.
  Logs are in `/tmp/hyperlane-dusk-dependency-remediation-1778613424` and the
  archive listed above. The only warnings were existing dead-code warnings in
  the integration test session helper and the E2E binary.
- Dependency-remediated E2E runs `1778613709` and `1778613956`: passed TestMock
  and MessageIdMultisig bidirectional bridge paths against clean Rusk. Logs are
  in `/tmp/hyperlane-dependency-remediation-e2e-testMock-1778613709.outer.log`,
  `/tmp/hyperlane-dependency-remediation-e2e-messageIdMultisig-1778613956.outer.log`,
  and the archive listed above.
- `make secret-hygiene`: passed.
- `make dependency-alert-status`: passed; queried 27 open GitHub Dependabot
  `Cargo.lock` alerts and confirmed all 27 have no current local lockfile
  package versions within the alert vulnerable ranges. The script runs a
  built-in vulnerable-range parser self-test before querying GitHub. This is a
  feature-branch triage aid and does not replace GitHub closing alerts after a
  default-branch rescan.
- `make completion-audit-status`: passed; verified the preserved Dusk and
  monorepo prototype archive refs, local backup artifact hashes, active branch
  refs, and absence of untracked source paths in both active repos.
- `make gate-status`: passed; reported the implementation PRs and manual
  workflow dispatcher PR, status-check rollups on all internal PRs,
  7 unchecked production sign-off items, all six split decision issues open,
  active-repo untracked source status, default-branch protection and merge
  method settings, required status-check policy enabled for Dusk
  `Dusk review policy gate` + `Production readiness guard` and monorepo
  `Dusk review policy gate` + `Dusk agent cargo check`, with
  `missingRequiredStatusChecks: none` for both repos, workflow visibility,
  repo-level Actions secret visibility, exact `DUSK_ORG_READ_TOKEN`
  visibility in both internal repos, optional `DUSK_STATUS_READ_TOKEN`
  visibility for the production-readiness workflow, self-hosted runner
  visibility, and exact `dusk-hyperlane` runner-label visibility for CI gate #8,
  latest clean-layout repro link visibility, post-rebase E2E/archive link visibility,
  dependency-remediated E2E link visibility, advisory reviewer routing link
  visibility, Dusk Dependabot open-alert visibility plus local lockfile
  vulnerable-range comparison, reviewer-facing `make gate-status-fresh`,
  `make dependency-alert-status`, and `make completion-audit-status` handoff
  text, reviewer-facing `make review-gates` and
  its archive hygiene self-test and extracted archive scan handoff text,
  stale monorepo queued-check wording after inherited Depot workflow guards,
  `make production-readiness-guard` handoff text, reviewer-facing branch
  protection/status-check policy handoff text, reviewer-facing latest
  clean-layout repro path delta handoff text, current Hyperlane upstream drift,
  no Dusk covered-path delta since latest clean-layout repro source ref
  `ef8ee43cd99569299b9744b498ac1bbac69950bc`, and no placeholder matches in
  tracked Dusk repo runtime paths or `rust/main/chains/hyperlane-dusk`.
  It also reports the monorepo delta since latest clean-layout repro monorepo
  ref `515fab074024271935bc7795604dbb4f0823a937` and verifies
  `monorepoCoveredPathDelta: none` for the scoped Dusk agent/runtime/workflow
  paths.
- `make review-gates`: passed; this lightweight wrapper runs
  `make completion-audit-status`, `make archive-hygiene-test`,
  `make archive-hygiene`,
  `make dependency-alert-status`, `make report-hygiene`,
  `make review-hygiene`,
  `make dispatcher-merge-order-smoke`, and
  `make gate-status-fresh`. Moving PR heads and check rollups are read through
  GitHub and `make gate-status` instead of pinned to one "current" handoff
  comment. Recent exports include `/tmp/hyperlane-review-export-1778717975`
  and `/tmp/hyperlane-review-export-1778718283`. This wrapper does not replace
  `make repro-check-agent`, clean-Rusk E2E, CI provisioning, or Dusk production
  sign-off.
- `make production-readiness-guard`: failed as expected while external
  production blockers remain open. It reports open internal PR/review/status
  gates, non-completed PR status-check counts, unchecked sign-off items, open
  split decision issues, required status-check policy enabled, missing
  `DUSK_ORG_READ_TOKEN` visibility in both internal repos, missing self-hosted
  runner visibility for the
  `dusk-hyperlane` label, Dusk Cargo dependency alert triage, upstream
  freshness, and latest clean-layout repro covered-path delta. Passing this
  guard would not by itself prove production readiness. Protected-branch review
  baselines are now enabled on both Dusk org default branches. In CI, some
  visibility checks can require permissions broader than source checkout;
  `DUSK_ORG_READ_TOKEN` remains source-checkout-only, and Dusk must use an
  approved admin/security-read local `gh` credential or separate approved CI
  credential such as `DUSK_STATUS_READ_TOKEN` for branch protection,
  Actions-secret, runner, and Dependabot alert visibility. The optional status
  token is only for `.github/workflows/production-readiness-gate.yml`; the
  manual repro workflow remains on the source-checkout token. After the
  2026-05-14 branch-protection promotion, the guard no longer reports missing
  required status-check contexts; the required-check blockers are now passing
  policy shape, while the checks themselves remain red until the known review,
  token, runner, workflow-publication, and sign-off blockers close.
- Required-check promotion refresh on 2026-05-14: Dusk branch protection now
  requires `Dusk review policy gate` plus `Production readiness guard`;
  monorepo branch protection now requires `Dusk review policy gate` plus
  `Dusk agent cargo check`. `git diff --check`, `make report-hygiene`,
  `make review-hygiene`, `make gate-status`, and `make review-gates` passed
  after the docs and live issue #2/#8 refresh. Review exports:
  `/tmp/hyperlane-review-export-1778719581` and
  `/tmp/hyperlane-review-export-1778719621`; dispatcher merge-order logs:
  `/tmp/hyperlane-merge-order-logs.C1vFVc`. `make gate-status` and
  `make review-gates` both reported `missingRequiredStatusChecks: none` for
  both repos. `make production-readiness-guard` still failed closed with
  exit code 2 only on the expected external blockers: Dusk PR #1,
  monorepo PR #1, and dispatcher PR #3 open/review-required; issue #2 has
  7 unchecked checklist items; split decision issues #4-#9 remain open; no
  visible repo-level `dusk-hyperlane` runner and org runner visibility is
  unknown; `DUSK_ORG_READ_TOKEN` is not visible in either internal repo.
- Recent CI `Production readiness guard` runs on Dusk PR #1 complete with the
  same expected failure. The live PR check rollup is checked with `gh pr view 1
  --repo dusk-network/hyperlane-dusk --json statusCheckRollup,headRefOid`
  rather than pinned here, because docs-only report commits move the PR head.
  The failed-job summary is limited to the known external blockers:
  unmerged/unapproved internal PRs, unknown branch-protection visibility from
  the Actions integration token, 7 unchecked production sign-off items, 6 open
  split decision issues, unavailable Dependabot alert triage in Actions,
  unknown `dusk-hyperlane` runner visibility, unknown repo-level Actions
  secret visibility, and `coveredPathDelta: none` plus
  `monorepoCoveredPathDelta: none`. This run exercises the CI path that now
  compares monorepo covered-path tree manifests instead of GitHub compare API
  file lists, avoiding false positives after the monorepo branch is rebased.
- After the Dusk head `803e855b3692706e38c630777230727d76a02afd`,
  `scripts/production-readiness-guard.sh` was hardened for CI status-check
  accounting: it ignores the current `GITHUB_RUN_ID` when counting
  non-completed PR checks and polls briefly for other concurrently-started
  checks before reporting the count. This prevents the production-readiness
  workflow from self-blocking on its own in-progress check while preserving the
  queued/incomplete-check blocker for external checks.
- After the Dusk head `65806dfdd64a2a1171d754b773e5b24224242289`, the
  production-readiness CI exposed a wait-loop bug: progress lines for
  non-completed companion checks were written to stdout and polluted the JSON
  status rollup consumed by `jq`. The guard now writes those wait-progress
  lines to stderr so captured stdout remains valid JSON.
- `make repro-check-agent`: added as a Makefile wrapper for the full local
  non-E2E repro command, including the Hyperlane Rust agent check. Latest
  checkout-v6 clean-layout run `1778695627` passed against Dusk source ref
  `ef8ee43cd99569299b9744b498ac1bbac69950bc`, monorepo
  `515fab074024271935bc7795604dbb4f0823a937`, upstream base
  `7689ff65f4929a72ad0650a03e8dd7d987f0e802`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-checkout-v6-repro-1778695627.tgz`;
  SHA256:
  `1f16dd8caa86c54ff351f0a0fc41f9ee8083c25515514ac77afb2f60f7483ccb`.
  The archive contains `/tmp/hyperlane-review-checkout-v6-repro-1778695627.log`
  with SHA256
  `9384e858bdd00f88665969a91cfa583048c4437177b615c7ddf49c676a4c2c12`;
  `scripts/secret-hygiene-check.sh` passed over the archive staging directory
  and tarball, and `scripts/archive-hygiene-check.sh` passed over the durable
  evidence archive directory after the archive was added.
- `bash scripts/local-repro-check.sh --agent-check`: passed. This wraps the
  same repeatable non-E2E checks plus the Hyperlane Rust agent check. It still
  requires local/private Rusk path dependencies and does not replace the
  E2E/fault-injection runs below.
- `.github/workflows/manual-repro-check.yml`: added after the local repro run as
  a manual self-hosted workflow template for the same `make repro-check-agent`
  command. It requires Dusk to provide a `dusk-hyperlane` self-hosted runner and
  `DUSK_ORG_READ_TOKEN` secret for private cross-repo checkout. The documented
  token scope is read-only source checkout for `dusk-network/hyperlane-dusk`,
  `dusk-network/hyperlane-monorepo`, and `dusk-network/rusk-private`; it is not
  a signer, runtime secret, relayer key, deployment key, or image-publishing
  credential. The manual inputs include `dusk_ref`, `rusk_ref`, and
  `monorepo_ref` so reviewers can run the workflow from the default branch
  against exact review heads. `CI_REPRO_STRATEGY.md` includes the
  `gh workflow run` command shape and says to record both requested refs and
  resolved heads for release evidence. The workflow logs the resolved checkout
  heads before running the repro command.
- dusk-network/hyperlane-dusk#3: opened as a workflow-only default-branch
  dispatcher PR containing `.github/workflows/manual-repro-check.yml`,
  `.github/workflows/manual-repro-dispatcher-gate.yml`,
  `.github/workflows/dusk-review-policy-gate.yml`, and
  `.github/actionlint.yaml`, so Dusk can make the manual workflow visible
  independently from the full implementation PR while preserving the shared
  review-policy gate.
- `make dispatcher-merge-order-smoke`: passed on 2026-05-13. This target uses
  temporary detached worktrees from `origin/main`
  `c5ce2135407dad6420d010bdafe82a0b9b4bb78d` and verifies both current remote
  landing orders: dispatcher branch `origin/ci/manual-repro-workflow`
  `a73cb10f0be7693a5d6eab4ebedcccb640b29655` followed by implementation
  branch `origin/feat/dusk-hardening-v2`
  `f656de36ea7b7099ab3a1eabcb89b672d9eb9c4d`, and the reverse order. In both
  orders, `.github/workflows/manual-repro-check.yml`,
  `.github/workflows/dusk-review-policy-gate.yml`, and
  `.github/actionlint.yaml` had an empty diff against the implementation
  branch after the combined merge. Temporary command logs were written under
  `/tmp/hyperlane-merge-order-logs.QUAn6t`.
- `actionlint .github/workflows/manual-repro-check.yml`: passed after adding
  `.github/actionlint.yaml` for the custom self-hosted `dusk-hyperlane` label.
- `.github/workflows/production-readiness-gate.yml`: added as a lightweight
  GitHub-hosted status-check candidate. It runs
  `make production-readiness-guard` with GitHub compare API freshness checks,
  and is expected to fail until the known review, sign-off, workflow
  runner/secret, and internal merge blockers close.
- `actionlint .github/workflows/production-readiness-gate.yml`: passed.
- `.github/workflows/dusk-review-policy-gate.yml`: added as the shared
  required status-check policy enabled on both Dusk org default branches.
- `actionlint .github/workflows/dusk-review-policy-gate.yml`: passed.
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
  `/tmp/hyperlane-review-export-1778605694`. It exports Dusk PR #1,
  workflow PR #3, monorepo PR #1, sign-off issue #2, split issues #4-#9, and
  their comments, scans for known stale evidence refs/wording, validates
  explicit current-head claims for Dusk PR #1, monorepo PR #1, and workflow
  PR #3 against live GitHub PR heads while ignoring comments explicitly marked
  Historical or Superseded, rejects active stale clean-layout evidence pointers
  to the superseded `4431719578` comment and Dusk source ref `6ac9a2bc`, rejects
  active stale upstream-rebase wording for the superseded `66e8c1f4` monorepo
  base, requires old status-snapshot comments to be marked superseded, and then
  runs `scripts/secret-hygiene-check.sh` over the export. The latest guard
  updates were committed as `3296586` and `76ca8ce`; after editing the PR
  evidence note to avoid pinning a moving PR head, the wrapper was rerun and
  passed against the updated GitHub review surface.
- `make review-hygiene` now also requires the latest clean-layout repro
  evidence link
  `https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043`,
  dependency-remediated E2E link
  `https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389`,
  and advisory reviewer routing link
  `https://github.com/dusk-network/hyperlane-dusk/blob/feat/dusk-hardening-v2/REVIEWERS.md`
  in Dusk PR #1, monorepo PR #1, and sign-off issue #2 bodies. It also requires
  the advisory reviewer routing link in workflow PR #3, and rejects the
  superseded `1778607202`/`4433179148` repro evidence from those active bodies
  plus stale "no owner-routing file" wording. The wrapper passed after this
  guard was added.
- `make review-hygiene` now rejects the stale pre-archive-hygiene
  `make review-gates` bundle description in active reviewer-facing text and
  requires Dusk PR #1, monorepo PR #1, and sign-off issue #2 bodies to mention
  archive hygiene self-tests and extracted evidence archive hygiene scans. This
  guard passed after the active PR and issue bodies were updated in place.
- `make review-hygiene` now also requires active reviewer-facing text to
  mention `make dispatcher-merge-order-smoke`, so the new PR #3 / PR #1
  landing-order check remains visible in the review handoff.
- `make review-hygiene` now requires current head/gate handoff text in active
  PR/sign-off bodies without pinning one specific "current" comment URL. Moving
  heads and check URLs remain live values from GitHub PR headers and
  `make gate-status`, which avoids a new handoff-comment churn cycle for each
  docs-only guardrail commit.
- Issue #8 was corrected so its latest clean-layout repro link names the tested
  Dusk source ref `2dd0d227cf0c33033cd9206151c1cbca6cddfffb`, monorepo ref
  `3b7d9f64d7d9465eaa868770d970243d98bc53c6`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`; `make review-hygiene` now
  rejects the stale older tested-ref text in that issue body.
- Active Dusk PR #1, monorepo PR #1, and sign-off issue #2 bodies were updated
  to call run `1778751867` the latest clean-layout repro and to demote
  checkout-v6 run `1778695627` to prior workflow-checkout evidence. `make
  review-hygiene` now rejects stale `Latest checkout-v6 clean-layout` wording
  in those active bodies.
- `make review-hygiene` now rejects stale active reviewer-facing wording that
  claims the live monorepo branch is rebased onto upstream Hyperlane
  `2b7db706023806b36a57e446205ae443537ae9ec`; historical clean-layout repro
  evidence can still cite that SHA as the tested repro base, while live branch
  status must point at current upstream `a8c9430c82f0b66faf4231828f798dcbec95dfab`.
- `make review-hygiene` now rejects JSON-escaped reviewer-facing PR, issue, or
  comment bodies that render as quoted strings with literal `\n` escapes. This
  guard passed after the Dusk PR #1, monorepo PR #1, and #8 runbook comment
  bodies were repaired with raw Markdown update payloads.
- `make review-hygiene` now rejects stale Dusk handoff heads and check URLs
  from the prior `54e56fa`, `db863ed`, and `cc3d75a` gate refreshes in active
  reviewer-facing text. After the #8 runbook comment was refreshed to avoid
  stale handoff wording and moving check-run pins, `make review-hygiene` passed
  with export
  `/tmp/hyperlane-review-export-1778682025`.
- `make review-hygiene` now checks workflow PR #3 comments for the current
  clean-layout repro evidence link
  `https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043`
  and rejects stale dispatcher repro evidence links
  `https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4440412895`
  and
  `https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4443963744`.
  `make fail-closed-self-test` covers the stale dispatcher-comment scan through
  `scripts/github-review-hygiene.sh --dispatcher-comment-scan-only`.
  The required `Dusk review policy gate` now also syntax-checks the review
  hygiene scripts and runs the same dispatcher stale-link scan against a CI
  fixture, so this regression is covered on every Dusk PR #1 push without
  requiring private repositories or local evidence archives. The workflow skips
  this script-specific check on the workflow-only dispatcher branch, where the
  Dusk guard scripts are intentionally not present, so PR #3 can still land
  before or after PR #1 without workflow drift.
  This guard was added after the PR #3 exact-ref dispatch comment was updated
  in place. `bash -n scripts/github-review-hygiene.sh`, `git diff --check`,
  `make secret-hygiene`, and `make review-hygiene` passed. The same stale
  clean-layout evidence link was also removed from
  `PRODUCTION_REVIEW_DECISIONS.md`, and `make report-hygiene` now rejects that
  stale link in the production decision record.
  Dusk review policy job passed and the production-readiness job failed with the
  expected blocker summary.
- `make review-hygiene` now rejects the active reviewer-facing phrase
  `Current latest clean-layout repro evidence`. The historical RUES-hardening
  repro comment was edited in place to start as historical/superseded evidence
  and point at the current clean-layout repro comment. `bash -n
  scripts/github-review-hygiene.sh`, `git diff --check`, `make review-hygiene`,
  and `make review-gates` passed at guard commit
  `fac1134131d37d8dff5b7e2ca9dada40983ac688`; later current-head refreshes
  extended those guards through the current Dusk head
  `13eaca26336820e27b862ecf3cfccc564a47e751`. Latest focused
  `make review-hygiene` export from `make review-gates` after the full
  dispatcher validation handoff refresh:
  `/tmp/hyperlane-review-export-1778698110`.
- After the live sign-off issue #2 body was corrected to remove stale
  `1778683232`/`9050143c1ef12f76d117ee97effa79da8df3e334` monorepo
  "latest" repro wording, `scripts/github-review-hygiene.sh` now rejects that
  stale clean-layout repro pair in active PR/issue bodies and requires issue #2
  to keep the current `1778695627`/`515fab074024271935bc7795604dbb4f0823a937`
  monorepo repro handoff text. `make review-gates` passed with review-hygiene
  export `/tmp/hyperlane-review-export-1778698688`.
- The previous gate refresh comment at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4446061536
  and the #8 CI/repro runbook comment were updated in place to avoid pinning
  moving Dusk PR-head SHAs or moving Dusk PR #1 check-run URLs. A later
  handoff at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4446372895,
  which records Dusk head `2df0582ac5ecf2b8232be996f302755d87ac5701`, required
  status-check promotion with `missingRequiredStatusChecks: none`, the passing
  `Dusk review policy gate` run, and the expected failed-closed
  `Production readiness guard` run. Active bodies were repointed to that
  handoff while live PR check rollups remained the source for moving state.
  `scripts/github-review-hygiene.sh` rejects the stale
  `8b15eb607e83b80cc334c6402e6c752dcfc9a1ba` gate-refresh head, old
  `25817675933`/`25817675973` check runs, the old `aheadBehind` `43 0`
  wording, and future active comments that pin moving Dusk PR #1 heads or
  Dusk PR #1 check-run URLs. The latest post-edit `make review-hygiene` run
  passed with export `/tmp/hyperlane-review-export-1778719946`; the latest
  post-edit `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778719621` and dispatcher merge-order smoke
  log `/tmp/hyperlane-merge-order-logs.C1vFVc`.
- The monorepo PR #1 body was updated in place to avoid pinning moving
  monorepo PR check-run URLs for `Dusk review policy gate`,
  `Dusk agent cargo check`, and inherited fork-skipped checks. It now points
  reviewers to the live PR check rollup and `make gate-status-fresh` for
  moving status, while keeping the stable failure shape for the missing
  `DUSK_ORG_READ_TOKEN` preflight. `scripts/github-review-hygiene.sh` now
  rejects future active comments that reintroduce moving monorepo PR check-run
  URLs or the stale `25816561587`/`25816561525` run IDs. The post-edit
  `make review-hygiene` run passed with export
  `/tmp/hyperlane-review-export-1778700112`; the post-edit
  `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778700153` and dispatcher merge-order smoke
  log `/tmp/hyperlane-merge-order-logs.wc83Bz`.
- Local report hygiene now rejects stale Dusk PR check-run/job IDs that were
  previously retained in historical report rows. The live-head delta audit row
  now points to live PR check rollups for moving status instead of pinning those
  old URLs. The post-edit `make review-gates` run passed with review-hygiene
  export `/tmp/hyperlane-review-export-1778700393` and dispatcher merge-order
  smoke log `/tmp/hyperlane-merge-order-logs.QvdMgT`.
- Local report hygiene now also rejects pinned moving live-head claims such as
  `live ... PR head is <sha>` in local reports. It fails closed on invalid
  stale-pattern regexes; `STALE_REPORT_PATTERNS='[invalid' bash
  scripts/report-hygiene-check.sh` failed as expected with log
  `/tmp/hyperlane-report-hygiene-invalid-pattern.log`. The post-edit
  `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778700709` and dispatcher merge-order smoke
  log `/tmp/hyperlane-merge-order-logs.R5YjiY`.
- GitHub review hygiene now also fails closed on `rg` errors in regex-backed
  stale-text scans. `STALE_REVIEW_PATTERNS='[invalid' bash
  scripts/github-review-hygiene.sh --export-dir
  /tmp/hyperlane-review-invalid-pattern --no-keep` failed as expected with log
  `/tmp/hyperlane-review-invalid-pattern.log`. The post-edit
  `make review-hygiene` run passed with export
  `/tmp/hyperlane-review-export-1778700945`; post-edit `make review-gates`
  runs passed with review-hygiene exports
  `/tmp/hyperlane-review-export-1778700968` and
  `/tmp/hyperlane-review-export-1778701047`, and dispatcher merge-order smoke
  logs `/tmp/hyperlane-merge-order-logs.prF75U` and
  `/tmp/hyperlane-merge-order-logs.758Ges`.
- Secret hygiene now fails closed on `rg` errors in tracked-source and runtime
  artifact scans. An unreadable runtime artifact probe failed as expected with
  log `/tmp/hyperlane-secret-hygiene-unreadable.log`. `make secret-hygiene` and
  `make archive-hygiene-test` passed after the guard update, and the post-edit
  `make review-gates` runs passed with review-hygiene exports
  `/tmp/hyperlane-review-export-1778701258` and
  `/tmp/hyperlane-review-export-1778701318`, and dispatcher merge-order smoke
  logs `/tmp/hyperlane-merge-order-logs.OHtUdQ` and
  `/tmp/hyperlane-merge-order-logs.EX9psz`.
- Archive hygiene now fails closed on `rg` errors in archive member-path scans
  and no longer pipes special-file detection through `rg`. An invalid member
  pattern probe with `ARCHIVE_UNSAFE_MEMBER_PATTERN='[invalid'` failed as
  expected with log `/tmp/hyperlane-archive-hygiene-invalid-pattern.log`.
  `make archive-hygiene-test` passed after the guard update, and the post-edit
  `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778701507` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.ebdc5r`.
- Runtime placeholder scans now fail closed on `git grep` errors in both
  `make gate-status` and `make review-hygiene`. Invalid pattern probes with
  `DUSK_PLACEHOLDER_PATTERN='[invalid' bash scripts/release-gate-status.sh` and
  `AGENT_PLACEHOLDER_PATTERN='[invalid' bash scripts/github-review-hygiene.sh`
  failed as expected with logs
  `/tmp/hyperlane-gate-status-invalid-placeholder-pattern.log` and
  `/tmp/hyperlane-review-invalid-agent-pattern.log`. The post-edit
  `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778701892` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.0nGCze`.
- `make fail-closed-self-test` now automates the archive member-path invalid
  regex, gate-status placeholder invalid regex, review-hygiene agent placeholder
  invalid regex, report-hygiene stale-pattern invalid regex, report-hygiene
  stale dispatcher smoke evidence, and secret-hygiene unreadable runtime
  artifact probes. It also creates a temporary untracked repo file and verifies
  `make completion-audit-status` rejects it. It now also pins the pre-malformed
  warp-token test slice ref and verifies production readiness fails closed when
  `tests/tests/integration.rs` is a latest-repro covered-path delta. It is part
  of `make review-gates`;
  the post-edit `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778702613` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.YO0Zpk`.
- `make fail-closed-self-test` now also injects a temporary report containing
  stale dispatcher smoke evidence and verifies `make report-hygiene` rejects it.
  It also verifies that `GOAL_AUDIT.md` rejects moving dispatcher smoke head/log
  pins and keeps that evidence live through `make dispatcher-merge-order-smoke`.
  The post-edit `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778703841` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.SSFKEq`.
- `make fail-closed-self-test` now also injects a temporary latest-repro report
  that cites only the `/tmp` checkout-v6 repro log and verifies
  `make report-hygiene` rejects it unless the durable
  `hyperlane-checkout-v6-repro-1778695627.tgz` archive is also named in the
  required report files. The post-edit `make review-gates` run passed with
  review-hygiene export `/tmp/hyperlane-review-export-1778704417` and
  dispatcher merge-order smoke log `/tmp/hyperlane-merge-order-logs.zqIcx0`.
- `make fail-closed-self-test` now also injects a temporary latest-repro report
  that names the durable checkout-v6 repro archive path without its tarball
  SHA256 and verifies `make report-hygiene` rejects it. `make review-hygiene`
  also requires active reviewer-facing text to keep both the durable archive
  path and hash visible. The post-edit `make review-gates` run passed with
  review-hygiene export `/tmp/hyperlane-review-export-1778704675` and
  dispatcher merge-order smoke log `/tmp/hyperlane-merge-order-logs.unXFvr`.
- `make report-hygiene` now also verifies that the latest checkout-v6 durable
  repro archive exists locally and matches the expected SHA256 before accepting
  report mentions as evidence. `make fail-closed-self-test` covers missing
  archive files and archive hash mismatches in addition to missing report path
  and hash text.
- `scripts/secret-hygiene-check.sh` now treats `.env` and `.env.*` files as
  secret-like runtime artifact filenames, matching the tracked-source policy
  for local dev env files. `make fail-closed-self-test` covers `.env.bridge`
  artifact rejection before CI or reviewer evidence upload.
- Runtime artifact secret-text scanning now rejects JSON `privateKey` and
  `private_key` fields containing 32-byte hex keys in addition to `key` fields
  and `hexKey` signer markers, and also rejects TOML/YAML-style `key`,
  `privateKey`, or `private_key` assignments plus uppercase env/log key
  assignments with 32-byte hex keys. `make fail-closed-self-test` covers JSON
  `privateKey`, TOML `private_key`, single-quoted YAML `privateKey`, and
  `DUSK_SIGNER_KEY` artifact fixtures. This was pushed as Dusk commit
  `28f834501f0f32b0d7333b5a1831059eb80fd4d1` after these local checks passed:
  `bash -n scripts/secret-hygiene-check.sh scripts/fail-closed-self-test.sh &&
  git diff --check`, `make secret-hygiene`, `make fail-closed-self-test`,
  `make report-hygiene`, and `make review-gates`.
- `gh pr checks 1 --repo dusk-network/hyperlane-dusk --watch --interval 10`
  on Dusk commit `28f834501f0f32b0d7333b5a1831059eb80fd4d1` reported
  `Dusk review policy gate` passed at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25828772990/job/75888475154
  and `Production readiness guard` failed closed at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25828773013/job/75888475203.
  The command exited nonzero because the expected fail-closed guard remained
  blocked.
- A post-push `make production-readiness-guard` run failed closed with
  `productionReadinessGuard: blocked`: Dusk PR #1, monorepo PR #1, and workflow
  dispatcher PR #3 were still open and review-required; production sign-off
  issue #2 still had 7 unchecked items; split decision issues #4 through #9
  remained open; no self-hosted runner with label `dusk-hyperlane` was visible;
  and `DUSK_ORG_READ_TOKEN` was not visible in the Dusk or monorepo repos.
- `make fail-closed-self-test` now also covers production CI provisioning
  blockers with mocked `gh` commands and isolated guard modes. The mock covers
  missing default-branch protection and protected branches that still require
  only one status check or require the wrong second status check with
  `BRANCH_PROTECTION_GATE_ONLY=1`, plus missing workflow visibility, no
  repo/org runner with label `dusk-hyperlane`, and no `DUSK_ORG_READ_TOKEN`
  secret in either internal repo with
  `CI_VISIBILITY_GATE_ONLY=1`; the guard rejects branch-protection, workflow,
  runner, and required-secret blockers before a live PR can be treated as
  production-ready.
- `make fail-closed-self-test` also covers unavailable dependency-alert triage
  with `DEPENDENCY_ALERT_GATE_ONLY=1` and a mocked dependency-alert helper that
  emits `dependencyAlertStatus: unavailable`. The production-readiness guard
  rejects that path before a PR can be treated as production-ready.
- Dusk commit `eff5e3bc181707756eead42880c71ccd1e685d34` removes the remaining
  direct `unwrap()` calls from production warp-route inbound router checks and
  extends `make gate-status` to reject direct `unwrap()` in `contracts/`.
  `make fail-closed-self-test` covers invalid-regex failure for that new scan.
  Validation:
  `bash -n scripts/release-gate-status.sh scripts/fail-closed-self-test.sh`,
  `bash scripts/release-gate-status.sh --placeholder-scan-only`,
  `make fail-closed-self-test`, `make clippy-contracts`, and
  `make test-integration` passed. `cargo fmt --all --check` and targeted
  `rustfmt --check` were not clean because of pre-existing formatting drift
  outside the changed hunks, so no broad formatting churn was applied.
- Clean-layout repro run `1778751867` passed with `make repro-check-agent` and
  `RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` against Dusk
  `2dd0d227cf0c33033cd9206151c1cbca6cddfffb`, monorepo
  `3b7d9f64d7d9465eaa868770d970243d98bc53c6`, upstream base
  `a8c9430c82f0b66faf4231828f798dcbec95dfab`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It built all contract WASMs,
  passed production wasm clippy, 28 type tests, 72 VM integration tests, 3
  `dusk-tx` tests, source secret hygiene, and Hyperlane Rust agent cargo check.
  Log: `/tmp/hyperlane-clean-repro-current-head-1778751867.log`; log SHA256
  `df88d712f4bfada0b1958a9b4d7c1b96b0b2ec4754482c524dca91b0c83e738d`.
  Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-clean-repro-current-head-1778751867.tgz`;
  archive SHA256
  `9e08ce22389f4a209d3d1ed79aa90de8d5384ce77ca7c142019c3264b799b7e7`.
- Post-promotion local guard checks passed: `git diff --check`,
  `make report-hygiene`, `make fail-closed-self-test`, `make review-hygiene`
  with export `/tmp/hyperlane-review-export-1778723293`, and
  `make archive-hygiene`. `make gate-status` reported
  `coveredPathDelta: none` and `monorepoCoveredPathDelta: none`.
  `make production-readiness-guard` failed
  closed as expected on open review/merge/sign-off gates, missing visible
  `DUSK_ORG_READ_TOKEN`, and missing visible `dusk-hyperlane` runner
  provisioning.
- Dusk commit `6f6854d3f8ec797cf21afda2c4df482fb42bea16` is a docs-only
  evidence refresh linking the latest dependency-alert guardrail comment. Local
  `git diff --check`, `make report-hygiene`, and `make completion-audit-status`
  passed. `gh pr checks 1 --repo dusk-network/hyperlane-dusk --watch
  --interval 10` reported `Dusk review policy gate` passed at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25830381479/job/75893688438
  and `Production readiness guard` failed closed at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25830381459/job/75893688355.
  A subsequent `make gate-status` run reported Dusk head
  `6f6854d3f8ec797cf21afda2c4df482fb42bea16`, monorepo head
  `515fab074024271935bc7795604dbb4f0823a937`, no untracked source,
  `coveredPathDelta: none`, `monorepoCoveredPathDelta: none`, and the same
  external review/sign-off/runner/token blockers.
- Dusk commit `c1388203d2757d8a47a23ccb9db3154c51a187e3` is a docs-only audit
  refresh for that recorded gate sample. `gh pr view 1 --repo
  dusk-network/hyperlane-dusk --json headRefOid,statusCheckRollup,reviewDecision,mergeable,state`
  reported the live PR head as `c1388203d2757d8a47a23ccb9db3154c51a187e3`,
  mergeable, open, and review-required. Its check rollup had `Dusk review
  policy gate` passed at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25830505657/job/75894085847
  and `Production readiness guard` failed closed at
  https://github.com/dusk-network/hyperlane-dusk/actions/runs/25830505654/job/75894085922.
  Future docs-only commits intentionally do not pin themselves as "latest";
  live values are read from `make gate-status` and GitHub check rollups.
- `make gate-status-fresh` and `make production-readiness-guard` now report an
  upstream submission gate using GitHub search for open
  `hyperlane-xyz/hyperlane-monorepo` PRs from
  `dusk-network:feat/dusk-support-v2`. The current query reports zero open
  upstream PRs from that head; if any appear while internal blockers remain,
  the production readiness guard treats that as premature upstream submission.
  `make fail-closed-self-test` covers that premature-upstream-PR blocker with a
  mocked search response.
- `make production-readiness-guard` now waits for the minimum expected PR check
  count as well as non-completed checks, avoiding transient zero-check snapshots
  immediately after a push while still failing if checks never appear before the
  wait timeout. The post-edit `make production-readiness-guard` run failed on
  the expected review/sign-off/runner/secret blockers, and the post-edit
  `make review-gates` run passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778702836` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.oQN6ID`.
- After pushing Dusk commit `15f7e0dccc8472648d4f9583c86bcbc1286fe07d`,
  `make completion-audit-status` passed and `make review-gates` passed with
  review-hygiene export `/tmp/hyperlane-review-export-1778713607` and
  dispatcher merge-order smoke log `/tmp/hyperlane-merge-order-logs.GvcDla`.
  The handoff comment
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4445875247
  records `coveredPathDelta: none`, `monorepoCoveredPathDelta: none`, the
  latest Dusk PR #1 check URLs, and the remaining external production blockers.
- A 2026-05-14 `make review-gates` run passed after synchronizing the shared
  review-policy workflow between implementation and dispatcher branches. It
  ran against Dusk head `e8b2b8b887fec8600cb1a6a2ce47341ca7960c7a`,
  monorepo head `515fab074024271935bc7795604dbb4f0823a937`, and dispatcher
  head `b6a2341f7e793468e2a631d51b803c0b9c495003`. Review hygiene exported
  `/tmp/hyperlane-review-export-1778715303`; dispatcher merge-order smoke
  passed with log directory `/tmp/hyperlane-merge-order-logs.no277R` and
  confirmed the dispatcher and implementation branches merge cleanly in either
  order with no shared workflow/actionlint drift. The same run reported
  `coveredPathDelta: none`, `monorepoCoveredPathDelta: none`, no untracked
  source in either active repo, zero open upstream Hyperlane PRs from
  `dusk-network:feat/dusk-support-v2`, 27 open Cargo alerts with no vulnerable
  locked versions, and the remaining external review/sign-off/runner/token
  blockers. After this report refresh, `make review-gates` passed again with
  review-hygiene export `/tmp/hyperlane-review-export-1778715510` and
  dispatcher merge-order smoke log `/tmp/hyperlane-merge-order-logs.0IZsy3`.
- `make report-hygiene`: added as a local report guard for `GOAL_AUDIT.md` and
  `TEST_REPORT.md`. It rejects the stale Dusk CI URLs, dispatcher merge-order
  implementation head, monorepo success count, and old export paths that were
  corrected during the current report refresh, so `make review-gates` now
  covers both GitHub-facing review text and local review reports.
- E2E wrappers that call `demo/gen-agent-configs.sh` now track generated Dusk
  signer key files and generated agent config files and remove them on exit
  after stopping agents. `bash -n` passed for the touched E2E scripts listed
  above. `make secret-hygiene` now fails if a future E2E wrapper calls
  `gen-agent-configs.sh` without both cleanup tracking variables and the
  generated-key/config `rm -f` cleanup calls.
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
- Previous current-runtime clean-layout repro run `1778599935` passed against Dusk source ref
  `de9b7fa3c832fb60982d3dbd22c8a59114d37732`, monorepo
  `a2db5731e385634268071d39b0554883d11d8ac5`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-head-1778599935.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778599935.tgz`.
- Historical clean-layout repro run `1778604592` passed against Dusk source ref
  `aa278208b2c2b5f4abc38c32ec792295080014a9`, monorepo
  `09e62be2e55ecd87d3931f6f10623086befd7848`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-head-1778604592.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778604592.tgz`.
- Historical clean-layout repro run `1778607202` passed against Dusk source ref
  `836ee7d8d8e95152b3daaeebbc3fb56b0cc8e253`, monorepo
  `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`, upstream base
  `c6bce706316206ac7b5652155c9ea92e96f78c39`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-head-1778607202.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778607202.tgz`.
- Historical clean-layout repro run `1778615349` passed against Dusk source ref
  `016eaa89e1afce0ef9a7534fe285d9aa16e26183`, monorepo
  `006e49dd7041097384683a78b1c1973c83e90de8`, upstream base
  `c6bce706316206ac7b5652155c9ea92e96f78c39`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-head-1778615349.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-head-repro-1778615349.tgz`.
- Historical live-head clean-layout repro run `1778669495` passed against Dusk
  source ref `0be9fbfa91ef39ecd912c79d360d036530fb524d`, monorepo
  `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-current-live-head-1778669495.log`. Durable
  archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-current-live-head-repro-1778669495.tgz`.
- RUES hardening clean-layout repro run `1778671118` passed against Dusk
  source ref `93ed3b07b26d8784610d2ba754a851490de15e21`, monorepo
  `48dcc0c87efc12584904d841d5088c1ae4acef20`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene, and the Hyperlane Rust agent check for
  `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`, `scraper`, and
  `lander`. Local log:
  `/tmp/hyperlane-dusk-repro-rues-hardening-1778671118.log`. Durable archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-rues-hardening-repro-1778671118.tgz`.
- Generated agent-config cleanup clean-layout repro run `1778683232` passed
  against Dusk source ref `8d3704e8f5a3ab0976b97fc3a68319e112e8affc`,
  monorepo `9050143c1ef12f76d117ee97effa79da8df3e334`, upstream base
  `2b7db706023806b36a57e446205ae443537ae9ec`, and clean Rusk
  `c0c64db4659500d077bb253ad13acba0e347d3fc`. It covered Dusk contract WASM
  builds, `make clippy-contracts`, 28 type tests, 70 VM integration tests,
  3 `dusk-tx` tests, secret hygiene with generated agent-config cleanup
  enforcement, and the Hyperlane Rust agent check for `hyperlane-dusk`,
  `hyperlane-base`, `validator`, `relayer`, `scraper`, and `lander`. Local
  log:
  `/tmp/hyperlane-dusk-repro-agent-config-cleanup-1778683232.log`. Durable
  archive:
  `/home/hein_/projects/hyperlane/.codex-backups/hyperlane-agent-config-cleanup-repro-1778683232.tgz`.
- E2E freshness check on 2026-05-12: the Dusk diff from the review-head
  clean-Rusk E2E source ref `2ac225175b15aac465d100e748ba68f8b14bd545` to
  the latest tested source ref `836ee7d8d8e95152b3daaeebbc3fb56b0cc8e253`
  touched only docs, workflow, reference, and hygiene-script files after the
  previous clean-layout repro source ref `de9b7fa3c832fb60982d3dbd22c8a59114d37732`.
  The
  monorepo diff from the review-head clean-Rusk E2E ref
  `a44020dc998b7fe868254a5d1a349b9eb8ded899` to the live monorepo PR head
  touched only Dusk upstream-plan docs plus upstream CCIP-read relayer retry
  and TypeScript infra/config files. Dusk returns explicit unsupported errors
  for CCIP-read ISMs in this branch. No Dusk contract, Dusk type, Dusk
  transaction, or Rust Dusk agent runtime files changed after the review-head
  clean-Rusk E2E runs.

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
- Previous clean-layout repro tested Dusk PR branch:
  `de9b7fa3c832fb60982d3dbd22c8a59114d37732`
- Clean-layout repro tested Dusk PR branch:
  `aa278208b2c2b5f4abc38c32ec792295080014a9`
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
- Previous clean-layout repro tested Hyperlane monorepo branch:
  `a2db5731e385634268071d39b0554883d11d8ac5`
- Clean-layout repro tested Hyperlane monorepo branch:
  `09e62be2e55ecd87d3931f6f10623086befd7848`
- Later clean-layout repro tested Hyperlane monorepo branch:
  `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`
- Earlier run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-rusk-dir-1778540326/rusk-private`, symlinked to clean
  detached worktree `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db`
  at `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Earlier run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-rusk-dir-current-1778541170/rusk-private`,
  symlinked to clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Previous clean-layout run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-current-head-1778599935/rusk-private`, symlinked
  to clean detached worktree
  `/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db` at
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Latest run Rusk path dependency checkout in the temporary layout:
  `/tmp/hyperlane-dusk-repro-current-head-1778604592/rusk-private`, symlinked
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
- Historical clean-layout repro Rusk path dependency checkout in the
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
- Passed again after rebasing `feat/dusk-support-v2` onto upstream Hyperlane
  `main` at `c6bce706316206ac7b5652155c9ea92e96f78c39`; post-rebase head
  `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`.
- Passed after adding file/env-backed `duskKey` signer sources.
- Passed on monorepo `9e8c7abf054fcf1193abc81a2d984fe59b08eb16` after
  replacing the Dusk agent `expect(...)` RUES/rkyv paths with error returns:
  `cargo check -p hyperlane-dusk -p hyperlane-base -p validator -p relayer -p
  scraper -p lander` passed in 1m51s, and incremental `cargo check -p
  hyperlane-dusk -p hyperlane-base` passed in 44.55s.
- Passed on monorepo `f90ca7820bf73b9c99eac807fac47e5b9a8ed9e8` after
  rebasing onto upstream Hyperlane `main` at
  `2b7db706023806b36a57e446205ae443537ae9ec`, adding the Dusk-fork
  monorepo image-publish guard, and refreshing the upstream-base docs:
  `cargo check -p hyperlane-dusk -p
  hyperlane-base -p validator -p relayer -p scraper -p lander` passed in
  7.60s.
- Passed workflow-only validation on monorepo
  `48dcc0c87efc12584904d841d5088c1ae4acef20` after adding Dusk-fork guards
  for inherited Depot-backed `rust.yml`, `test.yml`, and
  `rebalancer-e2e-test.yml` jobs: `actionlint -ignore 'label
  "depot-ubuntu-24.04' .github/workflows/test.yml .github/workflows/rust.yml
  .github/workflows/rebalancer-e2e-test.yml .github/workflows/dusk-agent-gate.yml
  .github/workflows/dusk-review-policy-gate.yml`, `git diff --check`, and the
  Dusk agent `git grep` panic/placeholder scan all passed. Monorepo CI then
  reported no queued checks: 14 successes, 22 skips, and the expected
  `Dusk agent cargo check` failure at the private companion-repo preflight.
- Fresh monorepo CI evidence for the same private-token preflight was posted to
  #8 at
  https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4445189878
  after `Dusk agent cargo check` run
  https://github.com/dusk-network/hyperlane-monorepo/actions/runs/25817401047/job/75849383145
  failed with `gh: Not Found (HTTP 404)` while checking
  `dusk-network/hyperlane-dusk` access through `DUSK_ORG_READ_TOKEN`. The saved
  local log is `/tmp/dusk-agent-cargo-check-75849383145.log`.
- `actionlint .github/workflows/dusk-agent-gate.yml` passed after replacing
  the CI runtime scan's runner-local `rg` dependency with `git grep`.
- `actionlint .github/workflows/dusk-agent-gate.yml
  .github/workflows/monorepo-docker.yml .github/workflows/rust-docker.yml`
  passed after the Dusk-fork monorepo image-publish guard.
- `git grep -n -E 'todo!|unimplemented!|panic!|expect\(' --
  rust/main/chains/hyperlane-dusk/src` reported no Dusk agent runtime matches.

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

### Current Live-Head Clean Rusk E2E: TestMock ISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
bash demo/e2e-agents.sh --only testMock --timeout 300 \
  2>&1 | tee /tmp/hyperlane-current-live-head-e2e-testMock-1778609411.outer.log
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Tested Dusk repo ref `b1ccdc9d1e7797bba4939405200aa4cc5aff2ea8`.
- Tested monorepo ref `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`.
- Tested upstream base `c6bce706316206ac7b5652155c9ea92e96f78c39`.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.
- `scripts/secret-hygiene-check.sh` passed over the safe text logs listed
  below.

Artifacts:

- `/tmp/hyperlane-current-live-head-e2e-testMock-1778609411.outer.log`
- `/tmp/hyperlane-start-env-testMock-1778609411.log`
- `/tmp/hyperlane-deploy-testMock-1778609411.log`
- `/tmp/hyperlane-relayer-testMock-1778609411.log`
- `/tmp/hyperlane-db-relayer-testMock-1778609411/`
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

### Current Live-Head Clean Rusk E2E: MessageIdMultisigISM

```bash
cd /home/hein_/projects/hyperlane/dusk
RUSK_DIR=/home/hein_/projects/hyperlane/rusk-private-clean-c0c64db \
bash demo/e2e-agents.sh --only messageIdMultisig --timeout 300 \
  2>&1 | tee /tmp/hyperlane-current-live-head-e2e-messageIdMultisig-1778609697.outer.log
```

Result:

- Passed on detached Rusk commit `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Tested Dusk repo ref `b1ccdc9d1e7797bba4939405200aa4cc5aff2ea8`.
- Tested monorepo ref `1f9e49fd9f0ba84ea93e472ddbd31fddd9a04cc3`.
- Tested upstream base `c6bce706316206ac7b5652155c9ea92e96f78c39`.
- Validator and relayer started successfully.
- EVM -> Dusk delivered: 3 wDUSK minted on Dusk.
- Dusk -> EVM delivered: 1 wDUSK minted back on EVM.
- `scripts/secret-hygiene-check.sh` passed over the safe text logs listed
  below.

Artifacts:

- `/tmp/hyperlane-current-live-head-e2e-messageIdMultisig-1778609697.outer.log`
- `/tmp/hyperlane-start-env-messageIdMultisig-1778609697.log`
- `/tmp/hyperlane-deploy-messageIdMultisig-1778609697.log`
- `/tmp/hyperlane-validator-messageIdMultisig-1778609697.log`
- `/tmp/hyperlane-relayer-messageIdMultisig-1778609697.log`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778609697/index.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778609697/0_with_id.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778609697/announcement.json`
- `/tmp/hyperlane-checkpoints-anvil-messageIdMultisig-1778609697/metadata_latest.json`
- `/tmp/hyperlane-db-relayer-messageIdMultisig-1778609697/`
- `/tmp/hyperlane-db-validator-anvil-messageIdMultisig-1778609697/`
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
- Added malformed inbound TokenMessage coverage for WarpDrc20, WarpNative, and
  WarpDrc20Collateral. The synthetic route test now also verifies the failed
  delivery does not mark the message delivered or mint tokens, while the
  native/collateral route tests verify the failed delivery does not mark the
  message delivered or create pending escrow.
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
- `cargo test -p hyperlane-dusk-integration-tests invalid_token_message -- --nocapture`
  passed:
  `3 passed; 0 failed; 0 ignored; 69 filtered out`.
- `cargo test -p hyperlane-dusk-integration-tests -- --nocapture` passed:
  `72 passed; 0 failed; 0 ignored`.

## Latest Review Handoff Refresh

On 2026-05-14, after Dusk commit
`a73a7eae616496572107f0d782e46f76eea9da58`, PR #3 reviewer-facing comments
were checked for stale clean-layout repro and gate handoff links. Three
historical dispatcher comments were edited in place so they now point at:

- latest clean-layout repro evidence:
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043
- current gate handoff:
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449346805
- reviewer action queue:
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4446734652

Validation:

```bash
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo fetch upstream main
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo rev-parse FETCH_HEAD
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo merge-base HEAD FETCH_HEAD
git -C /home/hein_/projects/hyperlane/hyperlane-monorepo rev-list --left-right --count HEAD...FETCH_HEAD
make review-hygiene
make production-readiness-guard
```

Result:

- Hyperlane upstream `main` remained at
  `a8c9430c82f0b66faf4231828f798dcbec95dfab`; the Dusk monorepo branch
  remained 45 commits ahead and 0 behind that base.
- `make review-gates` passed with review-hygiene export
  `/tmp/hyperlane-review-export-1778749783` and dispatcher merge-order log
  `/tmp/hyperlane-merge-order-logs.WdLtsi`.
- `make production-readiness-guard` failed closed as expected: Dusk PR #1,
  monorepo PR #1, and dispatcher PR #3 are still open and review-required;
  production sign-off issue #2 still has 7 unchecked items; split decision
  issues #4 through #9 remain open; no repo-level self-hosted runner with label
  `dusk-hyperlane` is visible; org runner visibility still requires admin or
  runner permissions; and `DUSK_ORG_READ_TOKEN` is not visible in either
  internal repo.
- Follow-up after the docs refresh removed moving Dusk PR-head pins from the
  gate handoff, reviewer queue, and #8 runbook comments. `make review-hygiene`
  passed with export `/tmp/hyperlane-review-export-1778724624`, and
  `scripts/report-hygiene-check.sh` now rejects local report wording that pins a
  live PR head with `live ... PR head to <sha>`.
- Follow-up current gate refreshes updated the existing issue #2 handoff comment
  and reviewer action queue in place, then updated the #8 CI/admin provisioning
  runbook observed-blocker section. The handoff comments now point reviewers to
  live PR headers or `gh pr view ... --json headRefOid` for moving refs instead
  of pinning live PR-head SHAs.
- Latest aggregate local gate evidence recorded in those comments:
  `make review-gates` passed with review hygiene export
  `/tmp/hyperlane-review-export-1778724892` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.uXBq9C`; `make gate-status-fresh` reported
  `coveredPathDelta: none`, `monorepoCoveredPathDelta: none`, required status
  checks present, and zero open upstream Hyperlane PRs from
  `dusk-network:feat/dusk-support-v2`; `make production-readiness-guard` failed
  closed on the expected review, sign-off, runner, and token blockers.
- Post-comment validation passed: `make review-hygiene` exports
  `/tmp/hyperlane-review-export-1778724988`,
  `/tmp/hyperlane-review-export-1778725055`, and
  `/tmp/hyperlane-review-export-1778725111` after refreshing the gate handoff,
  reviewer queue, and #8 runbook respectively.
- Latest aggregate gate refresh at Dusk head read live from GitHub:
  `make review-gates` passed with review hygiene export
  `/tmp/hyperlane-review-export-1778752305` and dispatcher merge-order smoke log
  `/tmp/hyperlane-merge-order-logs.q2c56I`. The refreshed issue #2 handoff,
  reviewer queue, and #8 runbook now reference that aggregate evidence, and
  `make review-hygiene` passed after those GitHub-only edits with export
  `/tmp/hyperlane-review-export-1778752305`.

## 2026-07-20 Escrow and Finality Reassessment Gate

The backed-escrow and hook-provenance code was frozen at
`d8616646e11e01f1932a424dacabded352da5a65` and reproduced from a detached,
clean worktree against Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e`. The exact command was:

```bash
RUSK_DIR=/tmp/hyperlane-rusk-5c6a0b-20260720 \
  bash scripts/local-repro-check.sh
```

Durable local log: `/tmp/hyperlane-dusk-base-repro-d861664.log`.

Result:

- all contract WASMs built and the targeted wasm clippy surface passed;
- `hyperlane-dusk-types`: 29 passed, 0 failed;
- `hyperlane-dusk-integration-tests`: 93 passed, 0 failed;
- `dusk-tx`: 12 passed, 0 failed; and
- secret-hygiene checks passed.

The VM set includes live per-contract state-version queries, native custody
reserve priority, rejection of unbacked native delivery,
authorization-delayed synthetic minting for ambiguous recipients, external and
contract pending claims, realistic collateral custody, and hook-owned message
ID, insertion height, and historical-root queries. The storage additions
require fresh deployment; the demo reuse paths require MerkleTreeHook and
WarpNative version 1 and WarpDrc20 version 2.

The companion agent regression at this stage also passed:

- `cargo test -p hyperlane-dusk`: 15 passed;
- `cargo test -p hyperlane-base dusk_`: 7 passed;
- Dusk clippy with warnings denied;
- package-scoped formatting; and
- the expanded `hyperlane-dusk`, `hyperlane-base`, `validator`, `relayer`,
  `scraper`, and `lander` cargo check.

The agent results cover consensus-finality parsing, explicit transaction
success, canonical Dusk transaction IDs, block-height/hash binding, bounded
single-handle signer-file reads, and finalized sequence/Merkle integration.
Bidirectional live-agent evidence is recorded separately after the final
cross-repository heads are pinned.

## 2026-07-20 Stacked Withdrawal Reassessment Gate

The 11-commit withdrawal series was rebased from base
`b46fda9265e3381203962a65c15b697271fd5dff` to the reassessed base without
semantic changes: `git range-diff` paired every commit exactly. Withdrawal code
anchor `ad6de95dd5e1a4efab55213733125d5d4b4d13da` was then reproduced from a
detached clean worktree against Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e`. The subsequent rebase onto the
base evidence-only commit changes documentation but no contract, CLI, driver,
type, test, lockfile, or repro-script path.

Durable local log: `/tmp/hyperlane-dusk-pr10-repro-ad6de95.log`.

Result:

- all contract WASMs and the data-driver release WASM built;
- the targeted contract/type wasm clippy surface passed;
- `hyperlane-dusk-types`: 29 passed, 0 failed;
- `hyperlane-dusk-integration-tests`: 98 passed, 0 failed;
- `hyperlane-dusk-data-driver`: 6 passed, 0 failed;
- `dusk-tx`: 13 passed, 0 failed; and
- secret-hygiene checks passed.

The five withdrawal cases prove payer-only authority, exact partial/full
custody reduction, multi-payer solvency, and owner-gated proxy withdrawal for
synthetic, native, and collateral routes. Invalid/identity recipient keys and
zero or over-credit amounts reject without changing credit or paying the
recipient. The actual VM receipt is decoded through the production data driver.
These cases run in the same 98-test VM set as the new native reserve,
synthetic pending-claim, realistic collateral-custody, and Merkle history cases.

The exact pre-rebase head remains recoverable at
`backup/feat-dispatch-credit-withdrawal-pre-6832b15`; no history was discarded
while updating the stacked PR.

## 2026-07-21 Complete Deployment-Compatibility Gate

An independent post-implementation red-team identified that the saved-state
reuse paths still treated legacy liveness queries as sufficient for contracts
whose persisted layout or security semantics had changed. The systemic fix was
frozen at `f3fba994e4274717b48ec6e4cc6d885278990352` and reproduced from a
detached, clean worktree against Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` with:

```bash
RUSK_DIR=/tmp/hyperlane-rusk-5c6a0b-20260720 \
  bash scripts/local-repro-check.sh
```

Durable local log:
`/tmp/hyperlane-dusk-base-repro-f3fba99.log` (SHA-256
`a5c0bb52a31158b62a3a20cde39e75c7e20b86754a57abdf901e7dd4ba98231a`).

Result:

- all 12 contract WASMs built and the targeted wasm clippy surface passed;
- `hyperlane-dusk-types`: 29 passed, 0 failed;
- `hyperlane-dusk-integration-tests`: 95 passed, 0 failed;
- `dusk-tx`: 16 passed, 0 failed;
- `hyperlane-dusk-data-driver`: 5 passed, 0 failed;
- the standalone E2E operator binary compiled; and
- secret-hygiene checks passed.

The added VM coverage queries the version entry point on Mailbox, TestMock,
TestRecipient, MessageIdMultisigISM, ProtocolFee, AggregationHook, IGP, and
WarpDrc20Collateral. Build coverage includes ValidatorAnnounce and the three
already-versioned route/hook contracts. The fail-closed self-test separately
asserts that both base-PR deployment-reuse boundaries contain the complete
12-contract version matrix: WarpDrc20 version 2 and version 1 for every other
base contract. The stacked withdrawal gate below raises Mailbox to compatibility
version 2. Existing kind and policy probes remain in addition to those version
checks.

## 2026-07-21 Stacked Withdrawal Compatibility Gate

The withdrawal stack was merged with base head
`6246428d9246f4e4b581e7c90328d28f1439d9e5`. Because withdrawal is a required
Mailbox ABI but adds no persisted field, the stacked Mailbox advances its
deployment compatibility version from 1 to 2. Both reuse paths require that
version, so a base-only Mailbox cannot be mistaken for a withdrawal-capable
deployment. The corrected stacked implementation was frozen at
`265b7e9b1e47f4feadc4e71644d23df04680661c` and reproduced from a detached,
clean worktree against Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` with the standard local repro
command.

Durable local log:
`/tmp/hyperlane-dusk-withdrawal-repro-265b7e9.log` (SHA-256
`0bbaf2663eaa82982c95eed91921309feffa39b6ae1d649e6292ebcdd43d5f07`).

Result:

- all 12 contract WASMs built and the targeted wasm clippy surface passed;
- `hyperlane-dusk-types`: 29 passed, 0 failed;
- `hyperlane-dusk-integration-tests`: 101 passed, 0 failed;
- `hyperlane-dusk-data-driver`: 7 passed, 0 failed;
- `dusk-tx`: 18 passed, 0 failed;
- the data-driver release WASM built;
- the standalone E2E operator binary compiled; and
- secret-hygiene checks passed.

The full fail-closed self-test also passed from the linked withdrawal worktree.
Its validation-only agent-config probe now runs without an ignored
`.env.bridge`, and completion-audit repository detection accepts both primary
checkouts and linked Git worktrees. This closes the two clean-runner assumptions
exposed when the policy workflow began executing the complete self-test.

## 2026-07-21 Final-Head Live Agent Matrix

Fresh live E2E used withdrawal head
`b16af0c05547a5d8e8687f47895c664b1aa93c00`, companion agent head
`dbed54abd3`, and Rusk
`5c6a0bab11c61fb4c81275afdeceb97fb942d85e` in an isolated compatible
layout. TestMock run `1784597325` and MessageIdMultisig run `1784598195` both
passed. Before generating agent configuration, each run redeployed fresh state
and exercised the complete saved-topology validation, including Mailbox
compatibility version 2 and every other exact contract version.

Both runs:

- confirmed a live one-LUX WarpDrc20 dispatch-credit withdrawal;
- delivered synthetic, native, and collateral routes in both directions;
- observed the exact ProtocolFee collection and residual route credit;
- asserted exact native custody and collateral allowance/custody changes; and
- observed successful Dusk process simulation before propagation.

The multisig run additionally produced, discovered, and consumed a real signed
checkpoint at threshold 1. Evidence:

- combined harness log `/tmp/hyperlane-final-e2e-b16af0c-dbed54a.log`, SHA-256
  `5d59231d77c1cce8fafa42e1527eecd9ba1d41993b18a8fc947a63257933170d`;
- TestMock relayer log `/tmp/hyperlane-relayer-testMock-1784597325.log`,
  SHA-256 `0cd06c863fa6d685657fc4f62e02673da77bc8f000c2ae7557f2c0275b20e7e5`;
- multisig relayer log
  `/tmp/hyperlane-relayer-messageIdMultisig-1784598195.log`, SHA-256
  `484a45e3e801b5a4dba1134b4131111b5465571f1ecc2c69cd70d5fe528c1a83`;
  and
- multisig validator log
  `/tmp/hyperlane-validator-messageIdMultisig-1784598195.log`, SHA-256
  `d0efdc9209129efaa93b70745c0730fda1b690a1e154560ef5ca19baf3490797`.

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
  recorded in `SECURITY_REVIEW.md`. `REVIEWERS.md` records advisory routing for
  the active PRs and split decision issues, but does not replace Dusk
  production sign-off.
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
