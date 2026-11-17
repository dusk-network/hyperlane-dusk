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
  most 4 GiB, and it never shrinks. No single allocation can reach 2 GiB, so
  a `Vec` that doubles as it grows fails with most of the memory still free.
  Anything a caller can make a contract store must be bounded or priced, and
  kept in fixed-size chunks. See [storage.md](storage.md).
- **Host queries.** All on-chain hashing uses the `keccak256` host query.
  `ism-multisig` and `validator-announce` use `secp256k1_recover`. Both must be
  active on the target network.

## Deviations from Solidity Hyperlane

| Area | Dusk behaviour |
|---|---|
| Caller identity | No `msg.sender`. A shared resolver maps a root Moonlight call to `keccak256(public_sender)` and a nested call to the caller's `ContractId`, both as `H256`. All owner checks use this. |
| Who can deliver and announce | Only a transaction's own call or a query can call the Mailbox's `process` and ValidatorAnnounce's `announce`; Solidity's take any caller. No contract can deliver a message, to itself or for others, or announce, so each delivery and announcement costs a transaction and a block holds at most 1,000. A recipient can't call back into `process` (HIGH-6). The relayer's simulation of `process` is a query, and still runs. |
| Native value | No `msg.value`. WarpNative, Mailbox fee funding and IGP top-ups check the exact Moonlight deposit through the transfer contract. Hook payments use authenticated contract-to-contract transfers. |
| Fees | Mailbox spends prepaid, sender-keyed credit (`fund_dispatch`, at least 0.01 DUSK for a payer with no credit, and no new payer past 3 GiB). Warp routes fund each dispatch from the sender's deposit and assert their own credit is unchanged. The required hook is a static AggregationHook (MerkleTreeHook + ProtocolFee). IGP is the default hook. ProtocolFee and IGP only record a payment after an authenticated transfer of the exact quoted amount, and hold real DUSK until claimed. Anyone can top up a message's IGP payment (`pay_for_gas`, as `payForGas` on EVM) from a Moonlight transaction's deposit; the IGP keeps its quote, sends the rest of the deposit back to the signer, and records the payment as it records a dispatch's. |
| Hook metadata | The Dusk IGP reads only a gas limit from a dispatch's hook metadata: its first 8 bytes, as a little-endian `u64`. Shorter metadata, empty or 1 to 7 bytes, means the default 50,000 gas, and bytes after the first 8 are ignored. It isn't EVM's `StandardHookMetadata`, which starts with a 2-byte variant and a 32-byte `msgValue` before its `gasLimit`: variant-1 EVM metadata reads as 256 gas, whatever its `gasLimit`, and the dispatch pays for that much. Dusk senders pass the 8-byte form; the warp routes pass none. |
| Account recipients | An account is `keccak256(bls_public_key)`. A route registers it only when its escrow is claimed, so the first transfer to an account waits in escrow and later ones are paid directly. Escrow is only ever paid to the key with that hash. |
| Reverts | `assert!` and `expect(...)` panics instead of custom errors. |
| Upgrades | None. Contract IDs are deterministic; an upgrade is a new deployment. Redeploying over existing state is refused. |
| Multisig metadata | Stricter than Solidity: validators must be sorted, the threshold valid, and the signature section an exact multiple of 65 bytes. |
| Default ISM | One MessageIdMultisig ISM verifies messages from every origin, since Routing ISMs are unsupported. Any origin whose checkpoints the configured validators sign is trusted by the Mailbox; warp routes still accept only their enrolled router per origin. |
| Recipient ISM lookup | Runs with 1M gas. A recipient without `interchain_security_module`, or whose getter panics, gets the default ISM, as in Solidity. A getter that runs out of gas fails the delivery instead, so the submitter's gas limit can't pick the ISM. |
| Dispatched messages | The Mailbox stores each message's ID, in 64 KiB chunks, and each block's height once, not the message. Agents read the message from the finalized `Dispatch` event and check it against the stored ID. `dispatch` refuses a message if what it would allocate could take the Mailbox's memory past 3 GiB of its 4 GiB; `process` and withdrawals keep working. |
| Delivered messages | The Mailbox records a delivery by the first 16 bytes of the message ID, and keeps no list of deliveries and no delivery heights. Agents index deliveries from the `ProcessId` events. Another message is refused as delivered only if its ID shares those 16 bytes. Blocking a message whose bytes an attacker doesn't choose takes a second preimage of 128 bits of keccak256; one it chooses at least 64 bits of, such as a payment to a recipient it names, about 2^64 to 2^65 hashes. Either way the message is left undelivered, never delivered twice ([storage.md](storage.md#the-16-byte-delivery-key)). |
| Merkle roots | The MerkleTreeHook stores every inserted message ID and block height, but no root per insertion. Validators compute the root at an index from the stored IDs. |
| Validator locations | ValidatorAnnounce stores the keccak256 of each announced location, not the location. The location is in the `ValidatorAnnouncement` event; agents check it against the stored hash. |
| DRC20 storage | WarpDrc20 stores balances and allowances under a 32-byte hash of each principal (`drc20::account_id`). The ABI and events still use principals. An owner holds at most 64 allowances, and transfers to new holders and approvals of new spenders stop where they could take the token's memory past 3 GiB, leaving the last GiB to the bridge ([storage.md](storage.md#the-warpdrc20-reserve)). |
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
| HIGH-6 | `process` asked the recipient for its ISM before marking the message delivered, so a recipient could re-enter `process` and have the same message handled twice. | Delivery is reserved before any external call (PR #11). Since Mailbox 6 a contract can't call `process` at all, so a nested call is refused before it gets that far. |
| MEDIUM-1 | IGP quote truncated `u128` cost with `as u64`. | `u64::try_from`, reverting on overflow. |
| MEDIUM-2 | ProtocolFee, IGP, Mailbox fee total and Mailbox nonce could overflow. | Checked arithmetic; dispatch fails on nonce exhaustion. |
| MEDIUM-3 | Multisig ISM ignored trailing partial-signature bytes and did not reject an uninitialised validator set. | Exact-length check and explicit initialisation check. |
| MEDIUM-4 | IGP gas-payment records left out the overhead that was quoted and charged. | Records the full gas plus overhead (PR #11). |
| LOW-1 | WarpDrc20 accepted zero-amount transfers. | Rejected. |
| MEDIUM-5 | TestRecipient, which `deploy-hyperlane` always deploys, let anyone set its ISM, for example to the always-true TestMock. Unsigned messages to it were then delivered by the real Mailbox. | The setter is removed; TestRecipient always uses the Mailbox's default ISM. |
| LOW-3 | WarpDrc20 let holders transfer to Phoenix principals, which no call can ever spend from. | Phoenix recipients are rejected. |
| LOW-2 | The multisig ISM and ValidatorAnnounce accepted signatures OpenZeppelin rejects (`v` of 0/1, high `s`), because the host query does. No double counting was possible. | Both require canonical signatures (`types::signature::is_canonical`). |
| HIGH-7 | The Mailbox stored every dispatched message in its wasm32 memory, which is capped at 4 GiB and never shrinks. About 57k maximum-size dispatches (a few hundred DUSK) would fill it, after which `process` could not allocate either and the bridge stopped both ways. | Only the message ID is stored, in 64 KiB chunks, and `dispatch` refuses a message whose allocations could take the Mailbox past 3 GiB, counting them before it makes them (`memory_bytes` reports usage). The first fix stored IDs in growing `Vec`s, which the storage study below found still failed early. |
| MEDIUM-6 | The Mailbox looked up a recipient's ISM with an unbounded call. A recipient without the getter was charged about 93% of the remaining gas, roughly 26M of a 30M transaction. | 1M gas allowance; running out of it fails the delivery rather than falling back. |
| MEDIUM-7 | A recipient whose only DUSK was in escrow couldn't pay gas to claim it, so every first-time Dusk user needed DUSK from elsewhere. | `claim_pending_for`, payable by anyone. |
| MEDIUM-8 | Dusk charges a failed transaction its whole gas limit. The relayer submitted `process` without a pre-check, so a message whose delivery always fails (for example to the zero recipient) cost 30M gas on every retry. | The relayer simulates `process` as a read-only query first (hyperlane-monorepo). |
| LOW-4 | Contract WASMs embedded the builder's absolute paths and weren't reproducible, so nobody could rebuild a release and check a deployed contract ID. | Paths remapped; one codegen unit with LTO (also 30–45% smaller). The build still depended on where `rusk-private` was: Cargo hashes the absolute path of a path dependency outside the workspace into its symbol names, which reordered functions and data in 11 of the 17 WASMs. Cargo now reaches it through the in-repository `rusk` link, and the WASMs are the same wherever the checkouts and the toolchain are ([testnet.md](testnet.md#reproduce-the-contracts)). |
| LOW-5 | `dusk-tx transfer-remote` accepted any 32-byte recipient. A right-padded EVM address passed Dusk's checks, the EVM token rejected it, and the DUSK stayed locked. | `dusk-tx` takes a 20-byte address and rejects non-zero upper bytes unless `--raw-recipient`. |
| CRITICAL-1 | `claim_pending_for` and `register_account_for` used the caller's `PublicKey` as given. A twin of the recipient's key (a changed y limb, or x.c0 stored as x.c0 + p, which even passes `is_valid`) has the recipient's hash but other raw bytes, so anyone could pay an escrow, or every later transfer to a registered account, into an account nobody can spend from. On all three routes. | Routes decode the key from its compressed encoding and use only the decoded key. |
| MEDIUM-9 | `register_account_for` registered any key for about 39k gas, enough to fill a route's 4 GiB of memory for about 500 DUSK, after which new escrow entries couldn't be allocated. | Removed. Only `register_account` (the signer) and a claim of existing escrow registered keys, and since MEDIUM-10's fix only a claim does. |
| LOW-6 | The relayer sent `process` with a 30M gas limit. A failed transaction pays its whole limit, so anyone who delivered a message first cost the relayer 30M gas. | The relayer's limit is 5M (`DUSK_RELAYER_GAS_LIMIT`), checked against the ISM threshold. |

The fixes have regression tests in `tests/tests/integration.rs`, except the
overflow paths listed under known limitations. HIGH-5 and HIGH-6 use the
`reentrant-hook` contract to attempt the real nested call; HIGH-6's is now
refused as a contract's call to `process`
(`test_contracts_cannot_call_process` tries it from a recipient's
`handle` with a message the Mailbox would otherwise deliver). The replay
tests assert why each nested call fails, and run as well against a
test-only Mailbox without that check (`make mailbox-ungated`), where the
delivery reservation refuses the replay on its own
(`test_the_delivery_reservation_alone_refuses_a_replay_during_ism_resolution`).
The recipient also asks the Mailbox, during its ISM lookup, whether the
message is already recorded delivered, so moving the reservation after any
external call fails them whatever the caller check does.

## Second review

A second internal review of the fixes for CRITICAL-1, MEDIUM-9 and LOW-6,
before the public testnet, found no new critical or high issue. It found
these, some of them in the fixes' own claims:

| ID | Issue | Status |
|---|---|---|
| MEDIUM-10 | Routes still register any number of keys. Without `register_account_for` (MEDIUM-9) every entry costs a transaction signed by its key, but anyone can fund new keys and call `register_account` from each, and no entry is ever removed. A test said that nobody could fill a route's memory with registrations. | Fixed. Routes have no `register_account` (WarpNative 5, WarpDrc20 8, WarpDrc20Collateral 7): only a claim of escrow registers an account, so every entry stands for a transfer someone sent from the remote chain ([below](#registry-growth)). |
| MEDIUM-11 | Routes deployed before the CRITICAL-1 fix reported the same `state_version` as fixed ones, so `deploy/testnet.sh deploy` and `demo/deploy.sh --skip-deploy` would reuse a recorded deployment whose sponsored claims still pay forged keys. | Fixed. Routes are at `state_version` 4 (WarpNative), 6 (WarpDrc20, 7 since the storage redesign) and 5 (WarpDrc20Collateral, 6 since MEDIUM-13), and both scripts refuse a recorded route below that and say to redeploy. `agent-configs` and `preflight` check it too. |
| LOW-7 | The Mailbox's `withdraw_dispatch_credit`, and each route's owner-only wrapper of it, pay the recipient key as given. A payer who passes a twin of the recipient's key (see CRITICAL-1) is debited and the event names the recipient, but the DUSK goes to an account nobody can spend from. Only the caller's own credit, or a route's credit by its owner, is at risk. | Fixed. The Mailbox decodes the key from its compressed encoding, reverts if that fails or gives the identity, and pays and names the decoded key; the routes' wrappers go through it. Decoding costs about 57M gas, so a withdrawal takes 58.7M, and `dusk-tx withdraw-dispatch` defaults to an 80M limit. |
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

### Registry growth

Each registration adds about 352 bytes to the route's memory (a
key takes 200 bytes in memory), and nothing removes it. The memory is
wasm32: at most 4 GiB, and it never shrinks. A route whose memory is full
of registrations can't allocate a new escrow entry or registration:
transfers to recipients that aren't registered can't be delivered, those
recipients can't claim, and WarpDrc20 can't mint to an account with no
balance yet.

Until the fix, `register_account` registered the transaction's signer, so
an entry cost a funded key and two transactions, about 23,700 to 47,400
DUSK for a route's 4 GiB. Now a route registers an account only when its
escrow is claimed: `claim_pending` by the account or `claim_pending_for`
by anyone for it, and both revert with nothing in escrow. Escrow only
comes from a delivery from the route's enrolled router, so each entry
takes a transfer someone sent from the remote chain and paid our IGP
there for, its delivery on Dusk, which the relayer pays and the IGP fee
repays, and a claim. `test_a_registration_costs_an_inbound_transfer_and_two_transactions`
measures the funding transfer and the claim at about 1.85M gas each.
Filling 4 GiB takes 9.8M to 12.2M inbound transfers, as many EVM
transactions, 1.3 to 1.6 ETH of IGP fees at the default pricing, and
39,000 to 71,000 DUSK of Dusk gas for the deliveries and the claims, with a
funding transfer for each key or without. Entries sorted by their key
fill 20% sooner than random ones ([storage.md](storage.md#key-order)).

The Mailbox's process path had the same problem, worse: it stored every
processed message ID in a growing `Vec`, which failed at 33.5M deliveries
for about 1,074 DUSK. The storage redesign removed that list
([storage.md](storage.md)); the Mailbox now keeps 16 bytes per delivery,
about 26 bytes of memory, and since a contract can't call `process`, each
delivery takes a transaction: filling the Mailbox with deliveries takes
239,000 to 301,000 DUSK, and closing its dispatch 179,000 to 225,000.
In DUSK a registry costs less to fill than the Mailbox, but each of its
entries also takes a transfer on the remote chain, 9.8M to 12.2M EVM
transactions in all. The cheapest damage anywhere is to a WarpDrc20's new
holders, and it stops at a 3 GiB reserve that leaves the bridge working
([costs.md](costs.md#attack-costs)). WarpDrc20 reports its memory
(`memory_bytes`); WarpNative and WarpDrc20Collateral don't yet.

## Storage study

A study ran each contract's storage under a metered host until it failed,
at 1 LUX per gas, with an attacker contract batching calls in one
transaction. Every growing `Vec` failed at 2^25 entries or fewer, long
before 4 GiB, because no allocation can reach 2 GiB on wasm32:

| Storage | Failed at | Cost to cause | Fix ([storage.md](storage.md)) |
|---|---|---|---|
| Mailbox processed-message list | 33.5M deliveries, to a recipient that names itself as ISM | about 1,074 DUSK | Removed. Deliveries are recorded by 16-byte keys in a `BTreeSet`; agents index `ProcessId` events. |
| Mailbox and MerkleTreeHook dispatch logs | 33.5M dispatches; the 3 GiB dispatch limit never fired, and in mixed traffic a 1.25 GiB reallocation overshot it and `process` failed at 16.8M | about 10,500 DUSK | 64 KiB chunks, heights once per block, no root per insertion, and an exact limit. |
| IGP payment records | 16.8M payments | about 5,600 DUSK plus IGP fees | 64 KiB chunks. |
| WarpDrc20 allowances and balances | 14.3M approvals to new spenders, 14.3M new holders | about 156 and 227 DUSK | Keyed by 32-byte account IDs. |
| ValidatorAnnounce locations | 3.5M announcements of 1 KiB locations by new keys | about 1,257 DUSK | Location hashes; the locations are in the events. |

Everything the contracts used before failed with the memory still mostly
free, and nothing could recover it: the contracts can't be upgraded, so the
Mailbox and its routes would have to be redeployed.

## Third review

A third internal review, of the operator tooling and the collateral route
before the public testnet, found:

| ID | Issue | Status |
|---|---|---|
| MEDIUM-12 | The LOW-10 check failed open. `deploy/testnet.sh` treated a `module_type` query that failed or returned nothing, and every module type but 5, TestMock's 6 (an ISM that accepts every message) included, as an ISM with no signatures to check, and fell back to `EVM_ORIGIN_THRESHOLD`. With the threshold raised to 20 on chain and that one query failing, `preflight` and `agent-configs` passed a 5M relayer gas limit; a 20-signature `process` needs 9.5M with the margin, so every delivery would have failed and paid the whole limit. The deployed validator set wasn't compared at all. | Fixed. Every query must succeed. The ISM the route uses, its own or else the Mailbox's default, must be a MessageIdMultisig ISM whose validators and threshold are exactly `EVM_ORIGIN_VALIDATORS` and `EVM_ORIGIN_THRESHOLD`, or `preflight`, `deploy` and `agent-configs` stop. A TestMock ISM passes only in a local rehearsal (`LOCAL_REHEARSAL_ALLOW_TEST_MOCK=1` with local endpoints). `scripts/test-deploy-scripts.py` runs both commands against a stubbed `dusk-tx`. |
| MEDIUM-13 | WarpDrc20Collateral's `transfer_remote` read its custody before and after the token's `transfer_from`, with no reentrancy guard. A token that takes a fee on transfers and calls the sender back could re-enter `transfer_remote` from that callback; with no dispatch fee, a contract can call it. Nested transfers small enough to pay no fee land between the outer transfer's two reads and make up its fee, so both checks pass: 100 locked tokens backed 110 remote mints. | Fixed. `transfer_remote`, `handle`, the three claims and `withdraw_dispatch_credit` take a guard stored in the route before their first call to another contract, and a nested call into any of them reverts. The balance checks stay. Collateral routes are at `state_version` 6, and both deploy scripts refuse older ones. The VM tests run the attack with a test token, `callback-fee-token`, against the route and against a build of it without the guard (`make warp-drc20-collateral-unguarded`), and re-enter every guarded entry point from the token's callback. |
| LOW-12 | The TypeScript SDK's `RuesClient` accepted an RPC URL with a username and password. `fetch` then failed every request with an error that quoted the whole URL, credentials included. Other transport errors, and the URL parser's error for a URL that doesn't parse, passed through with the URL in them too. | Fixed. The client refuses a URL with a username or password, and its errors name no part of the URL; a network failure keeps only an error code such as `ECONNREFUSED` (`ts-sdk/tests/rues-credentials.test.mjs`). |
| LOW-13 | `demo/gen-agent-configs.sh` decrypted the consensus keys a second time, in Python, with the password pasted into the Python source as `${CONSENSUS_PASSWORD@Q}`, which is Bash quoting. An apostrophe, a trailing backslash or a tab broke the script, and Python's syntax error printed the password; other backslashes were read as Python escapes, so it derived another key. | Fixed. The script writes the key file with `dusk-tx export-signer-key`, as `deploy/local-agents.sh` does, passing the password in the environment. `scripts/test-deploy-scripts.py` runs it with passwords holding quotes, backslashes, `$`, backticks, a tab and non-ASCII, through a stub and through the real `dusk-tx`. |
| LOW-14 | `dusk-tx` reported a write as done once `tx(hash)` showed it with no error, which only proves the transaction was included. Until its block is final, a revert can drop it or run it again with another result, and nothing in the output said which it was. | Fixed. Every write's output also has `block_height`, `gas_spent` and `finalized`, whether its block was final when checked right after inclusion (with `block_hash` once it is); the existing fields are unchanged, so the agents' parsing is too. `--wait-finality` waits until the block is final, and only then reports an execution error; it is opt-in, and the agents don't use it. |
| LOW-15 | The Dusk IGP only took a payment when a message was dispatched. Once the relayer enforced payment (`onChainFeeQuoting`), a Dusk message whose dispatch paid for too little gas, such as one whose recipient's `handle` takes more than a transfer's, or whose delivery got more expensive after it was sent, could never be relayed: nothing could add to its payment, as `payForGas` does on EVM. | Fixed. The IGP, at `state_version` 4, takes top-ups: `pay_for_gas(message_id, destination, gas_amount, deposit)`, priced by `quote_pay_for_gas` without the overhead. It takes the calling transaction's deposit through the transfer contract, which hands a deposit over once, at its exact value and only to the transaction's target, before it records anything, and sends the deposit beyond its price back to the signer. The record and the `gas_payment` event are a dispatch's, so the agents index top-ups unchanged and the relayer adds them up. `dusk-tx pay-for-gas` quotes and checks a top-up before signing it, both deploy scripts refuse an IGP below 4, and the local rehearsal tops up a stuck message until it is delivered. See [Design decisions](#design-decisions). |

## Fourth review

A review of the relaying fees before the public testnet found:

| ID | Issue | Status |
|---|---|---|
| HIGH-8 | `deploy/testnet.sh` deployed the Sepolia IGP and the route's token bare and initialized each in a second transaction, which anyone could send first. An IGP initialized with the deployer as owner and the attacker as beneficiary passed every check, and with `EVM_IGP_BENEFICIARY` empty `deploy` kept that beneficiary: every fee paid to the IGP went to the attacker. Or the attacker could initialize it with a contract of its own as owner, add a quote signer, store a free standing quote for its own messages to Dusk, or for everyone's, remove the signer and hand the IGP to the deployer. Standing quotes outlive their signer and come before the oracle, so the oracle checks still passed, and the relayer delivered messages that had paid nothing for the gas they were credited with. A token initialized first had the attacker's owner, ISM and supply, and `deploy` stopped. | Fixed. The IGP and the token are each an `ERC1967Proxy` of their implementation whose constructor calls `initialize`, so the transaction that creates one also initializes it. The proxies have no admin, and neither implementation can upgrade itself. `deploy` records each creation transaction, and every command that uses the contracts checks its receipt: sent by the EVM deployer, creating the contract as a proxy of the recorded implementation, with the contract's `Initialized` event in it. A contract recorded without one, by an older version of the script, is refused. The IGP must also have no quote signers, and quote a transfer what its oracle's pricing comes to, as the token and as any other sender. `scripts/test-deploy-scripts.py` runs both attacks and a token front-run against a fake chain, and on Anvil shows the initialization in the creation transaction and an IGP the attacker initialized first refused. |
| MEDIUM-14 | `evm-igp --apply` compared the settings with the pricing of whichever oracle the Sepolia IGP read from, not with the StorageGasOracle the state recorded. With the IGP pointed at another oracle that had the settings' pricing, it left the recorded oracle without any, pointed the IGP at it and only then failed its check. The oracle returned zero for Dusk, so the IGP quoted delivery at zero, and the relayer delivered messages that had paid nothing for the gas they were credited with. | Fixed. `evm-igp`, dry run or `--apply`, stops before sending anything when the IGP reads from an oracle other than the recorded one. `deploy` and `--apply` price the recorded oracle and read the pricing back before they point the IGP at it, and then check what the IGP quotes. `scripts/test-deploy-scripts.py` runs the case that switched to the unpriced oracle. |
| MEDIUM-15 | `dusk-igp --apply`, `evm-igp --apply` and `claim` never checked that the Dusk node and the EVM RPC were on `DUSK_EXPECTED_CHAIN_ID` and `EVM_CHAIN_ID`, nor that the contracts in the state were the deployment's, and the new `dusk-tx` fee commands signed for whatever chain the node reported. A Dusk contract's ID depends only on its bytecode, the deployer key and a nonce, so a node or RPC on another network can hold a contract at the same ID or address: an owner transaction could reprice, or a claim pay out, on a network nobody meant. | Fixed. Those commands and `agent-configs` check both chain IDs first, then that the recorded Mailbox is at `DUSK_DOMAIN`, the IGP and ProtocolFee take their calls from it or from one of its hooks that calls them (the ProtocolFee from the aggregation hook that is its required hook), the IGP is its default hook, and the Sepolia Mailbox, domain and contracts are the recorded ones. `revenue`, which only reads, checks both chain IDs and nothing else. `set-igp-config`, `set-beneficiary`, `claim-fees` and `pay-for-gas` take a required `--expected-chain-id` (optional with `--quote-only`), checked as `withdraw-dispatch` checks it, before any contract query or key access, and the scripts pass it. A `dusk-tx` test shows each command stopping after its chain ID request. |
| MEDIUM-16 | `demo/bench/agents.sh` and `deploy/local-agents.sh` started the agents with the operator's whole environment. Hyperlane applies `HYP_*` variables over the config files, so a `HYP_CHAINS_DUSK_CUSTOMRPCURLS` left in the shell sent the bench's agents past its proxy to whatever node it named, with the development keys, and other variables (`CONFIG_FILES`, `RUST_LOG`, `AWS_*`) changed what they did; the rehearsal's agents also inherited the env file's `EVM_DEPLOYER_KEY` and Dusk key password. | Fixed. Both start each agent with `env -i` and only `PATH`, `HOME`, `DUSK_TX_BIN`, its config, its keys and, in the bench, the shim's variables. `scripts/test-deploy-scripts.py` starts them with such a shell and checks each agent's environment. |
| LOW-16 | Choosing the Sepolia route's hook, `deploy/testnet.sh` counted the Mailbox's required hook as inserting into the Merkle tree only when it was that tree's hook or an aggregation with it among its own hooks. One that reached the tree through a nested aggregation or a routing hook counted as not inserting, so the route's hook added the tree again and every message went into it twice; and any hook it didn't know passed. | Fixed. `deploy` follows the required hook through aggregations and domain routing hooks (the hook for Dusk, or the fallback) and counts the insertions into `EVM_MERKLE_TREE_HOOK`. A Merkle hook, an IGP, a ProtocolFee and a pausable hook are leaves. Twice or more, any other kind of hook, a routing hook with nothing for Dusk, or nesting deeper than 8 stops `preflight` and `deploy`. |

## Fifth review

A last review of the operator tooling before the public testnet found
these. Each fix has a test that fails without it.

| ID | Issue | Status |
|---|---|---|
| MEDIUM-17 | `deploy/testnet.sh` checked the Sepolia IGP's pricing by its quote signers and by what it quoted a transfer asked as the route's token and as the zero address. The IGP resolves standing quotes by sender, and a standing quote outlives the signer that stored it. An adopted IGP whose former signer, since removed, left an unexpired zero-price quote for any other sender passed, and that sender's messages to Dusk paid nothing for the gas the relayer credited them with. | Fixed. This deployment uses no signed quotes, so `preflight`, `deploy`, `evm-igp` and `agent-configs` also refuse an IGP that has ever had a quote signer: they read its `QuoteSignerAdded` events from the block that created it, `EVM_LOG_BLOCK_RANGE` blocks a request, and record in the state how far they read. A new IGP has none; [operations.md](operations.md#an-igp-that-had-a-quote-signer) says how to replace one that had. The tests store the free quote for a sender that is neither the token nor the zero address, and on Anvil a real signer signs one, which the IGP honours after the signer is removed. |
| MEDIUM-18 | The commands that use the recorded Sepolia token only proved that the deployer created and initialized it as a proxy of the recorded implementation. Mixed or restored state could name this deployment's Mailbox with a genuine token of another deployment by the same deployer: the checks passed, the fee commands adopted it, and `deploy` set its hook and enrolled this route on it. | Fixed. Every command that uses the token first checks that its `mailbox()` is `EVM_MAILBOX`, that it verifies messages from Dusk with the ISM recorded in the state, and that this ISM is a MessageIdMultisig ISM of exactly `DUSK_ORIGIN_VALIDATORS` and `DUSK_ORIGIN_THRESHOLD`. |
| MEDIUM-19 | Only `withdraw-dispatch` and the fee commands (MEDIUM-15) required `--expected-chain-id`. `fund-dispatch`, `transfer-remote`, `enroll-router`, `deploy-hyperlane`, `register-account`, `claim-pending`, `dispatch`, `process` and `drc20-approve` signed for whatever chain the endpoint reported, several of them after reading the key, and `call` and `deploy-wasm` took the flag only optionally. | Fixed. Every `dusk-tx` command that sends a transaction requires `--expected-chain-id`, and checks it against the node before it queries a contract, reads the key or builds a transaction; `pay-for-gas --quote-only` signs nothing and needs none. The agents already passed it to `call`. Every script and documented example passes the chain it means, and a `dusk-tx` test drives each command against a node on another chain and sees it stop after one request. |
| MEDIUM-20 | `dusk-tx deploy-wasm`, which the local scripts use to put Multicall for Dusk at its published ID, reported success for a contract already at the ID its Wasm, deploy nonce and owner give, without looking at it. An existing contract at the expected ID is not trusted without verification. | Fixed. `deploy-wasm` takes an existing contract only when Rusk reports exactly the given owner for it and, with `--check-multicall`, it answers `version` with 1 and an `aggregate` of the transfer contract's `chain_id` with the node's chain ID. Otherwise, or when it can't tell, it fails and sends nothing. It checks a contract it deploys the same way, and the scripts pass `--check-multicall`. |
| LOW-17 | `check_dusk_hook_binding` accepted a fee contract that took its calls from the recorded Mailbox without checking that the Mailbox calls it: a detached ProtocolFee bound to the Mailbox passed, and `claim` signed a claim for it. | Fixed. A fee contract bound to the Mailbox must be its required or default hook, as one bound to one of those hooks must be called by it. |
| LOW-18 | The checks on the Sepolia IGP and token read EIP-1967's implementation slot but not its admin slot. The `ERC1967Proxy` `deploy` creates has no admin; a proxy with one, such as a `TransparentUpgradeableProxy` the same deployer created and initialized the same way, passed while its admin could change its code. | Fixed. Every command that adopts them requires the admin slot to be zero, as the Anvil test already asserted of a new deployment. |

The same round left the agents' experimental pipelined deliveries to Dusk
out of every generated config (see [Design decisions](#design-decisions)),
and stopped `dusk-tx transfer-remote --native` from asking a native route
for views only DRC20 routes have.

## Sixth review

A review of the fifth round's fixes found these. Each fix has a test that
fails without it.

| ID | Issue | Status |
|---|---|---|
| MEDIUM-21 | The check of an adopted Sepolia token's ISM (MEDIUM-18) folded the validators it read on chain into a set before comparing them with `DUSK_ORIGIN_VALIDATORS`. Hyperlane's MessageIdMultisig ISMs accept a validator listed more than once and take its signature for each place it is listed, so an ISM of [A, A, B] at threshold 2 passed as 2 of [A, B] while A alone could sign for any message, and so mint tokens nothing backs. The test changed the validators and the threshold together, so dropping either comparison went unnoticed, and its case for order had one validator. | Fixed. A validator set read on chain, the token's ISM's or the Dusk route's, that lists an address more than once, in any case, is refused before it is compared. The tests change the validators and the threshold of a 2-of-3 ISM one at a time, and change order and case on three validators. |
| MEDIUM-22 | Deployments by the same deployer through Sepolia's Mailbox with the same validators share the ISM Hyperlane's factory deploys for them, so another deployment's token, put in this one's state, passed every check on the token. `deploy` then enrolled this deployment's route on it in place of that deployment's, and it on this route: the other deployment's tokens, backed by its route's DUSK, could be redeemed for this route's. | Fixed. Before any command uses the token, and so before `deploy` writes to it, its router for Dusk must be the route recorded in the state, or none; with none, it must hold no tokens, as `deploy` creates it, so tokens minted before any route was enrolled can't be redeemed for this route's DUSK either. `deploy` checks again right before each write to the token, and enrolls this route only where it read none: a route the same owner enrolls after preflight is refused, unless it lands between that check and the write (LOW-20). A new or resumed deployment passes. |
| MEDIUM-23 | `demo/deploy.sh` and `demo/demo.sh` set the chain their transactions were signed for (`--expected-chain-id`) from the node's own answer, over an operator's `DUSK_EXPECTED_CHAIN_ID`. Pointed at a testnet node, or a local forwarder to one, they signed the deployment, funding and enrollment for testnet. `deploy/local-evm-core.sh` and the bench's `setup` did the same, and wrote the node's chain into the settings `deploy/testnet.sh` then signs with; `demo/start-env.sh` took any healthy node it found on its port. | Fixed. They sign for `DUSK_EXPECTED_CHAIN_ID`, the local devnet's 0 unless set (`demo/bridge.env.example`), and stop before they sign or write anything when the node reports another chain; `start-env.sh` checks a node it finds running, or starts. |
| LOW-19 | `deploy` set `DUSK_FEE_BENEFICIARY` on the recorded Dusk IGP and ProtocolFee without checking that the recorded Mailbox calls them (LOW-17): its preflight doesn't. Restored state naming a detached ProtocolFee the same key owns had its beneficiary changed, and with it who can claim the fees it holds. | Fixed. `deploy` checks the recorded Dusk deployment, the fee contracts' binding included, once its contracts are deployed or found recorded, before it sets a beneficiary. |

The same round added `dusk-tx call --ack-before-send`, so the agents can
keep a transaction's hash before it is sent: the helper signs the call,
names it, and sends it only on the line `send` within 30 seconds, and
otherwise never sends it and says so. Before `send` only unsigned reads
reach the node, its chain ID and, without `--nonce`, the signer's account
([operations.md](operations.md#acknowledged-sends)).

## State growth, stage 1

Stage 1 of the state-growth plan, before the public testnet, bounds who can
add to the contracts' memory cheaply, since Dusk's gas doesn't price the
bytes a call leaves behind ([storage.md](storage.md#who-can-add-entries)):

| Change | Contracts | Effect |
|---|---|---|
| `process` and `announce` take only a transaction's own call or a query | Mailbox 6, ValidatorAnnounce 3 | No contract can make many deliveries or announcements in one transaction. Filling the Mailbox with deliveries went from about 5,700 DUSK in 4 hours to 239,000 to 301,000 DUSK over 15 to 19 days of full blocks, and ValidatorAnnounce from about 10,100 DUSK to 74,000 to 82,000. A recipient can't call back into `process` (HIGH-6). |
| Routes register an account only when its escrow is claimed | WarpNative 5, WarpDrc20 8, WarpDrc20Collateral 7 | MEDIUM-10 is fixed: a route's registry fills only with accounts someone sent a transfer to, 9.8M to 12.2M of them. |
| At most 64 allowances per owner; transfers to new holders and approvals of new spenders stop at 3 GiB | WarpDrc20 8 | Approvals to the reserve cost 79,000 to 99,000 DUSK instead of 3,000 to 3,800 for the whole token. Holders still reach it for 2,300 to 2,900 DUSK and token units, but then only transfers and approvals to new addresses stop; the synthetic route keeps the last GiB. A collateral route over a WarpDrc20 would not: it pays out with the token's `transfer`, so it would stop paying accounts with no balance of the token. Wrapping a WarpDrc20 is unsupported, and `dusk-tx deploy-hyperlane` refuses it. |
| `fund_dispatch` takes at least 0.01 DUSK for a payer with no credit, and credit for new payers stops at 3 GiB | Mailbox 6 | The minimum comes back through one dispatch to a hook of the payer's own, so a new entry costs the protocol fee and about two transactions: credit for new payers, with those dispatches, fills the Mailbox's 3 GiB for about 140,000 to 225,000 DUSK instead of about 92,200, and can't reach the last GiB, which only deliveries fill. Before, credit at 1 LUX could fill all 4 GiB, and with it stop `process`, for about 98,000 to 123,000 DUSK. |

WarpDrc20 8 also refuses a transfer to the zero account, as ERC20 does:
nobody can spend from it, and its transfer event would read as a burn
while the supply stays the same.

Each rule has a VM test that fails without it, in `tests/tests/integration.rs`:
`test_contracts_cannot_call_process`, `test_contracts_cannot_announce`,
`test_a_funded_key_cannot_register_itself`,
`test_warp_drc20_owner_holds_at_most_64_allowances`,
`test_warp_drc20_new_holders_and_allowances_stop_at_3_gib`,
`test_a_new_dispatch_payer_funds_at_least_a_hundredth_of_a_dusk`,
`test_credit_for_new_payers_stops_at_3_gib` and
`test_warp_drc20_refuses_transfers_to_the_zero_account`.
`test_warp_drc20_zeroed_allowances_free_their_entries` pins the removal of
zeroed allowances, which the cap relies on but which predates it. Two tests
pin what stage 1 leaves as it is, and fail if it changes:
`known_limitation_collateral_route_over_warp_drc20_stops_paying_new_holders_at_the_reserve`
and `known_property_a_new_payers_minimum_comes_back_through_its_own_hook`.
The VM tests need the private rusk-private checkout, which CI can't fetch,
so they are a release gate run by hand (`make repro-check`; the VM tests
take about 10 minutes), with `make check-genesis-contracts`, which checks
that the genesis transfer and stake contracts they run are the ones
rusk-private ships at `RUSK_COMMIT`.
`deploy/testnet.sh`, `demo/deploy.sh` and the agents refuse the earlier
versions. The ranges run from keys in increasing order, which an attacker
can arrange, to keys in random order ([storage.md](storage.md#key-order)).

## Foundations review

A review of the parts stage 1 doesn't change, before the public testnet,
found these in this repository. Each fix has a test that fails without it.

| ID | Issue | Status |
|---|---|---|
| MEDIUM-24 | Nothing read back what `dusk-tx deploy-hyperlane` deployed: it took each deploy that executed as done, and `deploy/testnet.sh` recorded and used the contracts it reported. A contract someone else deployed or initialized first, front-running ours, has their owner and bindings, and nothing would have noticed: the script checked the domains, the state versions, the fee contracts' bindings, the route's ISM and the IGP's pricing, but no owner, nor the Mailbox's hooks, nor the Merkle hook's, ValidatorAnnounce's or the route's Mailbox. The Mailbox's or the ISM's owner controls every route's custody. | Fixed. Once every contract is deployed, `deploy-hyperlane` reads each one back, the owner Rusk records for it, its owner, each binding its `init` set and its `state_version`, and fails, reporting none of them, when one isn't what it sent (`dusk-tx/src/deployment.rs`). `deploy/testnet.sh` reads them again against the settings before it records them, records the owner it read, and reads them back before any command uses them; a deployment recorded without that owner is refused ([testnet.md](testnet.md#reading-back-the-dusk-contracts)). Each command reads `deploy-hyperlane`'s report and the record once, and holds a lock on the deployment's directory while it runs, so the contracts it reads back are the ones it records and the agent configs name. Routes have no view that lists their routers, so only the routers for the domains the deployment knows are read. A `dusk-tx` test changes each value read back in turn, and runs `deploy-hyperlane` against a node whose replies differ; `scripts/test-deploy-scripts.py` does the same to a new and a recorded deployment, replaces the report and the record while a command runs, and runs each command while another holds the lock. |
| LOW-20 | `deploy` enrolled the EVM token on the Dusk route whenever the route's router for the EVM chain wasn't the recorded token, replacing any other router, where the EVM side refuses to. Run again after the token record changed, it cut the old token's holders off from the DUSK that backs their tokens. | Fixed. `deploy` enrolls the token only where the route has no router for the EVM chain, reading it right before each write, and refuses to replace another; enrolling the same one again is a no-op. Every command refuses a route enrolled with another router than the recorded token. Left open: `deploy/testnet.sh` runs one command at a time on a deployment, but a router the owner enrolls another way between the last read and the enrollment is replaced by it, since `enroll_remote_router` replaces whatever is there. An on-chain check of the router the owner expects to replace waits for the mainnet contracts ([testnet.md](testnet.md#before-mainnet)). |

The same review corrected the docs: deliveries to Dusk are capped at the
relayer's gas limit whatever a message paid for
([costs.md](costs.md#what-stays-uncovered)); what the 16-byte delivery key
lets an attacker block ([storage.md](storage.md#the-16-byte-delivery-key));
owners can reprice dispatches paid from credit
([Owner powers](#owner-powers)); Dusk's hook metadata isn't EVM's
`StandardHookMetadata` ([Deviations](#deviations-from-solidity-hyperlane));
amounts that scaling rounds away, removing a router, and the synthetic
decimals rule ([Known limitations](#known-limitations)); and the password
order, what `revenue` checks and the EVM owner's fee powers. The one test
of the agent against a real `dusk-tx`, which ran nowhere, is part of the
release gate now ([operations.md](operations.md#release-gate)).

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

On the EVM side, `deploy/testnet.sh` makes the EVM deployer the owner of the
token, the IGP and its gas oracle. The token's ISM has no owner: its
validators and threshold are fixed when it is created. Whoever holds the
deployer key can mint or burn through the token's ISM and routers, point the token's hook at
anything (a hook without the MerkleTreeHook leaves its messages out of the
tree validators sign, and they never deliver), price delivery to Dusk at
anything (an absurd price or a removed oracle makes every transfer to Dusk
revert), and take the IGP's payments by changing its beneficiary. It can
also charge every transfer a fee in the token, to a fee contract it names
(the token's `setFeeRecipient`), make transfers pay for their delivery in
the token instead of ETH (`setFeeHook`), and price delivery to Dusk in any
ERC20 at a gas oracle it chooses (the IGP's `setTokenGasOracles`): once one
is set for Dusk, anyone can pay for Dusk gas in that token at that
oracle's price, and the checks on the IGP's ETH pricing don't see it.

The token and the IGP are `ERC1967Proxy`s without an admin, so nobody can
change their code. Their implementations, deployed first, are never
initialized or used; anyone may initialize one, which changes nothing about
the proxies.

They can't withdraw users' dispatch credit, or touch pending escrow or
existing WarpDrc20 balances. They do set what a dispatch paid from credit
costs, and `dispatch` takes no cap on it: the Mailbox's owner chooses its
hooks, the IGP's owner its prices, and the ProtocolFee's owner its fee, up
to its maximum of 0.1 DUSK. A price raised between signing a dispatch and
its inclusion is paid from the sender's credit, up to all of it. Only
withdrawals are beyond the owners' reach. A warp transfer pays its exact
quote from its deposit, so a repriced one fails instead of paying more. On
testnet one deployer key on each side is acceptable. Mainnet needs the
owner role on a multisig or equivalent custody.

## Design decisions

Accepted:

- **Dispatch credit belongs to the payer.** Anyone can fund credit, but only
  the effective payer can withdraw it, and only to a Moonlight account. Warp
  routes expose an owner-only withdrawal of their own credit. There is no
  global drain by the Mailbox owner, but the owners set the prices a
  dispatch pays from credit, with no cap ([Owner powers](#owner-powers)).
- **No admin recovery of escrow.** Pending transfers are only ever paid to
  the matching BLS key, though anyone can pay the gas to claim them for it.
  If a user loses that key, the funds stay locked. An admin drain would add
  a path to seize user funds.
- **Immutable registration** (issue #5). No call removes a registration or
  points it at another key. A later claim registers again, but with the same
  key: every key a route stores is canonical, either the claiming
  transaction's sender, which the node decodes from its compressed
  encoding, or a key `claim_pending_for` has decoded, and only one
  canonical key has a given hash. A new key produces a new recipient hash anyway, and remapping
  would give an admin control over where user funds go.
- **Top-ups aren't checked against the Mailbox**, as on EVM. `pay_for_gas`
  records a payment for any message ID and destination it is given. The
  Mailbox stores no message by ID, nor any destination, so checking would
  need the payer to send the nonce or the whole message, and would only
  protect payers from their own mistakes: a payment can only add to what a
  message has paid, the relayer looks payments up by the ID and destination
  of a message it has indexed, and a payment for anything else only costs
  its payer. `dusk-tx pay-for-gas --nonce` reads the message's ID and
  destination from the Mailbox and its `Dispatch` event for the payer.
- **Top-ups come from the transaction's own deposit.** `pay_for_gas` must be
  the Moonlight transaction's own call: the transfer contract only lets the
  transaction's target take its deposit, once and at its exact value, so a
  contract can't top up with the deposit of a transaction that calls it,
  and one deposit can't pay twice. The IGP takes the whole deposit before
  it records anything, and sends what it doesn't charge back to the account
  that signed, which is the account the deposit came from, in the same
  transaction. That needs no refund address: a key given as an argument
  would have to be decoded first (CRITICAL-1, LOW-7), for about 57M gas.
  The deposit is the most the payer pays, so a margin over the quote
  survives a price update before the transaction is included, where an
  exact payment would fail and pay its whole gas limit. Shielded
  transactions are refused, as everywhere else.
- **Top-ups close at 3 GiB.** A top-up takes a transaction, about 1.83M
  gas, for its 64-byte record, but pays no fee beyond the gas it buys, so
  at the testnet pricing it is a cheaper way to fill the IGP than a
  dispatch, which also pays 6 DUSK of IGP fees and the protocol fee.
  `pay_for_gas` refuses a top-up that could take the IGP's memory past
  3 GiB, as `dispatch` does the Mailbox's, so the last GiB stays for the
  payments dispatches make: filling the IGP to the point where dispatches
  through it revert still takes dispatches
  ([costs.md](costs.md#attack-costs)).
- **Deliveries to Dusk go one at a time.** A relayer key delivers about
  one message every two blocks. The agents' pipelined deliveries, several
  of the relayer's transactions sharing a block, are experimental: only a
  chain with both `pipelineDeliveries: true` and a `maxBatchSize` above 1
  pipelines, `agent-configs` writes neither, and they stay off on testnet
  and mainnet until the six issues their last review found are fixed
  ([operations.md](operations.md#deliveries-to-dusk)). The default path
  has none of them.
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
  scale parameters). That is a rule for whoever deploys one, not a check:
  `deploy-hyperlane --warp-decimals` takes any value, and the route keeps
  and reports the decimals it was given, whatever the EVM side's scale. The
  native DUSK route is not affected: both sides use 9 decimals and supply is
  bounded by DUSK custody. The demo's 18-decimal synthetic with a 10-token
  supply is a demo only. Tests: `redteam_synthetic_*` (ignored).
- **Scaling rounds small amounts away.** An EVM route that carries its
  token with fewer decimals on Dusk, such as an 18-decimal token bridged as
  9 (HypERC20's scale of 1/10^9), rounds each amount down as it sends it.
  Only whole units on Dusk arrive: the smallest amount that does is
  10^(EVM decimals - Dusk decimals) of the EVM token's units, in general
  `scaleDenominator / scaleNumerator` rounded up, and the rest of any
  amount is kept by the EVM side, burned or locked. An amount below that
  smallest one becomes 0, every Dusk route refuses a transfer of 0, and the
  message is never delivered: the sender loses all of it. The testnet's
  native route doesn't scale, so every amount but 0 arrives.
- **Routers can't be removed.** Routes have no `unenroll_remote_router`,
  and refuse a zero router. To stop a route taking messages from a domain,
  for example after the remote side is compromised, its owner enrolls in
  place of the old router one that nothing on that chain can send from: for
  an EVM chain, any value whose first 12 bytes aren't zero, such as 32
  bytes of `0xff`, matches no address. The route then refuses every message
  from that domain. Transfers to that domain still go out, to that router,
  and are never delivered, so what they take stays locked in the route, or
  burned on a synthetic one. Tell users before doing it.
- **Contract IDs are predictable during deployment.** A contract ID depends on
  bytecode, owner key and nonce, and Rusk doesn't check that the owner signed
  the deploy. Some contracts are initialised with the IDs of contracts
  deployed after them. Anyone who sees the deployer's first transaction can
  deploy at the later IDs first, or front-run a contract's `init`, and
  choose its owner and bindings. `dusk-tx deploy-hyperlane` reads every
  contract back once it has deployed them, and `deploy/testnet.sh` again
  before it records them and before any later command uses them: an owner,
  binding or state version that isn't what was sent stops them
  (MEDIUM-24). A front-run then denies the deployment, and a new key can be
  front-run the same way. Routes have no view that lists their routers, so
  a router a front-run `init` enrolled for another domain than those read
  would go unseen; the route would take that domain's messages that the
  Mailbox's ISM verifies. The fix for mainnet is an owner-signed check in
  every `init`.
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
  transaction's gas limit (5M), as gas estimation can be fooled on EVM. The
  relayer only delivers a message that paid our Sepolia IGP for at least a
  relayed `process` (2.17M gas at threshold 1, with the margin worth 3.26M),
  and doesn't try it again until more is paid, so each such message costs
  its sender that payment and the relayer at most the rest of the 5M
  ([costs.md](costs.md#what-stays-uncovered)).
- **Open relaying, priced on chain.** The relayer relays any message
  between the two chains, from any sender to any recipient, that paid this
  deployment's IGP on its origin for enough gas. It trusts the IGPs' prices.
  If they fall behind the market by more than the margin, the relayer
  delivers at a loss until the owner updates them, and if the owner sets
  them too high, nobody can afford a transfer. It counts gas, not money, so
  a message priced at an IGP's old rate still passes. Anyone can pay for
  any message, on the Sepolia IGP with `payForGas` and on the Dusk IGP with
  `pay_for_gas`, and payments for one message add up. A message to Sepolia
  that paid for more gas is sent with that much as its limit, so a sender
  can buy the gas an expensive `handle` needs. A message to Dusk is sent
  with at most the relayer's limit, `DUSK_RELAYER_GAS_LIMIT` (5M), whatever
  it paid for. One whose delivery needs more passes the relayer's
  simulation, which runs with the block's gas, and fails on chain at the
  whole limit each time it is tried; paying more doesn't change that
  ([costs.md](costs.md#what-stays-uncovered)). A Sepolia-origin message
  sent through the Mailbox's default hook pays no IGP for Dusk (it falls
  back to the Merkle tree alone), so it isn't relayed until someone pays our
  IGP for it.
- **Recipients must not read `public_sender()`.** A recipient's `handle`,
  its `interchain_security_module` getter and any ISM it uses run inside the
  relayer's simulation, where `public_sender()` panics. If `handle` or an
  ISM calls it, every simulation fails and the relayer never submits the
  message. If the getter calls it, the simulation and the delivery can
  verify with different ISMs, and the delivery fails after the relayer has
  paid for it.
- **Collateral routes need exact transfers.** WarpDrc20Collateral checks
  that every `transfer_from` and `transfer` moves exactly the amount asked
  for. With a token that takes a fee on transfers, every transfer that pays
  one reverts, so only amounts too small to pay a fee can be bridged, and an
  inbound transfer that would pay one stays undelivered. A token whose
  balances change by themselves, such as a rebasing token, fails the same
  checks. Don't deploy a collateral route for either.
- **Contracts can't bridge.** `transfer_remote` takes its DUSK and fees as the
  transaction's deposit, which only the transaction's target contract can
  claim. A smart wallet or router contract calling a warp route always
  reverts.
- The Mailbox keeps 32 bytes per dispatched message, plus 16 bytes per block
  that has any, and about 26 bytes per delivered message. The Merkle hook
  keeps 32 bytes per message plus 16 per block, and the IGP 64 bytes per
  payment, from dispatches and top-ups alike; top-ups stop at 3 GiB.
  `dispatch` closes for good when the Mailbox reaches 3 GiB: after
  about 100M messages, or fewer when deliveries share the memory. Anyone
  can close it on purpose with dispatches, for about 116,000
  DUSK at the testnet protocol fee, mostly the fee, or with 97M to 122M
  deliveries, a transaction each, for 179,000 to 225,000 DUSK
  ([storage.md](storage.md#ceilings)). Monitor `memory_bytes`.
- **Contracts can't deliver or announce.** `process` and `announce` take
  only a transaction's own call or a query, so no contract can relay a
  message, to itself or for others, and a recipient can't deliver another
  message from its `handle`.
- **A collateral route inherits its token's transfer rules.** It pays out
  with the token's `transfer`. Over a WarpDrc20, once holders fill the
  token to its 3 GiB reserve, a payout to a registered account with no
  balance of the token reverts, so the message can't be delivered, and
  escrow for a new account can't be claimed. Wrapping a token with a
  holder reserve is unsupported: `dusk-tx deploy-hyperlane` refuses a
  WarpDrc20 as collateral, and the demo wraps the pinned canonical DRC20.
  Pinned by
  `known_limitation_collateral_route_over_warp_drc20_stops_paying_new_holders_at_the_reserve`.
- `dispatch` is permissionless, as in Hyperlane. Security comes from the
  destination ISM.
- Dispatch credit can only be withdrawn to Moonlight accounts, not to
  contracts.
- Not tested in the VM: Mailbox nonce exhaustion (needs about 4 billion
  dispatches), and the WarpDrc20 and IGP checked-arithmetic panics, which are
  only reachable with an already-broken supply invariant or an absurd gas
  configuration. These are reviewed statically.
