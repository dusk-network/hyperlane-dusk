# Final red-team decisions (2026-07-21)

This record explains which findings from the independent GPT-5.6 xhigh and
Controlecentrum deep reviews were accepted, rejected, or retained as proof
obligations. It applies to the live heads of Hyperlane Dusk PRs #1, #3, and
#10 and companion monorepo PR #1. Runtime evidence remains attached to the
exact tested source refs; a later documentation or workflow commit is not
silently relabeled as having run those tests.

## Accepted and fixed

- **Trusted CI identity:** the privileged policy gate now queries the exact
  workflow file, pull-request event, current PR number, base ref/SHA, and
  proposed head SHA. A same-name check or same-head run for another PR/base
  cannot satisfy the gate. The proposed Dusk guard scripts, manual repro
  workflows, and actionlint policy are byte-locked to the base after bootstrap.
  The trusted gate has no manual trigger that could spoof its PR-only context.
- **Trusted readiness revision:** `pull_request_target` still executes only
  trusted base code, but it now fetches the exact event head as inert Git data
  and compares that SHA with the repro anchor. Manual production audits use
  `repository_dispatch`, which loads the default-branch workflow; the status
  job no longer accepts a writer-selected workflow ref or the source-read
  credential as a fallback.
- **Required-check provenance:** a check name attached to the proposed SHA is
  not sufficient authority. Readiness resolves each candidate check to its
  GitHub Actions workflow run and requires the exact workflow path, expected
  event type, repository, PR number, proposed head SHA, base SHA/ref, and the
  `github-actions` app before considering it. Only `SUCCESS` is accepted;
  `NEUTRAL`, `SKIPPED`, missing run metadata, and lookalike workflows fail
  closed. The manual dispatcher is deliberately bound to `pull_request`, while
  the trusted review/readiness gates remain `pull_request_target`.
- **Bootstrap scope:** the first guard introduction permits one explicit
  six-file bootstrap set. An empty computed worklist is no longer treated as
  proof that arbitrary changes are safe.
- **Proposal scanner binding:** the unprivileged proposal workflow invokes the
  report and secret scanners directly. A proposed Makefile can no longer turn
  the required status green by replacing those targets with no-ops.
- **IGP deployment preflight:** CLI admission applies the contract's minimum
  quote, checked-arithmetic, and maximum-quote rules before loading signer
  material or submitting any deterministic deployment.
- **Transaction reconciliation:** transport/server failures remain retryable;
  deterministic HTTP-client and successful-response schema incompatibilities
  terminate immediately while preserving the exact transaction hash.
- **Operator-visible transaction identity:** `dusk-tx` emits the locally
  computed hash before its first propagation attempt. The local bridge demo no
  longer discards the helper JSON; it prints completed Dusk hashes and carries
  exact-hash reconciliation instructions through ordinary error paths.
- **Escrow parity:** synthetic, collateral, and native routes support
  capability-authenticated claims for contract-shaped recipients. The native
  route transfers through a fixed `receive_native_pending` callback, and root
  Moonlight callers cannot use the contract claim entrypoint.
- **Escrow/custody invariants:** all routes reject zero recipients before state
  or value changes. Collateral release proves exact custody decrease and exact
  recipient credit for direct delivery and pending claims.
- **Warm readiness:** `deploy.sh --skip-deploy` validates topology and the live
  atomic multisig policy, verifies the generated single validator is currently
  authorized, and repairs depleted dispatch credit for every supported route
  before reporting readiness.
- **Validator discovery:** the data driver and CLI now encode and decode all
  bounded ValidatorAnnounce discovery shapes. The CLI also exposes the atomic
  validators-and-threshold query used by warm validation. Its batch limit now
  matches the contract's two-validator query bound exactly. Discovery is now
  paginated and exposes a separate count; the former 1,024-entry global cap was
  removed because an attacker could consume it with self-owned keys and prevent
  later legitimate validators from enrolling. Per-validator location history
  remains bounded, while storage-paying enrollment has no shared finite quota.
- **Route dispatch fees:** synthetic, collateral, and native users contribute
  the route's quoted native-DUSK fee in their own Moonlight transaction. Mailbox
  authenticates the contract-to-contract transfer and credits the actual route.
  Each route snapshots its pre-existing credit and requires the same balance
  after dispatch, so a user cannot spend shared operational/sponsor credit even
  if a hook quote changes between preflight and execution. Native deposits are
  `bridge amount + fee`; token routes deposit only the fee. The CLI queries this
  amount before loading signer material, while the contract repeats and enforces
  the quote atomically.
- **CLI public-input boundaries:** specialized dispatch calls apply the shared
  serialized-argument ceiling before endpoint or signer access; fund-dispatch
  and DRC20 approval parse deterministic contract identities before signer
  access; bytes32-returning queries preserve bytes32 arguments such as
  `recipient_ism`.
- **Artifact and repro fidelity:** secret-content scans include hidden and
  ignored regular files; the primary repro invokes the DRC20-feature-aware
  type target; oversized process metadata is rejected before signer/client
  access; parent E2E cleanup ownership begins only after generator success.
- **Fee beneficiary identity:** ProtocolFee and IGP claims require a direct
  Moonlight call whose public-key hash equals the configured beneficiary. A
  contract-shaped beneficiary can no longer redirect funds to its outer caller.
- **Validator fail-stop:** the companion agent treats reorg-flag read failures
  as errors and distinguishes a missing flag from other filesystem failures.

## Rejected as a current exploit

- **Mailbox renounce then reinitialize:** the pinned Dusk VM exposes `init`
  only at deployment, so a deployed Mailbox cannot re-enter it after ownership
  renunciation. The source guard now also checks both owner and the immutable
  initialization sentinel as defense in depth. No state-layout or version bump
  is justified by that source-only hardening.

## Retained proof obligations, not merge blockers

- Dusk transaction rollback is the authority for restoring the persisted
  dispatch reentrancy flag on a trapped outer call. The suite now proves a
  failed insufficient-credit dispatch can be followed by a successful funded
  dispatch. A full injected failure matrix for every hook/payment phase remains
  desirable, but no partial-state persistence was demonstrated.
- The adversarial reentrancy fixture currently collapses nested errors to a
  boolean. Its fixed target/method and successful outer continuation make the
  current regression meaningful, but a stable VM error discriminator would
  make the oracle stronger.
- Agent generation cannot atomically pin EVM and Dusk reads to one shared
  state root. It snapshots the manifest, performs fail-closed live validation,
  binds the configured signer to the live policy before secret output, and
  accepts the ordinary post-validation race inherent in an unversioned remote
  control plane.

## Operational decisions

- Dispatch credit readiness is a **minimum-balance** invariant, not an exact
  cap. Funding remains additive and permissionless so third-party sponsors are
  never blocked. A concurrent sponsor can safely leave more than the minimum;
  a concurrent dispatch that leaves less makes deployment readiness fail. The
  script re-reads and reports the observed post-funding balance instead of
  claiming an exact target. Only the sponsor's deposited value is exposed to
  overfunding, so an atomic capped top-up is not justified. Sponsor credit is
  operational reserve only: public warp-route transfers must leave the route's
  pre-existing balance unchanged and therefore cannot consume that reserve.

- The `dusk-hyperlane-repro` environment exists and is protected by the owner
  reviewer. No source secret or matching self-hosted runner has been provisioned,
  so the manual trusted reproduction remains intentionally unavailable.
- Temporary branch protection requires only checks that can run today. The
  production-readiness and manual-dispatcher checks are promoted to required
  only after their environment, secret, and runner are provisioned and proven.
- Administrators retain the owner bypass (`enforce_admins=false`). That makes
  emergency owner merging possible; it does not turn a bypassed PR into an
  approved or production-validated change.
- The local bridge demo is intentionally not a durable transaction
  orchestrator. It exposes the prepared hash before propagation and forbids a
  blind retry, but a production operator still needs an atomic on-disk journal
  and exact-hash reconciliation of unfinished entries.

## Evidence policy

The previously completed full Dusk E2E/fault run remains evidence only for its
recorded runtime source anchors (`9058755927473239d59ce702a8074acbae0e0a24`
for PR #1 and `dc8aba07773993878edd81735d59e66beddd66a3` for PR #10). The final
heads require fresh compile, unit/integration, hosted-check, independent xhigh,
and Controlecentrum receipts recorded separately.
