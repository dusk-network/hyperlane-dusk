# Hyperlane Dusk Reassessment — 2026-07-20

> **Superseded for the reopened candidate.** The independent closure red-team
> invalidated the “final” heads and evidence below. Current decisions and the
> replacement-evidence rule are in
> `CLOSURE_REASSESSMENT_DECISIONS_2026-07-20.md`. The historical references in
> this file remain useful only for regression comparison.

This pass re-evaluated the port against current Hyperlane, Rusk, Forge, and the
current Dusk DRC20 contract surface. It also implemented and validated the
contract changes identified by the first reassessment pass.

## Final status addendum — 2026-07-21

The final readiness hardening binds required check names to their originating
GitHub Actions workflow run and exact PR/head/base identity. This closes the
same-name/check-conclusion spoofing gap: only a completed `SUCCESS` from the
expected workflow path and event is accepted. The fail-closed self-test covers
lookalike names, skipped conclusions, wrong workflow provenance, and the manual
dispatcher's `pull_request` event.

The final base covered-tree anchor is
`aaad04937483897ffc0fcc77cfcedbc53bfee326`; the focused withdrawal stack was
validated at `db040e3f1eab4ba012a12a6be92c8f86268a993f`; and the synchronized
agent/E2E checkout was
`356cf22a592d1d657519b9cfd5f6af9148096972` on upstream
`67933966ed9c6f9e3d5ec095372e11414c82e4e7`. The base clean gate passed 13
WASMs, clippy, 29 type tests, 99 VM tests, 17 CLI tests, 5 data-driver tests,
the standalone operator compile, and secret hygiene. Its durable log is
`/tmp/hyperlane-dusk-base-repro-aaad049.log` (SHA-256
`95a6d2eff330df1c65873ee105a813ca11f0e679c5b4b58cc5ce055586d1b561`).
The final monorepo code/CI anchor is
`9e386e81851fd02df86d957fb9fce6f15d81df34`; its exact-pin gate log is
`/tmp/hyperlane-monorepo-agent-gate-9e386e8-20260721.log` (SHA-256
`583b7019906721c565ff383bdeef97fa2e7e99bb08c089363b5c6bc83282bd5d`).

The final live E2E was run only after fixing the harness to `exec` agent
binaries, ensuring that a stopped case cannot leave an orphaned relayer in the
next case. TestMock run `1784607919` and MessageIdMultisig run `1784608531`
each withdrew exactly one LUX from WarpDrc20's contract-keyed dispatch credit,
used the remaining credit, and delivered synthetic, native, and collateral
routes in both directions with exact custody, allowance, and protocol-fee
assertions. The multisig case used a real validator and signed checkpoint
metadata. The combined harness log SHA-256 is
`d796c471d024fbb3fce75fccddf01dfcaac426be45a25eb77de6c101e23948e7`;
all retained harness, deploy, warm-validation, relayer, and validator logs
passed the runtime secret scan. Earlier synchronization and E2E references
below are historical and are superseded by this addendum where they differ.

## Synchronized references

- Hyperlane upstream `main`:
  `197b1e0d1a7b7ee5539e9ad38a02a23a7eb0a0b3`.
- Hyperlane Dusk agent branch: `feat/dusk-support-v2` at
  `a931f75b3d23d2e15e75f2e064470a1a01289abb`, rebased onto that upstream.
- Clean Rusk private `master` used for VM and live checks:
  `bc281d2cd1e789db92e99bc59849c92363524e37`.
- Forge used by the contracts:
  `d1e39a16ad5e2cd0675c7aafa6e2c459310bcb1a` (Forge 0.3.0).

The Hyperlane agent feature was rebased onto that upstream Hyperlane commit.
The pre-rebase feature head is preserved in
`backup/dusk-support-v2-pre-sync-20260720` in the monorepo fork.

## What changed

### Shared Dusk caller model

All privileged contracts now use one `H256` principal representation. A root
Moonlight caller maps to `keccak256(public_sender)`, while a nested call maps to
the immediate `ContractId`. Query and shielded contexts are rejected. Mailbox,
ProtocolFee, IGP, MessageIdMultisigISM, WarpDrc20, WarpNative, and
WarpDrc20Collateral are initialized with the deployer's reachable Moonlight
identity and use the same positive and negative authorization behavior.

Transfer-contract callbacks additionally require a nested call frame, the
transfer contract as immediate caller, and the expected declared source. A
root Moonlight caller therefore cannot spoof a contract-to-contract payment by
supplying a forged callback payload.

### Value-backed dispatch hooks

Mailbox now owns native DUSK dispatch-fee credits keyed by the encoded sender.
`fund_dispatch` consumes a real Moonlight deposit. Dispatch calculates the
required and selected hook quotes, checks and consumes the sender's credit, and
transfers the exact quoted values to the hooks.

ProtocolFee and IGP prepare one pending payment only when invoked by their
authorized router, finalize accounting only after an authenticated transfer
callback, retain real DUSK custody, and can pay a Moonlight beneficiary through
the transfer contract. Direct calls and mismatched payments revert without
creating fee records.

A new static AggregationHook makes the required-hook topology equivalent to
Hyperlane's aggregate model. It invokes MerkleTreeHook and ProtocolFee, receives
their combined exact payment from Mailbox, and forwards each child quote. The
default hook is IGP, so every deployed demo dispatch exercises both the
required aggregation and gas-payment path.

The demo ensures each deployed warp route's Mailbox credit satisfies a minimum
ready balance. That balance is an operational reserve, not a public subsidy:
each synthetic, collateral, or native route call quotes its own dispatch,
collects that native-DUSK fee from the caller, forwards it through an
authenticated transfer callback, and proves the route's pre-existing credit is
unchanged afterward. Native calls deposit the bridge amount plus the fee; token
routes deposit the fee only. Permissionless sponsorship may still leave a route
above the readiness minimum. Base PR #1 intentionally stops at
beneficiary-keyed sponsorship; stacked PR #10 adds
beneficiary-authorized Moonlight withdrawal and owner-only proxies for each
route's own contract-keyed credit, without a funder reclaim right or
Mailbox-owner global drain.

### Current DRC20 compatibility

The current Dusk DRC20 ABI uses typed call structs and an external/contract
`Account` enum. Collateral locking also requires `approve` plus
`transfer_from`. The port's old tuple-based `transfer` calls were not compatible
with that contract surface.

The shared types crate now defines the current DRC20 account and call types.
WarpDrc20 implements `balance_of`, `allowance`, `approve`, `transfer`, and
`transfer_from` using those types. WarpDrc20Collateral locks via allowance
consumption and unlocks with the typed transfer call. The CLI exposes approval
and external/contract balance queries for deployment and validation.

### Deployment and operational tooling

`deploy-hyperlane` can deploy WarpDrc20, WarpNative, and
WarpDrc20Collateral in one deterministic topology. The collateral route may
wrap the simultaneously deployed WarpDrc20. The demo deploys corresponding EVM
synthetic routes, enrolls all router pairs, registers the Dusk account on every
route, funds their dispatch credits, and records all contract IDs.

`demo/start-env.sh` now always lets Cargo verify the release CLI and every
contract WASM before starting services. This prevents an existing but stale
binary or a single existing Mailbox WASM from silently selecting an older
deployment topology.

Saved-deployment reuse now treats the Dusk topology as one compatibility unit.
Every deployed contract exposes a persisted-layout version; both reuse
boundaries validate the complete matrix (WarpDrc20 and IGP version 2, all other
current contracts version 1) before generating agent configuration. The live
Mailbox default-ISM and exact IGP destination-pricing checks remain separate
policy-binding requirements. Consequently,
legacy contracts that merely retain an old liveness query cannot be accepted as
compatible with the current escrow, accounting, or validator-policy semantics.

The earlier current-stack compatibility work remains part of this candidate:
Forge 0.3 event declarations, current Rusk VM/deployment APIs, semver-compatible
Rusk requests, checked token amount conversion, ledger-confirmed transaction
outcomes, current contract metadata/balance queries, and the split-Rusk
checkout guard.

## Verification

- Final clean-layout static gate at
  `aaad04937483897ffc0fcc77cfcedbc53bfee326`, against exact Rusk
  `5c6a0bab11c61fb4c81275afdeceb97fb942d85e`, passed with durable log
  `/tmp/hyperlane-dusk-base-repro-aaad049.log` (SHA-256
  `95a6d2eff330df1c65873ee105a813ca11f0e679c5b4b58cc5ce055586d1b561`).
- All 12 contract WASM crates compile against the current stack.
- Contract/type WASM clippy passes.
- `hyperlane-dusk-types`: 29 passed, 0 failed.
- `cargo test -p hyperlane-dusk-integration-tests`: 99 passed, 0 failed.
- `dusk-tx`: 17 passed, 0 failed.
- `hyperlane-dusk-data-driver`: 5 passed, 0 failed.
- The standalone E2E operator binary compiles, and the fail-closed and secret
  hygiene gates pass.
- The VM suite covers reachable and rejected admin calls, authenticated and
  spoofed fee callbacks, fee custody/claims, aggregate hook payments, native
  custody, current-ABI DRC20 allowance/collateral custody, explicit IGP pricing,
  zero-dependency rejection, and zero-router rejection.

### Earlier live E2E evidence

- Fresh live TestMock and MessageIdMultisig agent E2Es both passed against the
  synchronized Hyperlane monorepo and clean current Rusk.
- Each live case delivered all three route types in both directions:
  synthetic WarpDrc20, native DUSK/WarpNative, and DRC20 collateral.
- Native sends asserted the exact transfer-contract balance after lock and
  return. Collateral sends asserted the exact owner debit/credit, allowance
  consumption, and route-contract custody. Three outbound Dusk messages
  produced exactly three ProtocolFee collections.
- The MessageIdMultisig case used a real validator and checkpoint-signature
  metadata; the TestMock case independently exercised the same route/custody
  matrix without validator metadata.

The live runs used a fresh state archive, matching current-Rusk consensus keys,
contract WASMs built from the same Rusk checkout as the node, and clean service
shutdown. They executed the Dusk agent tree at
`eaa43c3c4decdf007085b19ec6b7d586f150457e`. The final upstream-only rebase
created `a931f75b3d23d2e15e75f2e064470a1a01289abb`; a covered-path diff between
those heads is empty, and the Dusk chain plus base, validator, relayer, scraper,
and lander packages pass `cargo check` at the final head.

Those live runs predate the IGP version-2 pricing and deployment-interface
change. They remain valid only for their pinned heads; the combined final stack
must repeat both live modes before merge.

## Remaining production work

The previously reported frozen-owner, unbacked-fee, unwired-hook, current-DRC20
ABI, and missing live native/collateral-route blockers are resolved by this
branch. The remaining work is operational or belongs to the agent/indexing
surface:

- Dusk indexers still synthesize zero block and transaction hashes from
  query-only history instead of event-backed provenance.
- `latest_checkpoint_at_block` still reports current checkpoint state because
  the contract exposes no historical checkpoint query.
- The application operation verifier remains a no-op and the Dusk lander path
  remains explicitly unsupported.
- The agent crate still depends on an adjacent Dusk types checkout, which is
  appropriate for the paired internal repositories but is not a self-contained
  upstream Hyperlane contribution.
- Production signer custody, route funding/refund policy, account-registration
  UX, upgrade/migration policy, monitoring, and Dusk release sign-off remain
  explicit deployment decisions.
- The existing stress, fault-injection, and soak evidence remains historical
  evidence for its pinned commits. It should be repeated after later changes to
  the agent or runtime, even though both current live route matrices pass.

The May reports remain useful historical evidence only for their pinned
commits. This document and the current section of `TEST_REPORT.md` supersede
their older “current head” conclusions.
