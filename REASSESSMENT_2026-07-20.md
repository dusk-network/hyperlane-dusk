# Hyperlane Dusk Reassessment — 2026-07-20

This pass re-evaluated the port against current Hyperlane, Rusk, and Forge
rather than treating the May 2026 evidence as current.

## Synchronized references

- Hyperlane upstream and fork `main`:
  `58c5e11e1e5a6e0502c14a822f77e5fd378e3af9`.
- Rusk private `master` used for the clean VM checks:
  `bc281d2cd1e789db92e99bc59849c92363524e37`.
- Forge used by the contracts:
  `d1e39a16ad5e2cd0675c7aafa6e2c459310bcb1a` (Forge 0.3.0).

The Hyperlane agent feature was rebased onto that upstream Hyperlane commit.
The pre-rebase feature head is preserved in
`backup/dusk-support-v2-pre-sync-20260720` in the monorepo fork.

## Compatibility and correctness changes

- Migrated contracts to Forge 0.3 module-level event declarations and event
  trait implementations.
- Updated the integration harness to current Rusk Boreas execution policy and
  deployment APIs.
- Added the dependency patches required when current Rusk consumes these
  contracts as path dependencies.
- Removed the obsolete exact `Rusk-Version: 1.0.0-rc.0` request header. Current
  Rusk interprets a supplied value as a semver requirement; the old exact value
  rejects current nodes, while omitting the optional header permits current
  protocol negotiation.
- Rejected inbound Hyperlane uint256 token amounts above `u64::MAX` instead of
  silently truncating them to Dusk's token representation.
- Changed agent transaction outcomes to wait for ledger inclusion, report the
  actual gas spent, and preserve execution errors instead of treating mempool
  admission as successful execution.
- Moved contract-existence and balance reads off Rusk's deprecated owner/status
  routes onto contract metadata and the transfer contract's balance query.
- Made `make check` target the contract/type WASM crates rather than host-only
  workspace packages that cannot compile for `wasm32-unknown-unknown`.
- Closed the open workflow review request by moving dispatch inputs out of the
  shell script, checking the whole PR diff, and running actionlint.

## Verification

- All 11 contract WASM crates compile against the current stack.
- Contract/type WASM clippy passes.
- 29 type unit tests pass.
- 72 current-Rusk VM integration tests pass.
- `dusk-tx` builds successfully.
- `hyperlane-dusk` tests pass.
- The affected Hyperlane agent set (`hyperlane-base`, validator, relayer,
  scraper, and lander) passes `cargo check` with the Dusk chain crate enabled.

## Remaining production blockers

- ProtocolFee and IGP are accounting models, not DUSK payment enforcement.
  They do not collect/forward native value and their `post_dispatch` methods
  are directly callable. See `SECURITY_REVIEW.md`.
- Dusk indexers still synthesize zero block/transaction hashes from query-only
  contract history and do not provide event-backed provenance.
- `latest_checkpoint_at_block` returns current checkpoint state because the
  contract surface has no historical checkpoint query.
- The application operation verifier is a no-op and the Dusk lander path is
  explicitly unsupported.
- The agent crate depends on an adjacent Dusk types checkout, which is suitable
  for the paired internal repositories but not yet a self-contained upstream
  Hyperlane contribution.
- The existing signer custody, account registration, escrow recovery, runner,
  review, and production sign-off decisions remain open.

The May reports remain useful historical evidence, but their pinned commits
and “current head” wording must not be read as evidence for this synchronized
candidate.
