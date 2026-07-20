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
- 74 current-Rusk VM integration tests pass.
- The VM harness now submits real Moonlight deposits and queries the transfer
  contract's DUSK custody. An exact WarpNative deposit locks DUSK, an inbound
  message releases it to the registered account, and a mismatched deposit
  reverts without leaving contract custody.
- `dusk-tx` builds successfully.
- `hyperlane-dusk` tests pass.
- The affected Hyperlane agent set (`hyperlane-base`, validator, relayer,
  scraper, and lander) passes `cargo check` with the Dusk chain crate enabled.
- Fresh live TestMock and MessageIdMultisig agent E2Es pass against current
  Rusk and the synchronized Hyperlane monorepo. Both cases delivered a
  WarpDrc20 transfer EVM -> Dusk and Dusk -> EVM; the multisig case used a real
  validator and checkpoint-signature metadata.

The live runs used a fresh state archive, current Rusk consensus keys, contract
WASMs built against the same Rusk checkout as the running node, and a clean
shutdown. The harness now fails closed when the contracts' relative Rusk path
dependency resolves to a different checkout than the node binary. This caught
an otherwise plausible but invalid mixed-Rusk E2E attempt during this pass.

## Lessons from DuskEVM

- DuskEVM treats native value as custody, not an integer supplied by the
  caller. Its tests submit a transaction deposit and assert the transfer
  contract's balance. The WarpNative VM tests now follow that pattern.
- Multi-contract value routing in DuskEVM uses explicit per-transaction
  escrow/preload, exact consumption, clearing, and refund. ProtocolFee and IGP
  need equivalent value-backed semantics before their counters or events can
  represent payment.
- DuskEVM authenticates transfer-contract callbacks with both the immediate
  caller and call-stack context. A future hook-payment callback must not assume
  that seeing the transfer contract as caller alone proves a contract-to-
  contract payment.
- DuskEVM represents privileged principals as either a public account or a
  contract. Hyperlane's current mix of `ContractId`-only and `H256` ownership
  models should be replaced by one explicit principal model before deployment
  administration is considered usable.

## Remaining production blockers

- ProtocolFee and IGP are accounting models, not DUSK payment enforcement.
  They do not collect/forward native value, and a direct `post_dispatch` call
  can create an unbacked payment record. Public hook entrypoints are compatible
  with Hyperlane, but records must be tied to authenticated, actually escrowed
  value. The demo deployment also leaves these fee hooks unwired.
- Most privileged Dusk contracts accept only a calling `ContractId` as owner,
  while deployment assigns Mailbox/self ownership and Mailbox exposes no admin
  forwarding surface. Mailbox, ProtocolFee, IGP, MessageIdMultisigISM,
  WarpNative, and WarpDrc20Collateral therefore have unreachable or frozen
  administration in the deployed topology. WarpDrc20's `H256` owner handling
  is the exception, not a shared solution.
- Dusk indexers still synthesize zero block/transaction hashes from query-only
  contract history and do not provide event-backed provenance.
- `latest_checkpoint_at_block` returns current checkpoint state because the
  contract surface has no historical checkpoint query.
- The application operation verifier is a no-op and the Dusk lander path is
  explicitly unsupported.
- The agent crate depends on an adjacent Dusk types checkout, which is suitable
  for the paired internal repositories but not yet a self-contained upstream
  Hyperlane contribution.
- The live agent E2E currently exercises the synthetic WarpDrc20 route. Native
  custody is validated in the current-Rusk VM, but live cross-chain WarpNative
  and WarpDrc20Collateral route tests are still missing.
- The existing signer custody, account registration, escrow recovery, runner,
  review, and production sign-off decisions remain open.

The May reports remain useful historical evidence, but their pinned commits
and “current head” wording must not be read as evidence for this synchronized
candidate.
