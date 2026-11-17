# Contract storage

Every contract keeps its state in its wasm32 linear memory, which holds at
most 4 GiB and never shrinks, and Dusk's gas prices what a call executes,
not the bytes it leaves behind. Before this design, a contract stored each
growing list in a `Vec` and failed at 2^25 entries or fewer, with most of
its memory still free; filling the Mailbox for good cost about 1,074 DUSK.
The contracts now keep growing lists in fixed 64 KiB chunks and store less
per entry, and every path anyone can add to takes a transaction per entry,
a fee, a cap, or a reserve that keeps the rest of the memory for the
bridge ([Who can add entries](#who-can-add-entries)).

This page lists what each contract stores, who can add to it, what the
operations cost in gas and memory, what it takes to fill each contract,
and what the agents read now that some data lives only in events.

## The limits

- A contract's memory grows in 64 KiB pages up to 4 GiB, and memory it
  frees stays part of it. The node persists all of it.
- dlmalloc manages the memory, and no single allocation can reach 2 GiB
  (`isize::MAX` on wasm32). A `Vec` that doubles as it grows therefore
  fails with "capacity overflow" when it would need a 2 GiB block: a
  `Vec<[u8; 32]>` at 2^25 = 33,554,432 entries, a `Vec<GasPaymentRecord>`
  at 2^24. Each doubling also needs the new block while the old one is
  still allocated.
- Contracts can't be upgraded. A contract whose memory is full stays full,
  and replacing the Mailbox means redeploying every route that uses it.

So everything a caller can add to must be bounded, priced, or both, and must
not fail before the memory is actually full.

## Layouts

| Contract | `state_version` | What grows | Per entry |
|---|---|---|---|
| Mailbox | 6 | Dispatched message IDs, by nonce | 32 bytes |
| | | Dispatch block heights | 16 bytes per block with a dispatch |
| | | Delivered messages: the first 16 bytes of each ID, in a `BTreeSet` | about 26 bytes |
| | | Dispatch credit, per payer that has some | about 64 bytes |
| MerkleTreeHook | 2 | Inserted message IDs, by leaf index | 32 bytes |
| | | Insertion block heights | 16 bytes per block with an insertion |
| IGP | 4 | Gas payment records, from dispatches and top-ups | 64 bytes |
| ValidatorAnnounce | 3 | Validators, in order of first announcement | 20 bytes |
| | | keccak256 of each announced location, at most 16 per validator | 32 bytes, plus about 58 for a validator's first |
| WarpDrc20 | 8 | Balances, keyed by account ID | about 62 bytes |
| | | Allowances, keyed by owner's and spender's account ID, at most 64 per owner | about 62 bytes, the owner's own map included |
| Warp routes | 5, 8, 7 | Registered accounts, one per account that claimed escrow | about 352 bytes, see [security.md](security.md#registry-growth) |
| | | Escrow, per recipient with something waiting | about 72 bytes |

What the contracts no longer store:

- the Mailbox's list of processed messages, their block heights, and each
  delivery's block height (`processed_count`, `processed_at_index`,
  `processed_block_height_at_index`, `delivered_at`). Agents read
  deliveries from `ProcessId` events.
- the MerkleTreeHook's root after each insertion (`root_at`). Computing it
  cost 32 keccak calls, 247,144 gas, on every dispatch. Validators compute
  it from the stored message IDs.
- ValidatorAnnounce's location strings. The location is in the
  `ValidatorAnnouncement` event, and the contract keeps its hash.
- WarpDrc20's principals as map keys. A Moonlight principal is 194 bytes;
  the account ID is 32.

### Chunked logs

`types::storage::ChunkedLog<T>` is an append-only log in chunks of 64 KiB:
2048 message IDs, 1024 gas payment records or 3276 validator addresses to a
chunk. A chunk is allocated whole when the first entry that falls into it is
pushed, and nothing moves after that. The only other allocation is the
table of chunks, which doubles and stays under 768 KiB for a log that fills
4 GiB. `push_allocations()` returns what the next push allocates, so a
contract can check it before allocating anything. Reads by index and pages
of up to 256 entries work as they did on the `Vec`s.

`types::storage::BlockHeights` stores a block height per entry of such a
log as runs: each block that adds entries adds one 16-byte record of its
first index and its height, and a lookup is a binary search over the runs.
Many entries in one block cost 16 bytes in all. One entry per block costs
16 bytes each, 50 MB a year at Dusk's block time, and only someone who pays
for an entry in every block can make it do that.

Both are property-tested against `Vec` in `types/src/storage.rs`, across
chunk boundaries and for every entry size the contracts use.

### Account IDs

WarpDrc20 keys balances and allowances by `drc20::account_id(principal)`:

```
keccak256("hyperlane-dusk/drc20-account" || kind || key bytes)
```

`kind` is one byte, 0 for Moonlight, 1 for Phoenix and 2 for a contract, and
the key bytes are the principal's as the DRC20 ABI carries them: the raw
193-byte BLS key, the 32-byte Phoenix key, or the contract ID. The domain
tag keeps the IDs apart from recipient hashes and message IDs, and the kind
byte keeps principals of different kinds apart, including any key format a
later principal kind adds. Moonlight keys that differ in their raw bytes get
different IDs, as they had different map entries before, so a transfer to a
forged twin of a key still goes to an account nobody can spend from. The
ABI and the events still take and carry principals.

Each ID costs one keccak256 host call on the principal, about 12,000 gas
for a Moonlight key. A transfer or an approval computes two, so it costs
about 24,000 gas more than before.

## Who can add entries

Ethereum charges about 20,000 gas to create one 32-byte storage slot. Dusk
charges nothing for the bytes a call leaves in a contract's memory: a
delivery adds 26 bytes to the Mailbox, and a contract making many calls in
one transaction paid about 35,000 gas for each. So every path anyone can
add to is bounded by what it costs to call, by a fee, or by a cap:

| Path | What bounds it | Since |
|---|---|---|
| Deliveries (`process`) | Only a transaction's own call or a query can deliver, so each delivery is a transaction, about 1.85M gas, and a block holds at most 1,000 | Mailbox 6 |
| Announcements (`announce`) | The same, about 2.06M gas each | ValidatorAnnounce 3 |
| Dispatches | The protocol fee, 0.001 DUSK on testnet; dispatch closes at 3 GiB ([below](#the-dispatch-limit)) | Mailbox 5 |
| Dispatch credit (`fund_dispatch`) | A transaction each, and at least 0.01 DUSK for a payer with no credit; new payers stop at 3 GiB, as dispatch does | Mailbox 6 |
| IGP top-ups (`pay_for_gas`) | A transaction each, and the gas bought; top-ups close at 3 GiB | IGP 4 |
| Route registrations | Only a claim of escrow registers an account, and escrow only comes from a delivery from the enrolled router: a transfer someone sent, and paid the relaying of, on the remote chain | routes 5, 8, 7 |
| WarpDrc20 allowances | At most 64 live allowances per owner, and approvals of new spenders stop at 3 GiB ([below](#the-warpdrc20-reserve)) | WarpDrc20 8 |
| WarpDrc20 holders | Transfers to new holders stop at 3 GiB; a holder costs a token unit and about 56,000 gas from a contract | WarpDrc20 8 |
| Escrow | A delivery from the enrolled router, as registrations | |

### A transaction's own call

`process` and `announce` start with `caller::is_transaction_or_query()`,
which reads the call stack (`abi::callstack()`, the frames below the
current one). A Moonlight or Phoenix transaction's own call has one frame
below it, the transfer contract, which runs every transaction. A query has
none, so the relayer's simulation of `process`, a query, still runs. Any
call from a contract has the calling contract below it, and is refused:
a contract called by a transaction, one called by a query, and a contract's
`init` while it is deployed, which also runs with nothing below it. So no
contract can make many deliveries or announcements in one transaction, and
a recipient can't call back into `process` from its ISM lookup, its ISM or
its `handle` (HIGH-6). The relayer, `dusk-tx process` and the validators
already sent one transaction each, so nothing they do changes. On EVM,
`process` takes any caller; on Dusk no contract can relay a message, to
itself or for others.

The check costs about 900 gas a delivery or announcement. The IGP's
`pay_for_gas` was the first entry point to check its caller this way; it
also needs the transaction's deposit, so it takes only a Moonlight
transaction's own call.

### Dispatch credit

`fund_dispatch` takes at least 0.01 DUSK (10,000,000 LUX) for a payer with
no credit, and any amount for one with credit. Credit that reaches zero,
spent by dispatches or withdrawn, leaves no entry, so funding that payer
again takes the minimum again: the minimum is what a new entry costs.
Withdrawing credit costs about 59M gas, 0.059 DUSK, since the Mailbox
decodes the recipient's key, so credit that small was never worth taking
back. A route funds its own credit through the transfer contract for each
transfer (`receive_dispatch_funding`) with no minimum: only the route can
fund its own ID that way, and the transfer spends it in the same call.

A payer with no credit is also refused once its entry could take the
Mailbox past 3 GiB, where dispatch closes (`Mailbox: storage full, credit
for new payers is closed`), checked before the entry is allocated, by the
dispatch limit's rule: an entry can allocate at most 13 B-tree nodes, each
under 600 bytes, so at most 13 pages. Credit only pays for dispatches, so
nothing honest needs a new payer past that point, and the last GiB stays
for deliveries. Top-ups and withdrawals change an existing entry and go on.
The same check covers `receive_dispatch_funding`, though each new entry
there is a contract of its own, a deployment of at least 10 DUSK.

### Route registrations

A route registers an account only when its escrow is claimed, with
`claim_pending` (the account itself) or `claim_pending_for` (anyone, for
it). Both revert with nothing in escrow, and escrow only comes from a
delivery from the route's enrolled router. Routes have no
`register_account`, so a funded key can't register itself, and every entry
in a registry stands for a transfer someone sent from the remote chain. A
new account's first transfer on each route waits in escrow until it is
claimed; later ones arrive directly. That's as many transactions for the
account as a registration first, but a wallet can't register an account
ahead.

## The dispatch limit

`dispatch` refuses a message unless everything the rest of the call can
allocate fits under 3 GiB. After it has encoded and quoted the message, it
adds up the allocations it is about to make: a new chunk of message IDs or
of heights if the last one is full, a larger chunk table if that is full,
and the copies of the message and metadata the `Dispatch` event and the
hook calls take. dlmalloc grows memory, when no free block fits, by the
request plus 59 bytes of its own, rounded up to whole pages. If the memory
in use plus that bound would pass 3 GiB, the dispatch reverts with
`Mailbox: storage full, dispatch is closed`.

The check is made before the allocations it counts, so what a dispatch
allocates never takes the Mailbox past 3 GiB, and the last GiB stays for
the other calls: deliveries above all, and withdrawals, top-ups of existing
credit and any call's temporary buffers. Credit for a new payer stops at
the same limit ([Dispatch credit](#dispatch-credit)). No hook can deliver a
message during a dispatch, since a contract can't call `process`. The old
check only looked at the memory already in use, and a `Vec` doubling after
it could take 1.25 GiB of the reserve at once.

The bound is 4 pages for a dispatch with no metadata that starts no chunk,
6 for one that starts a chunk of message IDs, and 10 for the first dispatch,
which starts both logs and both chunk tables. It is largest, about 1.2 MiB,
for the dispatch that doubles the ID chunk table from 32,768 to 65,536
chunks. Dispatch closes when the next dispatch's bound no longer fits, so
that much of the 3 GiB at most stays unused.
`test_dispatch_never_takes_the_mailbox_past_3_gib` runs the deployed
Mailbox code in a memory that starts 12 pages below the limit, dispatches
until it closes, and checks that every admitted dispatch leaves the memory
at or below 3 GiB and that the Mailbox admits exactly the dispatches whose
bound fits. It then delivers messages until the memory is past 3 GiB,
about 18,000 of them from the 5 pages below the limit where dispatch
closed, each as a call of its own, and checks that a fresh message is
still reserved, handled and refused as a replay there.

Deliveries share the Mailbox's memory and count toward the 3 GiB too, so
inbound traffic closes dispatch sooner, and deliveries alone close it after
about 97M to 122M (the range is the order keys arrive in, see
[Key order](#key-order)). Each takes a transaction, so that costs about
179,000 to 225,000 DUSK and 11 to 14 days of full blocks.
Dispatching is the cheaper way to close dispatch on purpose: about 100M
dispatches with a free hook of the sender's own, about 116,000 DUSK
at the testnet protocol fee of 0.001 DUSK, most of it the fee, and 10
times that at 0.01. The design accepts it: reserving memory for dispatch
as well would let dispatch traffic close `process` instead. Past 3 GiB,
deliveries go on at about 26 bytes each, 32M to 41M more.

## The WarpDrc20 reserve

WarpDrc20 is a DRC20 token anyone can hold, so anyone holding a token unit
can add holders, one 62-byte balance per unit sent to a new address, and
anyone can approve spenders. Two limits keep that from filling the token's
memory:

- **An owner holds at most 64 allowances.** Approving a 65th spender
  reverts with `WarpDrc20: an owner holds at most 64 allowances` until the
  owner approves zero for one of them, which removes it, or a spender uses
  one up with `transfer_from`. Changing an allowance the owner holds always
  works. A contract owner then costs a deployment per 64 approvals, at
  least 5M gas at 2,000 LUX (10 DUSK), and a Moonlight owner a transaction
  per approval.
- **Transfers to new holders and approvals of new spenders stop at 3 GiB.**
  `transfer` and `transfer_from` to an address with no balance revert with
  `WarpDrc20: storage full, transfers to new holders are closed`, and
  `approve` of a new spender with `WarpDrc20: storage full, new allowances
  are closed`, once what they can allocate could take the memory past
  3 GiB. Transfers between holders and changes to existing allowances
  keep working, and so does the bridge: mints, escrow, and the claims that
  register an account and mint to it use the last GiB, which holds
  2.1M to 2.6M new bridged accounts at a registration and a balance
  each.

The check works as the dispatch limit's, before anything is stored. Adding
a key to a `BTreeMap` allocates at most one node per level of the tree,
when every level splits, and a new root. Every node but the root holds at
least 5 keys, so a map of fewer than 2^32 keys has at most 12 levels, and
an insertion allocates at most 13 nodes; an owner's first allowance adds
the first node of its own map of spenders. Each node takes under 600
bytes, and dlmalloc grows memory, when no free block fits, by whole pages,
so a new balance can grow memory by at most 13 pages and a new allowance
by 14. A transfer to a new holder is refused once the memory in use plus
13 pages would pass 3 GiB, an approval of a new spender plus 14.

`test_warp_drc20_new_holders_and_allowances_stop_at_3_gib` runs the
deployed token code in a memory that starts 16 pages below the limit,
transfers to new holders until they close, and checks that every admitted
transfer leaves the memory at or below 3 GiB, that the token admits
exactly the transfers whose bound fits, and that approvals of new
spenders close a page earlier. Then transfers between holders and changes
to an allowance still work, the bridge escrows transfers to new
recipients until the memory is past 3 GiB, and past it a claim registers
an account and mints to it, a later transfer mints to it directly, and a
transfer to a new holder is still refused.

This contains the holder path, the cheapest damage anywhere after these
limits: filling the token's 3 GiB with transfers of one unit to new
holders costs 2,300 to 2,900 DUSK of gas, from a contract, and 42M to 52M token units, and then nobody can send tokens or
approvals to a new address on Dusk, but the bridge keeps paying out. A
dust threshold for new holders, and a price per byte from Rusk, are
later stages of the plan.

## The 16-byte delivery key

The Mailbox records a delivered message by the first 16 bytes of its ID,
`keccak256(message)`, in a `BTreeSet<[u8; 16]>`. `delivered(id)` still
takes the full 32-byte ID and checks its first 16 bytes. The reservation is
made before any external call, as it was with the full ID: `process` inserts
the key, refuses the message if it was already there, and only then looks
up the recipient's ISM, verifies and calls `handle`. A failed delivery
reverts the insertion with everything else.

- **No message is delivered twice.** The same message always has the same
  ID, so a replay always finds its key.
- **An honest message can only be blocked by a second preimage.** Another
  message is refused as delivered only if its ID shares the first 16 bytes
  of a delivered one. To block a message someone else sent, an attacker has
  to deliver a different message whose ID has the same 128-bit prefix: a
  second preimage on 128 bits of keccak256, about 2^128 hashes. Finding two
  messages of its own with the same prefix takes about 2^64, but blocks only
  the attacker's own message, since an honest message's bytes, including its
  origin nonce and sender, aren't the attacker's to choose.
- **Accidental collisions don't happen.** Over n deliveries the chance that
  any two IDs share a prefix is about n^2 / 2^129: 2^-74 at the 163M
  deliveries that fill the memory, 2^-65 at 2^32.

A collision would make the relayer's `delivered` check skip the message and
`process` refuse it, so the cost of one is a stuck message, not a double
delivery. Storing 16 bytes instead of 32 lets the set hold about 163M
deliveries instead of 86M.

## Costs per operation

Measured in the VM with `tests/tests/storage_costs.rs` (see
[Measuring](#measuring)), before this design (the contracts at
`fa2d676`) and after (at `d6b0b3e`, before stage 1 of the state-growth
plan: since then no contract can deliver or announce, so the marginal rows
for those no longer apply, and [costs.md](costs.md) has the current
figures). "Transaction" is one Moonlight transaction making one
call, the 1.75M gas signature check included: what a user, relayer or
validator pays. "Marginal" is what one more call adds to a transaction that
makes many from a contract: what someone filling a contract pays. Memory is
how much the contract's memory grew per call over 32,768 calls (65,536
deliveries), in whole pages, so it includes allocator overhead and, before,
the doubling `Vec`s.

| Operation | Gas before | Gas after | Memory before | Memory after |
|---|---|---|---|---|
| Dispatch through the deployed hooks (MerkleTreeHook and ProtocolFee required, IGP by default), transaction | 2,267,855 | 2,020,810 | | |
| Same, marginal | 427,068 | 180,310 | Mailbox 134, MerkleTreeHook 264, IGP 262 bytes | Mailbox 32, MerkleTreeHook 32, IGP 64 bytes |
| Dispatch with a free hook instead of the IGP, marginal | 400,769 | 154,582 | Mailbox 130, MerkleTreeHook 130 bytes | Mailbox 32, MerkleTreeHook 32 bytes |
| Dispatch, MerkleTreeHook required and a free default hook, marginal | 325,914 | 78,925 | | |
| IGP payment (dispatch through the IGP minus a free hook), marginal | 26,299 | 25,728 | 262 bytes | 64 bytes |
| Process, native payout, 1 signature, transaction | 2,143,024 | 2,141,730 | | |
| Process, native payout, 6 signatures, transaction | 3,230,604 | 3,229,311 | | |
| Process, recipient that is its own ISM, marginal | 37,032 | 35,107 | Mailbox 166 bytes | Mailbox 26 bytes |
| Announce, new validator, 46-byte location, transaction | 2,070,799 | 2,067,372 | | |
| Announce, new validator, 1 KiB location, marginal | 450,562 | 445,839 | 1,250 bytes | 110 bytes |
| DRC20 transfer to a new holder, transaction | 1,851,206 | 1,875,847 | | |
| Same, marginal | 32,835 | 57,098 | 356 bytes | 60 bytes |
| DRC20 approval of a new spender, transaction | 1,845,349 | 1,873,039 | | |
| Same, marginal | 29,923 | 53,156 | 358 bytes | 62 bytes |
| Withdraw dispatch credit, transaction | 24,380,024 | 58,696,092 | | |

- Dispatch is 246,758 gas cheaper: the MerkleTreeHook no longer computes
  and stores the root on every insertion. With no fee-charging hook, a
  dispatch costs 4.1 times less, 78,925 gas instead of 325,914 (the study's
  model of the same went from 312,003 to 64,859).
- Delivery costs about 1,300 gas less: one insertion of 16 bytes instead
  of a map entry and two `Vec` pushes.
- A DRC20 transfer or approval costs about 24,000 gas more, for the two
  account IDs (see [Account IDs](#account-ids)).
- Withdrawing dispatch credit costs 34M gas more. It used to check the
  recipient key with `is_valid()`, 22M gas, which accepts a forged twin of
  the key; decoding it costs 57M ([security.md](security.md), LOW-7).
- The IGP at `state_version` 4, which takes top-ups, records a dispatch's
  payment for 643 gas less than the IGP measured here: a dispatch through
  the deployed hooks takes 2,020,167 gas, 179,667 marginal, and the IGP
  payment 25,085. [costs.md](costs.md) has the current figures, top-ups
  included.

## Ceilings

How many entries fill each contract, and what they cost at the minimum gas
price of 1 LUX, with the gas per entry of [costs.md](costs.md#attack-costs).
"Before" is the ceiling study's measurement of the contracts before this
design. "Now" is for the contracts after stage 1 of the state-growth plan:
the ranges run from keys in increasing order to keys in random order
([Key order](#key-order)).

| What fills | Before | Now |
|---|---|---|
| Mailbox, deliveries to a recipient that is its own ISM | dispatch closed at 31M, about 991 DUSK; `process` failed at 33.5M, "capacity overflow" in the processed list, about 1,074 DUSK | a transaction each: dispatch closes after 97M to 122M, 179,000 to 225,000 DUSK; `process` fails at 130M to 163M, the full 4 GiB, 239,000 to 301,000 DUSK |
| Mailbox, dispatches | 33.5M; "capacity overflow"; about 10,500 DUSK, and the 3 GiB limit never fired | about 100M, when dispatch closes at 3 GiB: about 116,000 DUSK with a free hook, 15,500 of it gas and the rest the 0.001 DUSK protocol fee; 1.02M at a 0.01 DUSK fee |
| Mailbox, both | | dispatches to 3 GiB, then 33M to 41M deliveries for the last GiB: 177,000 to 192,000 DUSK, the cheapest way to make `process` fail |
| Mailbox, credit for new payers | | 40M to 50M payers to 3 GiB, where new payers stop, at 0.01 DUSK each: 475,000 to 595,000 DUSK |
| MerkleTreeHook | 33.5M dispatches, with the Mailbox | 134M; the Mailbox closes dispatch first |
| IGP | 16.8M payments; about 5,600 DUSK plus IGP fees | about 67M payments. Top-ups close at 3 GiB, about 94,000 DUSK at the testnet pricing; dispatches through the IGP pay 6.001 DUSK each there, about 403 million DUSK to fill it |
| ValidatorAnnounce, new keys | 3.5M announcements of 1 KiB locations; about 1,257 DUSK | 36M to 40M, a transaction each: 74,000 to 82,000 DUSK with 1-byte locations |
| WarpDrc20, new holders | 14.3M; about 227 DUSK | 42M to 52M to the 3 GiB reserve: 2,300 to 2,900 DUSK, plus a token unit each |
| WarpDrc20, approvals of new spenders | 14.3M; about 156 DUSK | 42M to 52M to the 3 GiB reserve, at most 64 per owner and a transaction each: 79,000 to 99,000 DUSK |
| Warp route registry | 12.8M registrations; about 23,700 DUSK, 47,400 with a funding transfer for each key | 9.8M to 12.2M claims of escrow, each a transfer someone sent from the remote chain: 39,000 to 71,000 DUSK of Dusk gas and 1.3 to 1.6 ETH of IGP fees |
| Warp route escrow | | 48M to 60M deliveries from the enrolled router: 101,000 to 127,000 DUSK of the relayer's gas, repaid by 6.2 to 7.8 ETH of IGP fees |

What happens at each ceiling:

- **Mailbox.** At 3 GiB dispatch and credit for new payers close and stay
  closed; deliveries, withdrawals and top-ups go on in the last GiB, which
  only deliveries can fill. If deliveries fill all 4 GiB,
  `process` fails and the Mailbox has to be replaced, with every route on
  it. Doing that on purpose takes dispatches to 3 GiB and 33M to 41M
  deliveries, about 177,000 to 192,000 DUSK and 4 days of full blocks for
  the deliveries, or deliveries alone, 239,000 to 301,000 DUSK and 15 to
  19 days.
- **MerkleTreeHook.** Not reachable: the Mailbox closes dispatch at about
  100M messages, the hook holds 134M.
- **IGP.** Top-ups stop at 3 GiB, and the last GiB is left to the
  payments dispatches make. When it is full, payments fail, so dispatches
  through the IGP revert; dispatches with another hook still work. The
  owner can set a new default hook.
- **ValidatorAnnounce.** New validators can't announce; existing
  announcements stay readable. Agents need a new ValidatorAnnounce.
- **WarpDrc20.** At 3 GiB transfers and approvals to new addresses stop,
  and holders can still pay each other and change their allowances. The
  bridge goes on in the last GiB: mints to accounts with no balance, escrow
  and claims, until the memory is full.
- **Warp routes.** A route whose memory is full can't register an account
  or open an escrow entry: transfers to new accounts can't be delivered,
  and WarpDrc20 can't mint to an account with no balance.

### Key order

The bytes per entry in this page and in [costs.md](costs.md) hold for keys
in random order. An attacker can compute keys ahead, such as message IDs,
account IDs, recipient hashes, payer IDs and validator addresses, and
insert them in increasing order. Rust's B-tree then splits each full node
so that the side it won't insert into again keeps 6 of its 11 keys, and
the nodes stay about 55% full: 33.1 bytes per delivered key instead of
26.4, 77.2 instead of 61.6 for a map of 32-byte keys, and 429 instead of
343.5 for a registration (the state study's allocator measurement). Filling
a B-tree costs about 20% less than in random order, and the tables above
give both ends. Chunked logs don't depend on the order.

## What agents read

The agents in `hyperlane-monorepo` read state that this design removes.
They need these changes before they can run against these contracts.

### Versions

| Contract | `state_version` | Was |
|---|---|---|
| Mailbox | 6 | 4 (5 for this design, before stage 1) |
| MerkleTreeHook | 2 | 1 |
| IGP | 4 | 2 |
| ValidatorAnnounce | 3 | 1 (2 for this design, before stage 1) |
| WarpDrc20 | 8 | 6 (7 for this design, before stage 1) |
| WarpNative, WarpDrc20Collateral | 5, 7 | 4, 6 |

The agents' chain identity check requires Mailbox 6, MerkleTreeHook 2 and
ValidatorAnnounce 3: the first versions without the queries this design
removed and with stage 1's limits. Stage 1 changed no query or event the
agents read: Mailbox 6 and ValidatorAnnounce 3 take `process` and
`announce` only as a transaction's own call or a query, which is how the
agents send them and the relayer simulates `process`. The IGP's queries are
unchanged. IGP 3 stored its records in chunks; 4 adds top-ups, which write
the same records and events, so the agents need no IGP version.

### Mailbox

Removed queries:

- `processed_count() -> u32`
- `processed_at_index(index: u32) -> [u8; 32]`
- `processed_block_height_at_index(index: u32) -> u64`
- `delivered_at(id: [u8; 32]) -> u64`

Added:

- `quote_dispatch_for_sender(sender: [u8; 32], destination: u32, recipient: [u8; 32], body: Vec<u8>, metadata: Vec<u8>, hook: ContractId) -> u64`:
  what `dispatch` would charge `sender` now, the required hook's quote plus
  the selected hook's (a zero `hook` selects the default). It works as a
  read-only query. `quote_dispatch` still quotes for the caller and so
  still fails as one.

Unchanged: `delivered(id: [u8; 32]) -> bool`, now true for any ID whose
first 16 bytes match a delivered message's; `nonce`, `dispatched_id(nonce)`
and `dispatched_block_height(nonce)`, which panic with
`Mailbox: nonce out of range` past the end; `latest_dispatched_id`,
`memory_bytes`, and the configuration and credit queries.

Deliveries are indexed from events. Each successful `process` emits, in
this order and once each:

- topic `process`, `Process { origin: u32, sender: [u8; 32], recipient: [u8; 32] }`
- topic `process_id`, `ProcessId { message_id: [u8; 32] }`

A delivery's block height is its event's block. Index both from finalized
blocks only, and skip events the receipt flags as reverted: a call that
fails inside a transaction that succeeds, such as a delivery a recipient
attempts during another one, leaves its events in the receipt with the
flag set. Use `delivered(id)` to check a single message.

`dispatch`, `dispatch_id` and the fee events are unchanged.
`DispatchFeeWithdrawn.recipient` is the keccak256 of the decoded
recipient's compressed key, the same value as before for any honest key.

### MerkleTreeHook

Removed: `root_at(index: u32) -> [u8; 32]`.

Unchanged: `count() -> u32`, `root() -> [u8; 32]` (the current root),
`latest_checkpoint() -> ([u8; 32], u32)` (the current root and
`count - 1`), `tree() -> IncrementalMerkle { branch: [[u8; 32]; 32], count: u32 }`,
`message_id_at(index) -> [u8; 32]`, `message_ids(start, limit) -> Vec<[u8; 32]>`
(at most 256), `inserted_block_height(index) -> u64`, and the
`inserted_into_tree` event, `InsertedIntoTree { message_id: [u8; 32], index: u32 }`.

A validator computes the root at leaf index `i` from the stored IDs:

1. Read the IDs at indexes 0 to `i` in order with `message_ids(start, 256)`,
   checking that each page is as long as asked.
2. Insert them in order into an empty depth-32 keccak256 incremental tree,
   `hyperlane_core::accumulator::incremental::IncrementalMerkle` (the hook
   runs the same algorithm, `types::merkle::IncrementalMerkle`).
3. Its `root()` is the root at `i`.

Keep the tree between checkpoints and insert only the new IDs. The IDs are
the hook's own state, as authenticated as `root_at` was. The finalized
count at height `h` is still the number of indexes whose
`inserted_block_height` is at most `h` (a binary search), and the
checkpoint at `h` is that count minus one, the root computed for it, and
`message_id_at` of it.

To check the local tree against the hook, compare its root and count with
`root()` and `count()` whenever `count()` is `i + 1`. Don't compare its
whole branch with `tree().branch`: the hook starts every branch slot at
zero, while hyperlane-core's `IncrementalMerkle::default()` starts slot `l`
at `ZERO_HASHES[l]`. Neither reads a slot before writing it, so the roots
agree, but slot `l` keeps its initial value until the count reaches `2^l`,
and the two arrays differ there. Compare `branch[l]` only for the levels
whose bit is set in the count, or start the local tree from zeroed slots
(`IncrementalMerkle { branch: [H256::zero(); 32], count: 0 }`), whose
branch then equals the hook's. `types/tests/upstream_merkle.rs` checks
both at counts from 1 to 100.

### ValidatorAnnounce

Removed:

- `get_announced_storage_locations(validators: Vec<EthAddress>) -> Vec<Vec<String>>`
- `get_announced_storage_locations_for_validator(validator: EthAddress) -> Vec<String>`

Added: `get_announced_storage_location_hashes(validator: EthAddress) -> Vec<[u8; 32]>`,
the keccak256 of each location the validator has announced, as UTF-8 bytes,
oldest first, at most 16, empty if it never announced.

Changed: `get_announced_validators(start: u32, limit: u32) -> Vec<EthAddress>`
takes a `limit` of 1 to 256, up from 2. `announced_validator_count() -> u32`
and `announce(validator, storage_location, signature) -> bool` are
unchanged.

The locations come from events. Each successful `announce` emits topic
`validator_announcement`,
`ValidatorAnnouncement { validator: EthAddress([u8; 20]), storage_location: String }`.

- **Relayer**, for each validator `v` of the ISM it needs signatures from:
  read `get_announced_storage_location_hashes(v)`; take the finalized
  `validator_announcement` events of the contract, from its deployment
  block, whose validator is `v`; keep each location whose keccak256 is in
  the list; return them in the list's order. The last is the latest. A
  hash with no event found yet means the scan hasn't reached it.
- **Validator**, at startup: it has announced its location if
  keccak256(location) is in its own list. Otherwise it announces. A
  validator can't announce the same location twice, or more than 16.
- **Listing validators**: `announced_validator_count`, then pages of
  `get_announced_validators`.

### IGP

Queries and events are unchanged: `gas_payment_count`, `gas_payment_at`,
`gas_payments` (pages of 256) and `gas_payment`. `gas_payment_at` past the
end now panics with `IGP: gas payment index out of bounds`.

`state_version` 4 adds `pay_for_gas(message_id, destination, gas_amount,
deposit)`, which anyone can call to pay for more gas for a message, and
`quote_pay_for_gas(destination, gas_amount) -> u64`, its price. A top-up
appends a record to the same log, `GasPaymentRecord { message_id,
destination, gas_limit, payment, block_height }` with `gas_limit` the gas
it bought (a dispatch's record counts the overhead in), and emits the same
topic `gas_payment`, `GasPayment { message_id, gas_limit, payment }`. Index
both alike and add up the gas of every record with a message's ID and
destination, as hyperlane-base does. Several records can name one message,
and identical ones can share a block.

### WarpDrc20

No change to the ABI or events. Tools that need a balance's storage key
compute `account_id` as above.

## Measuring

```bash
make all
cargo test -p hyperlane-dusk-integration-tests --test storage_costs -- --ignored --nocapture
```

Each measurement prints a `cost|operation|metric|value` line. Set
`HYPERLANE_WASM_DIR` to a directory of contract WASMs built from another
commit to measure those instead; the harness only makes calls both versions
have. Since stage 1, it measures deliveries and announcements as
transactions, for the gas, and direct calls, for the memory, and approvals
from 512 owners of 64 each. A run takes about 8 minutes, and longer for
contracts that compute a root on every dispatch.

[costs.md](costs.md) has the cost of every entry point, the budgets the
tests hold them to, and the memory per entry of every path anyone can grow,
from `tests/tests/cost_profile.rs`.
