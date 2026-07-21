# Final red-team decisions (2026-07-21)

This record explains which findings from the independent GPT-5.6 xhigh and
Controlecentrum deep reviews were accepted, rejected, or retained as proof
obligations. It applies to the live heads of Hyperlane Dusk PRs #1, #3, and
#10 and companion monorepo PR #1. Runtime evidence remains attached to the
exact tested source refs; a later documentation or workflow commit is not
silently relabeled as having run those tests.

## Accepted and fixed

- **Trusted CI identity:** the privileged policy gate now queries the exact
  workflow file, pull-request event, and proposed head SHA. A same-name check
  from another GitHub Actions workflow cannot satisfy the gate. The proposed
  Dusk guard scripts are also byte-locked to the base branch after bootstrap.
- **Bootstrap scope:** the first guard introduction permits one explicit
  six-file bootstrap set. An empty computed worklist is no longer treated as
  proof that arbitrary changes are safe.
- **IGP deployment preflight:** CLI admission applies the contract's minimum
  quote, checked-arithmetic, and maximum-quote rules before loading signer
  material or submitting any deterministic deployment.
- **Transaction reconciliation:** transport/server failures remain retryable;
  deterministic HTTP-client and successful-response schema incompatibilities
  terminate immediately while preserving the exact transaction hash.
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
  validators-and-threshold query used by warm validation.
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

- The `dusk-hyperlane-repro` environment exists and is protected by the owner
  reviewer. No source secret or matching self-hosted runner has been provisioned,
  so the manual trusted reproduction remains intentionally unavailable.
- Temporary branch protection requires only checks that can run today. The
  production-readiness and manual-dispatcher checks are promoted to required
  only after their environment, secret, and runner are provisioned and proven.
- Administrators retain the owner bypass (`enforce_admins=false`). That makes
  emergency owner merging possible; it does not turn a bypassed PR into an
  approved or production-validated change.

## Evidence policy

The previously completed full Dusk E2E/fault run remains evidence only for its
recorded runtime source anchors (`9058755927473239d59ce702a8074acbae0e0a24`
for PR #1 and `dc8aba07773993878edd81735d59e66beddd66a3` for PR #10). The final
heads require fresh compile, unit/integration, hosted-check, independent xhigh,
and Controlecentrum receipts recorded separately.
