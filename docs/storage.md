# Contract storage

Every contract keeps its state in its wasm32 linear memory, which holds at
most 4 GiB and never shrinks. Before this design, a contract stored each
growing list in a `Vec` and failed at 2^25 entries or fewer, with most of
its memory still free; filling the Mailbox for good cost about 1,074 DUSK.
The contracts now keep growing lists in fixed 64 KiB chunks and store less
per entry, and the cheapest way to fill any of them costs about 3,700 DUSK.

This page lists what each contract stores, what the operations cost in gas
and memory, what it takes to fill each contract, and what the agents read
now that some data lives only in events.

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
| Mailbox | 5 | Dispatched message IDs, by nonce | 32 bytes |
| | | Dispatch block heights | 16 bytes per block with a dispatch |
| | | Delivered messages: the first 16 bytes of each ID, in a `BTreeSet` | about 26 bytes |
| | | Dispatch credit, per payer that funded some | about 60 bytes |
| MerkleTreeHook | 2 | Inserted message IDs, by leaf index | 32 bytes |
| | | Insertion block heights | 16 bytes per block with an insertion |
| IGP | 3 | Gas payment records | 64 bytes |
| ValidatorAnnounce | 2 | Validators, in order of first announcement | 20 bytes |
| | | keccak256 of each announced location, at most 16 per validator | 32 bytes, plus about 58 for a validator's first |
| WarpDrc20 | 7 | Balances, keyed by account ID | about 61 bytes |
| | | Allowances, keyed by owner's and spender's account ID | about 61 bytes, plus a map per owner |
| Warp routes | 4, 7, 6 | Registered accounts, escrow | unchanged, see [security.md](security.md#open-registry-growth) |

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
the other calls: deliveries above all, and withdrawals, credit funding and
any call's temporary buffers. A hook that delivers a message during a
dispatch allocates for that delivery, which may use the last GiB. The old
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
at or below 3 GiB, that the Mailbox admits exactly the dispatches whose
bound fits, and that deliveries keep working past the limit.

Deliveries share the Mailbox's memory and count toward the 3 GiB too, so
inbound traffic closes dispatch sooner, and deliveries alone close it after
about 122M. That is the cheapest way to close dispatch on purpose: about
4,300 DUSK of gas, delivering messages to a recipient that names itself as
its ISM. The design accepts it: reserving memory for dispatch as well would
let dispatch traffic close `process` instead. Past 3 GiB, deliveries go on
at about 26 bytes each, about 41M more.

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
`fa2d676`) and after. "Transaction" is one Moonlight transaction making one
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
| Same, marginal | 29,923 | 53,134 | 358 bytes | 62 bytes |
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

## Ceilings

How many operations fill each contract, and what they cost at the minimum
gas price of 1 LUX with the marginal gas above. "Before" is the ceiling
study's measurement of the old contracts. "After" is the number of entries
that fit: the study ran the Mailbox's delivered set, the chunked logs and
the DRC20 maps to the full 4 GiB (or to the 3 GiB dispatch limit), and the
ValidatorAnnounce figure divides 4 GiB by the 110 bytes measured above.

| What fills | Before | After |
|---|---|---|
| Mailbox, deliveries to a recipient that is its own ISM | dispatch closed at 31M, about 991 DUSK; `process` failed at 33.5M, "capacity overflow" in the processed list, about 1,074 DUSK | dispatch closes at about 122M, about 4,300 DUSK; `process` fails at about 163M, the full 4 GiB, about 5,700 DUSK |
| Mailbox, dispatches | 33.5M; "capacity overflow"; about 10,500 DUSK, and the 3 GiB limit never fired | about 100M, when dispatch closes at 3 GiB; about 15,500 DUSK of gas and, at the testnet protocol fee of 0.001 DUSK, 100,000 DUSK of fees |
| MerkleTreeHook | 33.5M dispatches, with the Mailbox | 134M; the Mailbox closes dispatch first |
| IGP | 16.8M payments; about 5,600 DUSK plus IGP fees | about 67M payments; about 12,100 DUSK plus IGP fees |
| ValidatorAnnounce, new keys | 3.5M announcements of 1 KiB locations; about 1,257 DUSK | about 39M; about 17,400 DUSK |
| WarpDrc20, new holders | 14.3M; about 227 DUSK | about 70M; about 4,000 DUSK, plus a token unit each |
| WarpDrc20, approvals of new spenders | 14.3M; about 156 DUSK | about 70M; about 3,700 DUSK |
| Warp route registry | 12.7M registrations; 23,500 to 46,900 DUSK | unchanged |

What happens at each ceiling:

- **Mailbox.** At 3 GiB dispatch closes and stays closed; deliveries and
  withdrawals go on in the last GiB. If deliveries fill all 4 GiB,
  `process` fails and the Mailbox has to be replaced, with every route on
  it. Doing that on purpose takes about 163M deliveries and 5,700 DUSK of
  gas; closing only dispatch, about 122M and 4,300 DUSK.
- **MerkleTreeHook.** Not reachable: the Mailbox closes dispatch at about
  100M messages, the hook holds 134M.
- **IGP.** Payments fail, so dispatches through the IGP revert; dispatches
  with another hook still work. The owner can set a new default hook.
- **ValidatorAnnounce.** New validators can't announce; existing
  announcements stay readable. Agents need a new ValidatorAnnounce.
- **WarpDrc20.** New holders and new allowances can't be added, which
  includes minting to an account with no balance. Existing balances still
  move.

## What agents read

The agents in `hyperlane-monorepo` read state that this design removes.
They need these changes before they can run against these contracts.

### Versions

| Contract | `state_version` | Was |
|---|---|---|
| Mailbox | 5 | 4 |
| MerkleTreeHook | 2 | 1 |
| IGP | 3 | 2 |
| ValidatorAnnounce | 2 | 1 |
| WarpDrc20 | 7 | 6 |

The agents' chain identity check should require Mailbox 5, MerkleTreeHook 2
and ValidatorAnnounce 2: all three removed queries the agents use. The IGP's
queries are unchanged.

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
the hook's own state, as authenticated as `root_at` was. To check the
tree, compare it with `root()` and `tree().branch` whenever `count()` is
`i + 1`. The finalized count at height `h` is still the number of indexes
whose `inserted_block_height` is at most `h` (a binary search), and the
checkpoint at `h` is that count minus one, the root computed for it, and
`message_id_at` of it.

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
have. A run takes about 8 minutes, and longer for contracts that compute a
root on every dispatch.
