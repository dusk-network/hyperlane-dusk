# Security notes

What the contracts assume about the Dusk platform, where they deviate from
Hyperlane's Solidity contracts, which issues internal review found and fixed,
and which design decisions are still open.

## Platform assumptions

- **Reentrancy is possible.** The Dusk VM (Piecrust) reuses a contract's
  active instance when a callee calls back into it during the same
  transaction. Any state that spans an `abi::call` must be protected
  explicitly. Rolling back the transaction does not help if the callback
  returns successfully after corrupting an invariant.
- **Cross-contract calls** (`abi::call`) are synchronous and return a result.
  Unhandled failures revert the whole transaction.
- **Moonlight transactions** are public, BLS-signed transactions.
  `abi::public_sender()` returns the signer's BLS key, and the transaction's
  `deposit` field is how DUSK moves from an account into a contract.
- **Overflow.** The workspace sets `overflow-checks = true` for release
  builds. Security-relevant arithmetic also uses explicit checked operations.
- **Host queries.** All on-chain hashing uses the `keccak256` host query.
  `ism-multisig` and `validator-announce` use `secp256k1_recover`. Both must be
  active on the target network.

## Deviations from Solidity Hyperlane

| Area | Dusk behaviour |
|---|---|
| Caller identity | No `msg.sender`. A shared resolver maps a root Moonlight call to `keccak256(public_sender)` and a nested call to the caller's `ContractId`, both as `H256`. All owner checks use this. |
| Native value | No `msg.value`. WarpNative and Mailbox fee funding check the exact Moonlight deposit through the transfer contract. Hook payments use authenticated contract-to-contract transfers. |
| Fees | Mailbox spends prepaid, sender-keyed credit (`fund_dispatch`). The required hook is a static AggregationHook (MerkleTreeHook + ProtocolFee). IGP is the default hook. ProtocolFee and IGP only record a payment after an authenticated transfer of the exact quoted amount, and hold real DUSK until claimed. |
| Account recipients | An account is `keccak256(bls_public_key)` and must `register_account` on a route before it can receive. Transfers to unregistered recipients go to escrow, claimable only by the matching key. |
| Reverts | `assert!` and `expect(...)` panics instead of custom errors. |
| Upgrades | None. Contract IDs are deterministic; an upgrade is a new deployment. Redeploying over existing state is refused. |
| Multisig metadata | Stricter than Solidity: validators must be sorted, the threshold valid, and the signature section an exact multiple of 65 bytes. |
| Unsupported | Routing, Aggregation and CCIP-read ISMs; rate-limited hooks and ISMs. |

## Issues found and fixed

| ID | Issue | Fix |
|---|---|---|
| HIGH-1 | WarpNative `transfer_remote` did not check that the caller had deposited the DUSK being bridged. | Verifies the exact Moonlight deposit through the transfer contract. |
| HIGH-2 | WarpNative `handle` panicked for unregistered recipients, so the message could never be delivered. | Unregistered recipients go to escrow; `claim_pending` after registering. |
| HIGH-3 | WarpDrc20Collateral treated every recipient as a contract. | Added account registration and escrow, matching WarpNative. |
| HIGH-4 | WarpDrc20 `mint`, `burn` and `transfer` could overflow. | Checked arithmetic. |
| HIGH-5 | A hook could re-enter `dispatch` from `quote_dispatch`, desynchronising nonces from stored messages and permanently stalling Dusk-origin relaying. | Persisted dispatch guard across all hook and payment callbacks. |
| HIGH-6 | `process` asked the recipient for its ISM before marking the message delivered, so a recipient could re-enter `process` and have the same message handled twice. | Delivery is reserved before any external call (PR #11). |
| MEDIUM-1 | IGP quote truncated `u128` cost with `as u64`. | `u64::try_from`, reverting on overflow. |
| MEDIUM-2 | ProtocolFee, IGP, Mailbox fee total and Mailbox nonce could overflow. | Checked arithmetic; dispatch fails on nonce exhaustion. |
| MEDIUM-3 | Multisig ISM ignored trailing partial-signature bytes and did not reject an uninitialised validator set. | Exact-length check and explicit initialisation check. |
| MEDIUM-4 | IGP gas-payment records left out the overhead that was quoted and charged. | Records the full gas plus overhead (PR #11). |
| LOW-1 | WarpDrc20 accepted zero-amount transfers. | Rejected. |

The fixes have regression tests in `tests/tests/integration.rs`, except the
overflow paths listed under known limitations. HIGH-5 and HIGH-6 use the
`reentrant-hook` contract to attempt the real nested call.

## Design decisions

Accepted:

- **Dispatch credit belongs to the payer.** Anyone can fund credit, but only
  the effective payer can withdraw it, and only to a Moonlight account. Warp
  routes expose an owner-only withdrawal of their own credit. There is no
  global drain by the Mailbox owner.
- **No admin recovery of escrow.** Pending transfers can only be claimed by
  the matching BLS key. If a user loses that key, the funds stay locked. An
  admin drain would add a path to seize user funds.

Recommended, not yet confirmed:

- **Immutable registration.** Registrations can't be removed or remapped. A
  new key produces a new recipient hash anyway, and remapping would give an
  admin control over where user funds go.
- **Sender resolution through the transfer contract.** When `dispatch` is
  reached from a Moonlight transaction (via the transfer contract), the sender
  is `keccak256(public_sender)`; any other caller is its `ContractId`. This
  is correct only if the transfer contract never calls user contracts on its
  own initiative with an unrelated `public_sender`. That needs confirming with
  the Rusk maintainers.

## Known limitations

- The Mailbox stores every dispatched message and processed ID, so its state
  grows without bound. Agents read messages from it. This mirrors the
  Solidity Mailbox.
- `dispatch` is permissionless, as in Hyperlane. Security comes from the
  destination ISM.
- Dispatch credit can only be withdrawn to Moonlight accounts, not to
  contracts.
- The live E2E runs a 1-of-1 multisig. 2-of-3 is covered by VM tests only.
- Not tested in the VM: Mailbox nonce exhaustion (needs about 4 billion
  dispatches), and the WarpDrc20 and IGP checked-arithmetic panics, which are
  only reachable with an already-broken supply invariant or an absurd gas
  configuration. These are reviewed statically.
