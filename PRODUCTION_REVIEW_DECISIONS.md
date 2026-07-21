# Production Review Decisions

This file is the reviewer-facing decision record for the Dusk Hyperlane
revival. It should be updated when reviewers accept a recommendation or request
a different implementation.

Do not treat unchecked items as production approval.

## Contract Policy Decisions

### Mailbox Moonlight Sender Resolution

Tracking issue: dusk-network/hyperlane-dusk#4.

Decision:

- [ ] Accept current implementation.
- [ ] Request a Rusk-level discriminator or another sender-resolution design.

Current implementation:

- If `Mailbox.dispatch` is reached through the Dusk transfer contract during a
  Moonlight contract-call transaction, the sender is resolved as
  `keccak256(abi::public_sender().to_bytes())`.
- Other inter-contract calls resolve to the immediate caller `ContractId`.

Evidence:

- `SECURITY_REVIEW.md`, "Open Production Review Decisions".
- `test_dispatch_via_transaction`.
- `test_dispatch_via_recipient_proxy`.
- Clean-Rusk TestMock and MessageIdMultisig E2E in `TEST_REPORT.md`.
- Latest clean-layout repro evidence at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043,
  including 72 VM integration tests and the Hyperlane Rust agent check.

Recommended stance:

Accept for v1 only if Rusk maintainers confirm the transfer contract cannot
call arbitrary user contracts for non-user-initiated reasons with an unrelated
`public_sender`.

### Immutable Account Registration

Tracking issue: dusk-network/hyperlane-dusk#5.

Decision:

- [ ] Accept immutable `registered_accounts` for v1.
- [ ] Request deregistration or key-rotation semantics before release.

Current implementation:

- Dusk external recipients are keyed by `keccak256(bls_public_key_bytes)`.
- Registration stores the BLS public key for that hash.
- There is no deregistration or remapping path.

Evidence:

- `SECURITY_REVIEW.md`, "Address mapping".
- `SECURITY_REVIEW.md`, "Open Production Review Decisions".
- WarpDrc20, WarpDrc20Collateral, and WarpNative registration tests.
- Latest clean-layout repro evidence at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043,
  including 72 VM integration tests and the Hyperlane Rust agent check.

Recommended stance:

Accept immutable registration for v1. A different BLS key naturally produces a
different recipient hash, and admin-controlled remapping would introduce a
privileged path over user recipient identity.

### Dispatch Fee Credit Ownership and Withdrawal

Tracking issue: dusk-network/hyperlane-dusk#2.

Decision (accepted 2026-07-20):

- [x] Keep credit authority with the effective payer and allow withdrawal to
  an explicit Moonlight account.
- [x] Give each production warp route an owner-only proxy for its own
  contract-keyed credit.
- [x] Do not give the Mailbox owner a global credit-drain power.
- [x] Defer contract-recipient withdrawal until a callback ABI and receiving
  contract requirements are specified.

Semantics:

- `fund_dispatch` remains permissionless. A third party may fund a user or
  route, but funding does not create a separate refund claim: the resulting
  credit belongs to the named payer identity.
- A direct Moonlight `withdraw_dispatch_credit` call resolves the signing
  account as the payer. An inter-contract call resolves the immediate calling
  contract as the payer. Callers cannot supply or impersonate a different
  payer.
- The payer selects an explicit Moonlight public key as recipient. State is
  debited before the transfer-contract call, and the transaction reverts both
  changes if that transfer fails. The key must also pass Dusk's semantic BLS
  validity check before any credit is debited; an identity or invalid point is
  rejected.
- `dusk-tx withdraw-dispatch` defaults to the signer but accepts
  `--recipient-public-key` so production routes can use a distinct operational
  treasury without changing route ownership or signer custody.
- `dusk-tx fund-dispatch` and `withdraw-dispatch` report success only after the
  exact transaction hash is present in Rusk's ledger with no execution error.
  Moonlight nonce advancement is intentionally not treated as success because
  rejected contract calls are still spent transactions.
- Once the withdrawal transaction has been constructed, every submission error
  retains its exact hash. Preverification failures are labeled as occurring
  before propagation; propagation transport/read failures are labeled
  outcome-unknown and instruct the operator to reconcile that hash before any
  retry of the non-idempotent withdrawal.
- Transaction-result polling checks immediately, enforces a 60-second wall-
  clock deadline as the authoritative bound rather than stopping at a smaller
  attempt count, retries transient observation failures without losing the
  transaction hash, and caps the GraphQL response at 256 KiB. The generic
  `dusk-tx call` path uses the same execution-success boundary.
- WarpDrc20, WarpNative, and WarpDrc20Collateral expose the same method only to
  their configured owner. The nested Mailbox call can withdraw only that
  route's credit.

Evidence:

- `test_dispatch_credit_withdrawal_is_payer_owned_and_value_backed`.
  This test also proves invalid-recipient rollback and decodes the actual VM
  receipt event through the explorer data driver.
- `test_dispatch_credit_withdrawal_rolls_back_after_transfer_failure` forces
  the transfer contract to fail after the Mailbox credit debit and proves
  credit, custody, recipient balance, and later caller resolution are restored.
- `test_dispatch_credit_withdrawals_preserve_multi_payer_solvency`.
- `test_warp_drc20_owner_can_withdraw_route_dispatch_credit`.
- `test_warp_native_owner_can_withdraw_route_dispatch_credit`.
- `test_warp_collateral_owner_can_withdraw_route_dispatch_credit`.
- `dusk-tx` transaction-status response and exact-hash query tests.
- `dusk-tx` bounded-response, transient-retry, immediate-check, execution-
  failure, no-attempt-cap, submission-hash preservation, and nonce-exhaustion
  tests.
- Direct and all three owner-proxied VM withdrawal receipts are asserted below
  the documented 30,000,000-gas CLI default on the pinned current Rusk runtime.
- Clean-current-Rusk reproduction at implementation anchor
  `183b56a875e5c2962ef621937258b8e497baef2a`: 12 WASMs, production contract
  clippy, 29 type tests, 100 VM tests, 7 data-driver tests, 18 `dusk-tx` tests,
  release data-driver WASM, standalone E2E host build, and tracked-source
  secret hygiene all pass. The exact log SHA256 is
  `4b70209aeddd30fe161a71d5b83110d3b7c5a7de9a42d02e6b4e1d1fcb2f2e69`.

### Pending Escrow Without Admin Drain

Tracking issue: dusk-network/hyperlane-dusk#6.

Decision (accepted 2026-07-20):

- [x] Accept no admin drain/recovery path for pending escrow.
- [ ] Request a governed recovery design before release.

Current implementation:

- WarpNative and WarpDrc20Collateral escrow unregistered recipients by
  recipient hash. Both reserve aggregate pending liabilities against live
  route custody before accepting another inbound delivery.
- Synthetic WarpDrc20 leaves an unregistered recipient amount unminted until
  the matching Moonlight key or immediate contract caller proves the recipient
  type and claims it. It never guesses that an arbitrary H256 is a contract.
- WarpDrc20Collateral tracks aggregate pending liability and reserves that
  amount against live route custody. A direct delivery cannot consume token
  backing already promised to pending recipients.
- Only the matching BLS key can register and claim pending funds.
- Admins cannot drain pending user escrow.
- Pending claims do not expire. An invalid or permanently lost recipient
  identity can therefore reserve funds indefinitely.

Evidence:

- `SECURITY_REVIEW.md`, "Unregistered recipients".
- `test_warp_native_handle_escrows_unregistered_recipient`.
- `test_warp_native_escrow_accumulates`.
- `test_warp_native_pending_reserve_has_priority_over_direct_delivery`.
- `test_warp_synthetic_handle_escrows_unregistered_contract_recipient`.
- `test_warp_synthetic_contract_pending_accumulates_and_claims`.
- `test_warp_collateral_handle_escrows_unregistered_recipient`.
- `test_warp_collateral_claim_pending_transfers_after_registration`.
- Latest clean-layout repro evidence at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043,
  including 72 VM integration tests and the Hyperlane Rust agent check.

Recommended stance:

Accept no admin drain for v1 if Dusk wants a non-custodial failure mode. If Dusk
wants recovery for lost keys or wrong recipient hashes, design that separately
with governance, timelock, audit, and user-dispute rules.

This decision does not claim that lost-key or wrong-hash funds are recoverable.
It records that adding unilateral route-owner seizure is a worse default. A
future recovery design would need message/source provenance, an eligible refund
destination, a delay and dispute window, governance authorization, events, and
cross-chain replay/double-spend analysis.

### Permissionless Dispatch-Credit Funding

Decision:

- [ ] Accept permissionless funding keyed to the beneficiary identity.
- [ ] Restrict who may sponsor another sender before release.

Current implementation:

- Anyone may deposit native DUSK into another sender's Mailbox dispatch-credit
  balance.
- The funder receives no withdrawal or dispatch authority from that deposit.
- Only the beneficiary sender's dispatch consumes the balance; exact
  consumption removes its storage entry.
- The stacked dispatch-credit PR adds beneficiary-authorized withdrawal and
  does not grant the original funder a reclaim path.

Recommended stance:

Accept permissionless sponsorship. It supports relayer/operator funding without
creating an allowance or custody claim for the sponsor. Unwanted dust is paid
for by the sponsor and does not let them consume or redirect the beneficiary's
credit. If Dusk wants funder-reclaimable deposits, model those as a distinct
escrow product with explicit ownership rather than overloading fee credit.

## Operational Decisions

### Production Signer Custody

Tracking issue: dusk-network/hyperlane-dusk#7.

Decision:

- [ ] Accept `PRODUCTION_SIGNER_POLICY.md` Option A for internal/testnet use.
- [ ] Require external Dusk signer work before production.
- [ ] Block production until KMS/HSM or equivalent custody exists.

Evidence:

- `PRODUCTION_SIGNER_POLICY.md`.
- `SECRET_HANDLING.md`.
- `scripts/secret-hygiene-check.sh`.
- `make secret-hygiene`.
- `cargo test -p hyperlane-base dusk` in the companion monorepo, including the
  Unix loose-permission rejection case for `duskKey.keyFile`.
- Latest clean-layout repro evidence at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4449591043,
  including `make secret-hygiene` and the Hyperlane Rust agent check.
- Dependency-remediated clean-Rusk E2E evidence at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434118389.

Recommended stance:

Accept Option A only for internal review and testnet-style validation. Mainnet
production should explicitly decide whether raw-key presence in a local
`keyFile` or `keyEnv` source on relayer or validator hosts is acceptable. If it
is not acceptable, require external signer work before production. The current
branch reduces local-file risk by rejecting non-regular files and, on Unix,
group/world-readable `keyFile` paths before reading key material; that hardening
does not replace a custody decision.

### Contract State Migration

Decision:

- [ ] Accept fresh deployment for the versioned Merkle/escrow state model.
- [ ] Design and review an explicit in-place migration before production.

Current implementation:

- Every deployed Dusk contract exposes an explicit `state_version()`.
  MerkleTreeHook, TestMock, MessageIdMultisigISM, ValidatorAnnounce,
  ProtocolFee, AggregationHook, WarpNative, WarpDrc20Collateral, and
  TestRecipient require version 1. WarpDrc20 requires version 2 after adding
  aggregate pending synthetic supply capacity. Mailbox requires version 2 on
  this stacked withdrawal PR so an instance without
  `withdraw_dispatch_credit` cannot be reused. IGP requires version 2 because
  unknown destinations and zero pricing now fail closed.
- Existing serialized instances are not treated as compatible. Both demo
  `--skip-deploy` reuse boundaries validate the complete contract-version
  matrix and fail closed when any version is absent or unexpected. Semantic
  policy checks, including the live Mailbox default ISM and persisted IGP
  destination pricing, remain additional requirements; a legacy liveness query
  is never accepted as compatibility.
- The compatible contract set and Rust agent must be deployed from the pinned
  cross-repository heads recorded in the review documents.

Recommended stance:

Use a fresh deterministic deployment for this reassessment and do not claim an
in-place upgrade. A migration would need separate state-layout, rollback, and
live-data validation work.

### CI/Repro Runner Strategy

Tracking issue: dusk-network/hyperlane-dusk#8.

Decision:

- [ ] Accept the manual self-hosted runner proposal plus required
      status-check policy.
- [ ] Replace it with another Dusk private CI system and equivalent branch
      required status-check policy.
- [ ] Keep PR evidence local/manual and explicitly accept that branch
      required status checks are enforced but not release gates until the
      accepted CI/repro path is provisioned and passing.

Evidence:

- `CI_REPRO_STRATEGY.md`.
- `CI_REPRO_STRATEGY.md` `Admin Provisioning Runbook`, also mirrored in
  dusk-network/hyperlane-dusk#8:
  https://github.com/dusk-network/hyperlane-dusk/issues/8#issuecomment-4435830841.
- `.github/workflows/manual-repro-check.yml`.
- `.github/workflows/production-readiness-gate.yml`, the lightweight
  status-check candidate for `make production-readiness-guard`.
- `.github/workflows/dusk-review-policy-gate.yml`, the shared required
  status-check policy gate now required by both Dusk org default branches.
- Required status-check promotion completed on 2026-05-14:
  `dusk-network/hyperlane-dusk` requires `Dusk review policy gate` and
  `Production readiness guard`, and `dusk-network/hyperlane-monorepo` requires
  `Dusk review policy gate` and `Dusk agent validation`. `make gate-status`
  reports `missingRequiredStatusChecks: none` for both repos.
- dusk-network/hyperlane-dusk#3, the narrow default-branch dispatcher PR.
- `scripts/local-repro-check.sh`.
- `make repro-check-agent`.
- `make gate-status`.
- `actionlint .github/workflows/manual-repro-check.yml`.
- `actionlint .github/workflows/production-readiness-gate.yml`.
- `actionlint .github/workflows/dusk-review-policy-gate.yml`.
- `make production-readiness-guard`, which blocks while the Dusk repos have
  open/unapproved PRs or missing CI/default-branch workflow runner/secret
  provisioning, and while Dusk Cargo dependency-alert triage is unavailable or
  reports vulnerable locked versions, unparsed vulnerable ranges, or missing
  patched-version data. The default branches now have protected-branch review
  baselines and required status-check policy enabled.
- Workflow inputs for exact review heads: `dusk_ref`, `rusk_ref`, and
  `monorepo_ref`.
- Workflow run name includes the requested refs, and the workflow logs resolved
  Dusk, Rusk, and monorepo checkout heads before `make repro-check-agent`.
- The final repro step explicitly uses `shell: bash` and runs under
  `set -euo pipefail` before invoking `make repro-check-agent`.

Recommended stance:

Accept the manual self-hosted runner proposal for internal review once Dusk
provides the `dusk-hyperlane` runner and read-only `DUSK_ORG_READ_TOKEN`.
The token should be scoped only for source checkout of
`dusk-network/hyperlane-dusk`, `dusk-network/hyperlane-monorepo`, and
`dusk-network/rusk-private`; it must not be a signer, validator key, consensus
password, deployment key, relayer key, or image-publishing credential. Hyperlane
image-publishing/GitHub App credentials are not required for the internal Dusk
review path because the inherited Rust image workflow is skipped on the Dusk
fork and remains enabled for the later upstream PR path.
Keep branch protection, Actions-secret, runner-admin, and Dependabot-alert
visibility separate from the source-checkout token. The production-readiness
guard should remain blocked until those status APIs are visible through an
approved admin/security-read local `gh` credential or a separate
Dusk-approved CI credential such as `DUSK_STATUS_READ_TOKEN`; do not broaden
`DUSK_ORG_READ_TOKEN` beyond source checkout unless Dusk explicitly changes the
token policy. The status token, if used, is for the production-readiness
workflow only and must not be used by manual repro checkout steps or any Dusk
runtime process.
Keep the protected `main` and required-review baseline now enabled for
`dusk-network/hyperlane-dusk` and `dusk-network/hyperlane-monorepo`. The
proposed GitHub Actions contexts are already enforced: `Production readiness
guard` on `dusk-network/hyperlane-dusk` and `Dusk agent validation` on
`dusk-network/hyperlane-monorepo`, each alongside `Dusk review policy gate`.
If Dusk chooses another private CI system, record the replacement contexts in
#8 and #2 and override the guard's accepted context list accordingly. The
production readiness guard still expects at least two required status checks on
each protected default branch before it can pass, but the current blockers have
moved to acceptance, runner/token/status visibility, open reviews, open
decision issues, and internal merges.
When running it from the default branch, resolve the live Dusk and monorepo PR
heads immediately before dispatch and pass those exact SHAs as `dusk_ref` and
`monorepo_ref`; keep `rusk_ref` pinned to the reviewed clean Rusk commit unless
Dusk explicitly chooses a different Rusk reference. Record the workflow URL,
requested refs, resolved checkout heads, and pass/fail result in
`TEST_REPORT.md`. Promote it to required PR CI only after one stable manual run
is recorded there.

### Soak Acceptance

Tracking issue: dusk-network/hyperlane-dusk#9.

Decision:

- [ ] Accept the current 7282-second clean-Rusk high-volume soak.
- [ ] Require a longer or differently shaped soak before internal release.

Current evidence:

- `TEST_REPORT.md`, run id `1778541618`.
- 7 cycles.
- 20 EVM -> Dusk and 20 Dusk -> EVM transfers per cycle.
- 280 total transfers.
- 7282 seconds.
- The wrapper stopped before cycle 8 because the 120-minute time budget had
  been reached.
- Clean detached Rusk reference
  `c0c64db4659500d077bb253ad13acba0e347d3fc`.
- Durable local evidence archive with the 7282-second soak logs:
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4430670295.

Recommended stance:

Accept for internal review evidence. Require a longer or differently shaped
soak only if Dusk wants a stricter formal release gate before production
claims.

## Upstream Preparation Decision

Decision:

- [ ] Internal Dusk PRs reviewed and accepted; prepare upstream draft PR.
- [ ] Keep upstream preparation blocked.

Evidence:

- `GOAL_AUDIT.md`.
- `dusk-network/hyperlane-dusk#1`.
- `dusk-network/hyperlane-monorepo#1`.
- `dusk-network/hyperlane-dusk#2`.
- `docs/dusk-upstream-compatibility-review.md` in the monorepo fork.
- `make production-readiness-guard`, which reports whether any open upstream
  `hyperlane-xyz/hyperlane-monorepo` PRs already exist from
  `dusk-network:feat/dusk-support-v2` while internal blockers remain.

Required condition:

Do not prepare upstream Hyperlane PRs until the internal Dusk PRs are reviewed
and the production sign-off tracker is resolved or replaced with explicit
follow-up issues accepted by Dusk maintainers.
