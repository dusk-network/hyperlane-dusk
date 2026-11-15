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
  `abi::public_sender()` returns the signer's BLS key (and panics outside a
  transaction, see Known limitations), and the transaction's `deposit` field
  is how DUSK moves from an account into a contract.
- **Public keys in arguments are unchecked.** The archive check accepts any
  limbs in a BLS `PublicKey`. Its compressed encoding (`to_bytes`), which
  recipient hashes cover, depends only on x reduced mod p, the infinity flag
  and the sign of y. The transfer contract and DRC20 balances key accounts by
  the raw limbs (`to_raw_bytes`). A forged key can therefore share an
  account's hash and still pay somewhere else. Decode a caller's key from its
  compressed bytes before hashing, storing or paying it.
- **Overflow.** The workspace sets `overflow-checks = true` for release
  builds. Security-relevant arithmetic also uses explicit checked operations.
- **Failed transactions pay their whole gas limit**, and so does a failed
  inter-contract call within a transaction. Bound calls into untrusted
  contracts with `call_with_limit`, and check before submitting.
- **Contract memory.** A wasm32 contract's state is its linear memory: at
  most 4 GiB, and it never shrinks. Anything a caller can make a contract
  store must be bounded or priced.
- **Host queries.** All on-chain hashing uses the `keccak256` host query.
  `ism-multisig` and `validator-announce` use `secp256k1_recover`. Both must be
  active on the target network.

## Deviations from Solidity Hyperlane

| Area | Dusk behaviour |
|---|---|
| Caller identity | No `msg.sender`. A shared resolver maps a root Moonlight call to `keccak256(public_sender)` and a nested call to the caller's `ContractId`, both as `H256`. All owner checks use this. |
| Native value | No `msg.value`. WarpNative and Mailbox fee funding check the exact Moonlight deposit through the transfer contract. Hook payments use authenticated contract-to-contract transfers. |
| Fees | Mailbox spends prepaid, sender-keyed credit (`fund_dispatch`). Warp routes fund each dispatch from the sender's deposit and assert their own credit is unchanged. The required hook is a static AggregationHook (MerkleTreeHook + ProtocolFee). IGP is the default hook. ProtocolFee and IGP only record a payment after an authenticated transfer of the exact quoted amount, and hold real DUSK until claimed. |
| Account recipients | An account is `keccak256(bls_public_key)` and must be registered on a route before it can receive. Transfers to unregistered recipients go to escrow, which is only ever paid to the key with that hash. |
| Reverts | `assert!` and `expect(...)` panics instead of custom errors. |
| Upgrades | None. Contract IDs are deterministic; an upgrade is a new deployment. Redeploying over existing state is refused. |
| Multisig metadata | Stricter than Solidity: validators must be sorted, the threshold valid, and the signature section an exact multiple of 65 bytes. |
| Default ISM | One MessageIdMultisig ISM verifies messages from every origin, since Routing ISMs are unsupported. Any origin whose checkpoints the configured validators sign is trusted by the Mailbox; warp routes still accept only their enrolled router per origin. |
| Recipient ISM lookup | Runs with 1M gas. A recipient without `interchain_security_module`, or whose getter panics, gets the default ISM, as in Solidity. A getter that runs out of gas fails the delivery instead, so the submitter's gas limit can't pick the ISM. |
| Dispatched messages | The Mailbox stores each message's ID and block height, not the message. Agents read the message from the finalized `Dispatch` event and check it against the stored ID. `dispatch` closes once the Mailbox's memory reaches 3 GiB of its 4 GiB; `process` and claims keep working. |
| Escrow claims | A recipient claims its own escrow with `claim_pending`, which registers the transaction's sender and pays it, for about 1.9M gas. Anyone can call `claim_pending_for(key)` and pay the gas. The route decodes `key` from its compressed encoding and reverts if that isn't a valid key; the escrow lookup, the registration and the payout all use the decoded key, the one the recipient signs with. The decoding costs about 57M gas, so that claim takes about 59M. Either claim reverts with nothing in escrow and then registers nothing, so neither can register a key nobody sent anything to, and routes have no other call that registers someone else. |
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
| MEDIUM-5 | TestRecipient, which `deploy-hyperlane` always deploys, let anyone set its ISM, for example to the always-true TestMock. Unsigned messages to it were then delivered by the real Mailbox. | The setter is removed; TestRecipient always uses the Mailbox's default ISM. |
| LOW-3 | WarpDrc20 let holders transfer to Phoenix principals, which no call can ever spend from. | Phoenix recipients are rejected. |
| LOW-2 | The multisig ISM and ValidatorAnnounce accepted signatures OpenZeppelin rejects (`v` of 0/1, high `s`), because the host query does. No double counting was possible. | Both require canonical signatures (`types::signature::is_canonical`). |
| HIGH-7 | The Mailbox stored every dispatched message in its wasm32 memory, which is capped at 4 GiB and never shrinks. About 57k maximum-size dispatches (a few hundred DUSK) would fill it, after which `process` could not allocate either and the bridge stopped both ways. | Only the message ID is stored, and `dispatch` closes at 3 GiB (`memory_bytes` reports usage). |
| MEDIUM-6 | The Mailbox looked up a recipient's ISM with an unbounded call. A recipient without the getter was charged about 93% of the remaining gas, roughly 26M of a 30M transaction. | 1M gas allowance; running out of it fails the delivery rather than falling back. |
| MEDIUM-7 | A recipient whose only DUSK was in escrow couldn't pay gas to claim it, so every first-time Dusk user needed DUSK from elsewhere. | `claim_pending_for`, payable by anyone. |
| MEDIUM-8 | Dusk charges a failed transaction its whole gas limit. The relayer submitted `process` without a pre-check, so a message whose delivery always fails (for example to the zero recipient) cost 30M gas on every retry. | The relayer simulates `process` as a read-only query first (hyperlane-monorepo). |
| LOW-4 | Contract WASMs embedded the builder's absolute paths and weren't reproducible, so nobody could rebuild a release and check a deployed contract ID. | Paths remapped; one codegen unit with LTO (also 30–45% smaller). |
| LOW-5 | `dusk-tx transfer-remote` accepted any 32-byte recipient. A right-padded EVM address passed Dusk's checks, the EVM token rejected it, and the DUSK stayed locked. | `dusk-tx` takes a 20-byte address and rejects non-zero upper bytes unless `--raw-recipient`. |
| CRITICAL-1 | `claim_pending_for` and `register_account_for` used the caller's `PublicKey` as given. A twin of the recipient's key (a changed y limb, or x.c0 stored as x.c0 + p, which even passes `is_valid`) has the recipient's hash but other raw bytes, so anyone could pay an escrow, or every later transfer to a registered account, into an account nobody can spend from. On all three routes. | Routes decode the key from its compressed encoding and use only the decoded key. |
| MEDIUM-9 | `register_account_for` registered any key for about 39k gas, enough to fill a route's 4 GiB of memory for about 500 DUSK, after which new escrow entries couldn't be allocated. | Removed. Only `register_account` (the signer) and a claim of existing escrow register keys. |
| LOW-6 | The relayer sent `process` with a 30M gas limit. A failed transaction pays its whole limit, so anyone who delivered a message first cost the relayer 30M gas. | The relayer's limit is 5M (`DUSK_RELAYER_GAS_LIMIT`), checked against the ISM threshold. |

The fixes have regression tests in `tests/tests/integration.rs`, except the
overflow paths listed under known limitations. HIGH-5 and HIGH-6 use the
`reentrant-hook` contract to attempt the real nested call.

## Second review

A second internal review of the fixes for CRITICAL-1, MEDIUM-9 and LOW-6,
before the public testnet, found no new critical or high issue. It found
these, some of them in the fixes' own claims:

| ID | Issue | Status |
|---|---|---|
| MEDIUM-10 | Routes still register any number of keys. Without `register_account_for` (MEDIUM-9) every entry costs a transaction signed by its key, but anyone can fund new keys and call `register_account` from each, and no entry is ever removed. A test said that nobody could fill a route's memory with registrations. | **Open**, see [below](#open-registry-growth). The test now says what holds. |
| MEDIUM-11 | Routes deployed before the CRITICAL-1 fix reported the same `state_version` as fixed ones, so `deploy/testnet.sh deploy` and `demo/deploy.sh --skip-deploy` would reuse a recorded deployment whose sponsored claims still pay forged keys. | Fixed. Routes are at `state_version` 4 (WarpNative), 6 (WarpDrc20) and 5 (WarpDrc20Collateral), and both scripts refuse a recorded route below that and say to redeploy. `agent-configs` and `preflight` check it too. |
| LOW-7 | The Mailbox's `withdraw_dispatch_credit`, and each route's owner-only wrapper of it, pay the recipient key as given. A payer who passes a twin of the recipient's key (see CRITICAL-1) is debited and the event names the recipient, but the DUSK goes to an account nobody can spend from. Only the caller's own credit, or a route's credit by its owner, is at risk. | **Open.** The fix belongs in the Mailbox, which waits on a separate study of its storage. |
| LOW-8 | `claim_pending` paid the sender but didn't register it, unlike `claim_pending_for`, so every later transfer to that account went to escrow again. The contracts' comments said a recipient had to register before claiming, which it didn't. | Fixed. `claim_pending` registers the sender, then pays it. |
| LOW-9 | `dusk-tx claim-pending` called `claim_pending_for` with an 80M gas limit even for the signer's own escrow: about 59M gas where 1.9M would do, and 80M, 0.08 DUSK, when the claim failed. | Fixed. The signer's own claim (no `--for`, or `--for` naming the signer) calls `claim_pending` with a 3M limit: 1.5 times the costliest route's 1.91M, rounded up. |
| LOW-10 | `deploy/testnet.sh` checked the relayer's gas limit against `EVM_ORIGIN_THRESHOLD`, not against the threshold deployed on chain, which the owner can raise later. | Fixed. Once the contracts are deployed it reads the threshold of the ISM the route uses (its own, or the Mailbox's default) and checks the larger of the two. |
| LOW-11 | When a recorded Mailbox wasn't on chain, for example after a testnet reset, `deploy/testnet.sh` exited without a message: under `set -e` and `pipefail` the failed query ended the script before its error. Found while fixing MEDIUM-11. | Fixed. |

The review also asked for tests that had been missing, now in
`tests/tests/integration.rs`:

- `claim_pending_for` with a key whose raw infinity byte is 2, which gives a
  compressed encoding with the infinity flag next to a non-zero x, and with a
  point on the curve but outside the prime-order subgroup. Both revert on all
  three routes, with escrow waiting under the hash the route computes for
  them, and change no escrow, registration or balance
  (`*_claim_for_undecodable_keys_changes_nothing`).
- A relayed `process` at threshold 6, the highest the 5M relayer limit
  allows, in a transaction whose gas limit is 5M rather than one measured
  under a bigger limit, on every route and both delivery paths
  (`test_relayed_process_at_threshold_6_fits_a_5m_gas_limit`). The costliest
  is a payout from a collateral route locking the upstream DRC20, at 3.27M.

### Open: registry growth

Each registration adds about 339 bytes to the route's memory (338.6 on
average, the map's own overhead included), and nothing removes it. The
memory is wasm32: at most 4 GiB, and it never shrinks. A route whose memory
is full of registrations can't allocate a new escrow entry or registration:
transfers to recipients that aren't registered can't be delivered, those
recipients can't claim, and WarpDrc20 can't mint to an account with no
balance yet. The registrations never go away, so neither does the problem.

`register_account` registers the transaction's signer, so an entry costs a
transaction signed by a new key, and that key needs DUSK for its gas first.
`test_self_registration_costs_two_transactions_per_key` measures the funding
transfer and the registration at about 1.85M gas each, nearly all of it the
1.75M signature check every Moonlight transaction pays at the Boreas
host-query prices testnet and mainnet use (the registration itself costs
about 35k). Filling 4 GiB takes about 12.7 million entries:

| Per entry | Gas | DUSK to fill 4 GiB at 1 LUX/gas |
|---|---|---|
| Registration only | 1.85M | about 23,500 |
| A funding transfer and the registration | 3.70M | about 46,900 |

This assumes the minimum gas price of 1 LUX, that the whole 4 GiB is free,
and one ordinary transfer to fund each key. Funding many keys from a
contract in one transaction costs less per key, so the registration-only
figure is the floor. Each key must also hold its registration's gas limit
when it submits, but keeps what isn't spent. It takes two transactions per
entry, about 15,600 blocks filled to their 3 billion gas limit. For
comparison, `register_account_for` (MEDIUM-9) did the same for about
500 DUSK.

This is the same class of problem as the Mailbox's process path after
HIGH-7, which stores every processed message ID and isn't covered by the
3 GiB dispatch limit: storage anyone can add to, bounded only by the cost of
the transactions. It stays open until the storage design being worked out
separately bounds or prices both. Routes don't report their memory use yet.

## Owner powers

`dusk-tx deploy-hyperlane` makes the deployer key the owner, fee beneficiary
and router admin of every contract. Whoever holds that key can:

- **Mint or release funds** by setting the Mailbox default ISM, the multisig
  validator set, a route's ISM or a route's enrolled routers. That drains
  WarpNative and collateral custody (except what is reserved for escrow) and
  mints synthetic tokens.
- **Freeze transfers** by removing the Merkle hook from the Mailbox's required
  hook (Dusk-origin messages become unprovable) or installing a reverting or
  overpriced hook or IGP quote.
- **Take fees**: unclaimed and future ProtocolFee and IGP fees, and each
  route's dispatch credit.

They can't touch users' own dispatch credit, pending escrow, or existing
WarpDrc20 balances. On testnet one deployer key is acceptable. Mainnet needs
the owner role on a multisig or equivalent custody.

## Design decisions

Accepted:

- **Dispatch credit belongs to the payer.** Anyone can fund credit, but only
  the effective payer can withdraw it, and only to a Moonlight account. Warp
  routes expose an owner-only withdrawal of their own credit. There is no
  global drain by the Mailbox owner.
- **No admin recovery of escrow.** Pending transfers are only ever paid to
  the matching BLS key, though anyone can pay the gas to claim them for it.
  If a user loses that key, the funds stay locked. An admin drain would add
  a path to seize user funds.
- **Immutable registration** (issue #5). No call removes a registration or
  points it at another key. Registering again overwrites the entry, but with
  the same key: every key a route stores is canonical, either the
  transaction's sender, which the node decodes from its compressed encoding,
  or a key `claim_pending_for` has decoded, and only one canonical key has a
  given hash. A new key produces a new recipient hash anyway, and remapping
  would give an admin control over where user funds go.
- **Sender resolution through the transfer contract** (issue #4). A frame
  whose only caller is the transfer contract (call-stack depth 1) is a
  Moonlight transaction's own top-level call, and its sender is
  `keccak256(public_sender)`. Any deeper frame resolves to its immediate
  caller's `ContractId`. Checked against the Rusk 1.7.3 transfer contract
  (`contracts/genesis/transfer/src/state.rs`): `spend_and_execute` is the only
  place it calls a contract chosen by the transaction, and it calls only
  `tx.call`. Its other outbound calls (`contract_to_contract`,
  `mint_to_contract`) are themselves nested calls, so the receiving contract
  sees depth 2 or more and resolves the sender to the transfer contract, never
  to a user. Phoenix transactions have no public sender and are rejected.
  Re-check this if a Rusk upgrade changes how the transfer contract calls out.

## Known limitations

- **Synthetic amounts are u64.** DRC20 balances are u64, so a WarpDrc20
  synthetic of an 18-decimal token can never hold more than about 18.4
  tokens. Inbound amounts above u64 are rejected, not truncated, which leaves
  them undeliverable after the EVM side has burned them. An attacker can also
  fill the supply cap by escrowing transfers to a recipient nobody can claim,
  freezing inbound transfers for everyone. Synthetic routes must therefore
  carry at most 9 decimals on Dusk, with the EVM side scaling (HypERC20's
  scale parameters). The native DUSK route is not affected: both sides use 9
  decimals and supply is bounded by DUSK custody. The demo's 18-decimal
  synthetic with a 10-token supply is a demo only. Tests:
  `redteam_synthetic_*` (ignored).
- **Contract IDs are predictable during deployment.** A contract ID depends on
  bytecode, owner key and nonce, and Rusk doesn't check that the owner signed
  the deploy. Some contracts are initialised with the IDs of contracts
  deployed after them. Anyone who sees the deployer's first transaction can
  deploy at the later IDs first; `dusk-tx` then aborts, and a new key can be
  front-run the same way. A denial of service, not a theft. The fix for
  mainnet is an owner-signed check in every `init`.
- **Undeliverable messages lock funds.** A transfer whose delivery always
  fails stays undelivered, as in Solidity Hyperlane. Examples: an EVM-side
  transfer to the zero recipient; a Dusk-side transfer to a 32-byte recipient
  the EVM token rejects. Tokens burned on the EVM side, or DUSK locked in the
  route, then belong to nobody. Escrow under a hash that no key produces
  (or for a contract that never calls `claim_pending_contract`) is locked the
  same way. Nothing expires or refunds.
- **Simulation is detectable.** The relayer simulates `process` as an
  unsigned query, where the gas limit is the block's, the height is the
  tip's and there is no transaction: `abi::public_sender()` panics, because
  Rusk 1.7.3 only sets the sender for a transaction
  (`rusk-private/core/src/abi.rs`). A hostile recipient can pass the
  simulation and fail the real delivery, costing the relayer the
  transaction's gas limit (5M), as gas estimation can be fooled on EVM.
  Enforce IGP gas payments for Dusk destinations on mainnet.
- **Recipients must not read `public_sender()`.** A recipient's `handle`,
  its `interchain_security_module` getter and any ISM it uses run inside the
  relayer's simulation, where `public_sender()` panics. If `handle` or an
  ISM calls it, every simulation fails and the relayer never submits the
  message. If the getter calls it, the simulation and the delivery can
  verify with different ISMs, and the delivery fails after the relayer has
  paid for it.
- **Contracts can't bridge.** `transfer_remote` takes its DUSK and fees as the
  transaction's deposit, which only the transaction's target contract can
  claim. A smart wallet or router contract calling a warp route always
  reverts.
- The Mailbox keeps the ID and block height of every dispatched message and
  every processed message ID, about 100 bytes per message, plus 72 bytes per
  message in the Merkle hook. That is tens of millions of messages before the
  3 GiB dispatch limit, but the limit is permanent: monitor `memory_bytes`.
- `dispatch` is permissionless, as in Hyperlane. Security comes from the
  destination ISM.
- Dispatch credit can only be withdrawn to Moonlight accounts, not to
  contracts.
- Not tested in the VM: Mailbox nonce exhaustion (needs about 4 billion
  dispatches), and the WarpDrc20 and IGP checked-arithmetic panics, which are
  only reachable with an already-broken supply invariant or an absurd gas
  configuration. These are reviewed statically.
