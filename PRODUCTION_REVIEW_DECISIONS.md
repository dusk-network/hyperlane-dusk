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
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434277572,
  including 70 VM integration tests and the Hyperlane Rust agent check.

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
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434277572,
  including 70 VM integration tests and the Hyperlane Rust agent check.

Recommended stance:

Accept immutable registration for v1. A different BLS key naturally produces a
different recipient hash, and admin-controlled remapping would introduce a
privileged path over user recipient identity.

### Pending Escrow Without Admin Drain

Tracking issue: dusk-network/hyperlane-dusk#6.

Decision:

- [ ] Accept no admin drain/recovery path for pending escrow.
- [ ] Request a governed recovery design before release.

Current implementation:

- WarpNative and WarpDrc20Collateral escrow unregistered recipients by
  recipient hash.
- Only the matching BLS key can register and claim pending funds.
- Admins cannot drain pending user escrow.

Evidence:

- `SECURITY_REVIEW.md`, "Unregistered recipients".
- `test_warp_native_handle_escrows_unregistered_recipient`.
- `test_warp_native_escrow_accumulates`.
- `test_warp_collateral_handle_escrows_unregistered_recipient`.
- `test_warp_collateral_claim_pending_transfers_after_registration`.
- Latest clean-layout repro evidence at
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434277572,
  including 70 VM integration tests and the Hyperlane Rust agent check.

Recommended stance:

Accept no admin drain for v1 if Dusk wants a non-custodial failure mode. If Dusk
wants recovery for lost keys or wrong recipient hashes, design that separately
with governance, timelock, audit, and user-dispute rules.

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
  https://github.com/dusk-network/hyperlane-dusk/issues/2#issuecomment-4434277572,
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

### CI/Repro Runner Strategy

Tracking issue: dusk-network/hyperlane-dusk#8.

Decision:

- [ ] Accept the manual self-hosted runner proposal plus required
      status-check policy.
- [ ] Replace it with another Dusk private CI system and equivalent branch
      required status-check policy.
- [ ] Keep PR evidence local/manual and explicitly accept that branch
      required status checks are not release gates yet.

Evidence:

- `CI_REPRO_STRATEGY.md`.
- `.github/workflows/manual-repro-check.yml`.
- dusk-network/hyperlane-dusk#3, the narrow default-branch dispatcher PR.
- `scripts/local-repro-check.sh`.
- `make repro-check-agent`.
- `make gate-status`.
- `actionlint .github/workflows/manual-repro-check.yml`.
- `make production-readiness-guard`, which blocks while the Dusk repos have
  open/unapproved PRs, no required status checks, or missing CI/default-branch
  workflow visibility. The default branches now have protected-branch review
  baselines enabled.
- Workflow inputs for exact review heads: `dusk_ref`, `rusk_ref`, and
  `monorepo_ref`.
- Workflow run name includes the requested refs, and the workflow logs resolved
  Dusk, Rusk, and monorepo checkout heads before `make repro-check-agent`.
- The final repro step explicitly uses `shell: bash` and runs under
  `set -euo pipefail` before invoking `make repro-check-agent`.

Recommended stance:

Accept the manual self-hosted runner proposal for internal review once Dusk
provides the `dusk-hyperlane` runner and read-only `DUSK_ORG_READ_TOKEN`.
Keep the protected `main` and required-review baseline now enabled for
`dusk-network/hyperlane-dusk` and `dusk-network/hyperlane-monorepo`, then add
at least one required status check once the accepted CI path exists.
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

Required condition:

Do not prepare upstream Hyperlane PRs until the internal Dusk PRs are reviewed
and the production sign-off tracker is resolved or replaced with explicit
follow-up issues accepted by Dusk maintainers.
