# Costs

What every public entry point of the production contracts costs on chain,
the budgets the tests hold each cost to, what the relaying fees charge
against what relaying costs, what it costs to fill each contract's memory
on purpose, and how much traffic a block takes. All numbers are measured in
the VM by `tests/tests/cost_profile.rs`, on the contracts configured as
`deploy/testnet.sh` deploys them: the MessageIdMultisig ISM as the default
ISM, MerkleTreeHook and ProtocolFee (0.001 DUSK) aggregated as the required
hook, the IGP as the default hook priced for the EVM chain, and the three
warp routes. The profile prices the EVM chain at a token-sized 25,000 LUX
per transfer: what a call costs in gas doesn't depend on the price, and
[Fee coverage](#fee-coverage) has what the configured pricing charges. Gas
prices are 1 LUX per gas, the minimum, so 1M gas is 0.001 DUSK.

## Measuring

```bash
make all canonical-drc20-fixture
cargo test -p hyperlane-dusk-integration-tests --test cost_profile -- --nocapture

# Memory per entry, over 32,768 entries a path (4,096 for credit, top-ups and
# registrations, which take two transactions or a deposit each). About 10
# minutes in release mode, after the build.
cargo test --release -p hyperlane-dusk-integration-tests --test cost_profile \
    growth -- --ignored --nocapture
```

Every value is printed as `cost|<row>|<metric>|<value>`. Set
`HYPERLANE_WASM_DIR` to a directory of contract WASMs to profile those
instead of this checkout's build.

The four default tests run in the normal suite (about 40 seconds) and fail
when a row exceeds its budget in `BUDGETS`, when a row has no budget, or
when a platform limit is crossed. A transaction row records:

- **Tx gas**: what the Moonlight transaction spends.
- **Call gas**: what the called contract and everything it calls spend: tx
  gas minus the transfer contract's own work, which is mostly the BLS
  signature check (see [below](#what-every-transaction-pays)).
- **Calls**: the inter-contract calls the called contract makes, directly or
  through others, from the receipt's call tree. A call that fails and is
  caught, such as the Mailbox's ISM lookup on a recipient without the
  getter, doesn't appear.
- **Depth**: the deepest stack of contract frames, the transfer contract's
  included. The VM allows 48.
- **Events** and **event bytes**: the events the transaction leaves, the
  transfer contract's included, and the bytes of their topics and data.
- **Tx bytes**: the transaction as the network carries it
  (`Transaction::to_var_bytes`). A block stores 13 bytes more per
  transaction.

A query row records the query's gas and the bytes of its reply.

Entry points that only run inside another contract's call are measured in
the rows that make the call: the hooks' `post_dispatch` and
`receive_payment`, and the Mailbox's `receive_dispatch_funding` and
`quote_dispatch_for_contract`, in the transfer and dispatch rows; the
routes' `handle` and `interchain_security_module`, in the delivery rows.
`quote_dispatch`, which quotes for its caller, costs what
`quote_dispatch_for_sender` does. `init` runs once, when a contract is
deployed ([testnet.md](testnet.md#before-you-start) has what deploying
costs).

### Budgets

Each row's budget is its measured value plus 5% for gas and 10% for bytes,
rounded up. Calls, depth and event counts have no margin: they only change
when the code does. The platform limits the tests also check:

| Limit | Value | Checked on |
|---|---|---|
| Relayer gas limit (`DUSK_RELAYER_GAS_LIMIT`) | 5M, with 1.5x headroom | every relayed delivery to a route, at 1, 3 and 6 signatures |
| Least relayer limit `deploy/testnet.sh` accepts | 1.5 x (1.95M + 0.22M per signature) | the same deliveries, at their signature count |
| A call's arguments, a query's reply, a serialized transaction | 64 KiB | every query reply; every transaction |
| Call depth | 48 frames | every transaction |
| Query gas | 3e9, the block gas limit | every query |

A change that makes something cost more fails its row. Raise the budget in
`tests/tests/cost_profile.rs` and the row here together, saying why in the
commit. Two mutations of the Mailbox, built in a scratch copy and profiled
with `HYPERLANE_WASM_DIR`, show the budgets catch regressions: a loop of
about 43,000 gas in `process` fails the call gas budget of all 21
deliveries at one and three signatures (at six it is within the 5%), and a
second `dispatch_id` event fails the event budget of all 9 dispatches. A
loop of about 865,000 gas fails the relayer's 1.5x headroom at 3
signatures.

## What every transaction pays

The transfer contract's own work in a Moonlight transaction, mostly the
BLS signature check, is 1,795,675 gas plus 80 gas per transaction byte,
within 2,500 gas over all 110 transactions measured: 1.82M for a 250-byte
call, 1.84M for a relayed delivery at one signature, 1.87M at six, and
6.9M for a 63.5 KB transaction. Call gas is tx gas minus this.

## Transactions

### Users

`transfer_remote` deposits are the dispatch fee, plus the amount on the
native route. In the profile the fee is 1,025,000 LUX: the 0.001 DUSK
protocol fee and the IGP's 25,000 LUX. With the pricing
`deploy/testnet.env.example` configures, the IGP's part is 6 DUSK
([Fee coverage](#fee-coverage)); the gas is the same. Dispatching from an
account spends the fee from its prepaid credit. The largest body is the
largest whose transaction fits 64 KiB.

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| WarpNative transfer_remote | 2,131,122 | 301,480 | 27 | 7 | 14 | 1,937 | 404 |
| WarpNative transfer_remote, first message on the Mailbox | 2,135,494 | 305,874 | 27 | 7 | 14 | 1,937 | 404 |
| WarpDrc20 transfer_remote | 2,168,469 | 338,768 | 27 | 7 | 15 | 2,351 | 404 |
| WarpDrc20Collateral transfer_remote | 2,191,157 | 361,456 | 30 | 7 | 15 | 2,351 | 404 |
| Mailbox dispatch_default, empty body | 2,015,598 | 193,783 | 16 | 6 | 10 | 1,399 | 305 |
| Mailbox dispatch_default, 64-byte body | 2,069,922 | 243,000 | 16 | 6 | 10 | 1,463 | 369 |
| Mailbox dispatch_default, 1 KiB body | 3,008,012 | 1,104,240 | 16 | 6 | 10 | 2,423 | 1,329 |
| Mailbox dispatch_default, largest body (63,252 bytes) | 62,351,848 | 55,469,407 | 16 | 6 | 10 | 64,651 | 63,557 |
| Mailbox dispatch, 64-byte body, free hook instead of the IGP | 2,052,907 | 223,791 | 14 | 6 | 8 | 1,312 | 401 |
| Mailbox fund_dispatch, new payer | 1,828,801 | 8,781 | 1 | 3 | 3 | 971 | 298 |
| Mailbox fund_dispatch, existing payer | 1,828,689 | 8,669 | 1 | 3 | 3 | 971 | 298 |
| Mailbox withdraw_dispatch_credit | 58,696,331 | 56,862,325 | 1 | 3 | 3 | 1,010 | 477 |
| WarpNative withdraw_dispatch_credit | 58,699,182 | 56,865,239 | 2 | 4 | 3 | 1,010 | 477 |
| WarpDrc20 withdraw_dispatch_credit | 58,699,182 | 56,865,239 | 2 | 4 | 3 | 1,010 | 477 |
| WarpDrc20Collateral withdraw_dispatch_credit | 58,699,737 | 56,865,794 | 2 | 4 | 3 | 1,010 | 477 |
| WarpDrc20 transfer, to a new holder | 1,874,219 | 41,575 | 0 | 2 | 2 | 1,071 | 461 |
| WarpDrc20 transfer, to a holder | 1,874,528 | 41,884 | 0 | 2 | 2 | 1,071 | 461 |
| WarpDrc20 approve, a new spender | 1,873,150 | 40,802 | 0 | 2 | 2 | 1,071 | 460 |
| WarpDrc20 approve, an existing allowance | 1,873,775 | 41,427 | 0 | 2 | 2 | 1,071 | 460 |
| WarpDrc20 transfer_from | 1,910,212 | 61,819 | 0 | 2 | 2 | 1,071 | 658 |
| WarpNative claim_pending | 1,868,377 | 52,717 | 1 | 3 | 4 | 1,030 | 258 |
| WarpNative claim_pending_for | 58,656,559 | 56,824,036 | 1 | 3 | 4 | 1,030 | 462 |
| WarpNative claim_pending_contract, by the red-team contract | 1,843,065 | 21,171 | 3 | 5 | 3 | 813 | 332 |
| WarpDrc20 claim_pending | 1,874,433 | 58,773 | 0 | 2 | 4 | 1,185 | 258 |
| WarpDrc20 claim_pending_for | 58,666,842 | 56,834,319 | 0 | 2 | 4 | 1,185 | 462 |
| WarpDrc20 claim_pending_contract, by the red-team contract | 1,839,357 | 17,463 | 1 | 3 | 3 | 1,135 | 332 |
| WarpDrc20Collateral claim_pending | 1,878,103 | 62,506 | 5 | 3 | 4 | 1,185 | 258 |
| WarpDrc20Collateral claim_pending_for | 58,671,143 | 56,838,620 | 5 | 3 | 4 | 1,185 | 462 |
| WarpDrc20Collateral claim_pending_contract, by the red-team contract | 1,850,433 | 28,539 | 6 | 4 | 3 | 1,135 | 332 |
| IGP pay_for_gas | 1,832,548 | 11,460 | 1 | 3 | 3 | 971 | 312 |
| IGP pay_for_gas, with a refund | 1,847,771 | 26,683 | 2 | 3 | 4 | 1,230 | 312 |

- A transfer makes 27 to 30 inter-contract calls, 7 frames deep: route,
  Mailbox, quotes from every hook, the fee forwarding through the transfer
  contract, and the AggregationHook's payment to the ProtocolFee.
- The first message on a fresh Mailbox costs 4,372 gas more than the next:
  starting the first chunk of every log takes 2 pages in each of the
  Mailbox, the MerkleTreeHook and the IGP. Memory isn't priced by size.
- A dispatch costs about 980 gas per body byte beyond 64.
- Withdrawing credit and sponsored claims decode a public key on chain,
  about 57M gas ([security.md](security.md), LOW-7 and CRITICAL-1).
- `fund_dispatch` takes at least 0.01 DUSK for a payer with no credit,
  any amount for one with some; the rows fund 0.1025 DUSK. A payer with no
  credit is refused once its entry, at most 13 pages, could take the
  Mailbox past 3 GiB.
- A claim registers the account it pays, the only way a route registers
  one: the first transfer to an account waits in escrow until it is
  claimed. Routes have no `register_account`.
- A top-up (`pay_for_gas`, [operations.md](operations.md#topping-up-a-message))
  takes 11,460 gas of its own, 15,223 more when the IGP sends part of the
  deposit back: the deposit's transfer to the IGP, its record and event,
  and the refund. Its deposit, here 10,000 LUX for 100,000 gas at the
  profile's pricing, is on top. `dusk-tx pay-for-gas` defaults to a 3M
  gas limit.

### Relayed deliveries

The relayer's `process`, signed by the last validators of a sorted set of
6, the worst case for the ISM's scan. Tx gas:

| Delivery | 1 signature | 3 signatures | 6 signatures | Calls | Depth | Events | Event bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| WarpNative, payout to a registered account | 2,143,593 | 2,577,970 | 3,231,328 | 5 | 4 | 5 | 1,105 |
| WarpNative, new escrow | 2,128,546 | 2,563,455 | 3,217,485 | 4 | 4 | 4 | 846 |
| WarpNative, existing escrow | 2,128,705 | 2,564,086 | 3,217,318 | 4 | 4 | 4 | 846 |
| WarpDrc20, mint to a registered account | 2,147,878 | 2,583,003 | 3,235,486 | 3 | 3 | 5 | 1,260 |
| WarpDrc20, new escrow | 2,127,163 | 2,561,690 | 3,215,410 | 3 | 3 | 4 | 846 |
| WarpDrc20, existing escrow | 2,127,135 | 2,561,107 | 3,215,383 | 3 | 3 | 4 | 846 |
| WarpDrc20Collateral, payout to a registered account | 2,184,221 | 2,618,193 | 3,272,066 | 9 | 4 | 5 | 1,260 |
| WarpDrc20Collateral, new escrow | 2,130,489 | 2,563,494 | 3,217,514 | 4 | 4 | 4 | 846 |
| WarpDrc20Collateral, existing escrow | 2,129,158 | 2,563,996 | 3,217,119 | 4 | 4 | 4 | 846 |
| A recipient contract, empty body | 2,105,243 | 2,541,527 | 3,192,419 | 3 | 3 | 3 | 774 |

Each signature adds about 218,000 gas: 208,000 in the ISM's check, 4,900
in passing it to the ISM, and 5,200 for the 65 bytes it adds to the
transaction.

| Signatures | Tx bytes | Call gas, WarpNative payout |
|---:|---:|---:|
| 1 | 544 | 304,610 |
| 3 | 672 | 728,747 |
| 6 | 868 | 1,366,425 |

Every delivery to a route fits the relayer's 5M limit with 1.5x headroom
up to 6 signatures, and stays under the least limit `deploy/testnet.sh`
accepts at its signature count. The script's estimate itself, 1.95M plus
0.22M per signature, is below the costliest delivery, a payout from the
collateral route that locks the upstream DRC20, by 14,221 gas at one
signature, 8,193 at three and 2,066 at six; its 1.5x margin covers that.
`process` checks that it is a transaction's own call, which costs about
900 gas a delivery.

A route's messages have a 64-byte body. A message to another recipient
contract can be larger, and each body byte costs about 290 gas, 80 of it in
the signature check:

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Mailbox process, recipient contract, 1 KiB body, 6 of 6 signatures | 3,493,282 | 1,551,638 | 3 | 3 | 3 | 774 | 1,828 |
| Mailbox process, recipient contract, 16 KiB body, 6 of 6 signatures | 7,994,274 | 4,823,830 | 3 | 3 | 3 | 774 | 17,188 |
| Mailbox process, recipient contract, largest body (62,761 bytes), 6 of 6 signatures | 21,583,861 | 14,702,849 | 3 | 3 | 3 | 774 | 63,564 |
| Mailbox process, recipient that is its own ISM | 1,845,867 | 22,370 | 3 | 3 | 3 | 774 | 348 |

At the 5M limit a delivery can carry about 9,900 body bytes at one
signature and 6,200 at six; with the 1.5x headroom, 4,200 and 480. The
generated relayer config relays any message that paid for its delivery,
and sends it with the larger of the 5M limit and the gas it paid for, so a
larger message has to pay for more gas
([Fee coverage](#what-the-relayer-requires)).

### Validators

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| ISM verify, 1 of 6 signatures | 2,075,485 | 241,137 | 0 | 2 | 1 | 657 | 479 |
| ISM verify, 3 of 6 signatures | 2,501,234 | 656,664 | 0 | 2 | 1 | 657 | 607 |
| ISM verify, 6 of 6 signatures | 3,140,433 | 1,280,183 | 0 | 2 | 1 | 657 | 803 |
| ISM verify, 6 of 255 signatures | 3,151,499 | 1,291,308 | 0 | 2 | 1 | 657 | 803 |
| ValidatorAnnounce announce, new validator, 50-byte location | 2,069,678 | 241,540 | 0 | 2 | 2 | 759 | 405 |
| ValidatorAnnounce announce, second location of a validator | 2,070,473 | 241,495 | 0 | 2 | 2 | 759 | 409 |
| ValidatorAnnounce announce, new validator, 1 KiB location | 2,253,080 | 346,419 | 0 | 2 | 2 | 1,731 | 1,381 |

### Owners

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Mailbox renounce_ownership | 1,852,224 | 36,362 | 0 | 2 | 2 | 708 | 263 |
| Mailbox set_default_hook | 1,856,917 | 37,704 | 0 | 2 | 2 | 705 | 293 |
| Mailbox set_default_ism | 1,857,037 | 37,720 | 0 | 2 | 2 | 704 | 292 |
| Mailbox set_required_hook | 1,857,010 | 37,706 | 0 | 2 | 2 | 706 | 294 |
| Mailbox transfer_ownership | 1,857,380 | 37,982 | 0 | 2 | 2 | 742 | 295 |
| ISM set_validators_and_threshold, 255 validators | 2,504,250 | 277,004 | 0 | 2 | 2 | 5,797 | 5,385 |
| ISM set_validators_and_threshold, 6 validators | 1,870,410 | 42,632 | 0 | 2 | 2 | 817 | 405 |
| ISM transfer_ownership | 1,857,047 | 37,649 | 0 | 2 | 2 | 742 | 295 |
| IGP claim | 1,867,137 | 52,557 | 1 | 3 | 2 | 916 | 250 |
| IGP set_beneficiary | 1,856,690 | 37,432 | 0 | 2 | 2 | 704 | 292 |
| IGP set_domain_gas_config | 1,857,078 | 37,457 | 0 | 2 | 2 | 710 | 298 |
| IGP set_domain_gas_configs, 2 domains | 1,862,850 | 40,030 | 0 | 2 | 3 | 763 | 339 |
| IGP transfer_ownership | 1,857,146 | 37,748 | 0 | 2 | 2 | 742 | 295 |
| ProtocolFee claim | 1,865,656 | 51,017 | 1 | 3 | 2 | 916 | 250 |
| ProtocolFee set_beneficiary | 1,855,317 | 36,059 | 0 | 2 | 2 | 704 | 292 |
| ProtocolFee set_protocol_fee | 1,851,244 | 34,576 | 0 | 2 | 2 | 681 | 269 |
| ProtocolFee transfer_ownership | 1,855,773 | 36,375 | 0 | 2 | 2 | 742 | 295 |
| WarpNative enroll_remote_router, enrolled domain | 1,857,017 | 37,111 | 0 | 2 | 2 | 715 | 301 |
| WarpNative enroll_remote_router, new domain | 1,857,000 | 37,094 | 0 | 2 | 2 | 715 | 301 |
| WarpNative set_hook | 1,856,136 | 37,477 | 0 | 2 | 2 | 697 | 285 |
| WarpNative set_ism | 1,855,836 | 37,473 | 0 | 2 | 2 | 696 | 284 |
| WarpNative transfer_ownership | 1,857,384 | 37,986 | 0 | 2 | 2 | 742 | 295 |
| WarpDrc20 enroll_remote_router, enrolled domain | 1,857,017 | 37,111 | 0 | 2 | 2 | 715 | 301 |
| WarpDrc20 enroll_remote_router, new domain | 1,857,000 | 37,094 | 0 | 2 | 2 | 715 | 301 |
| WarpDrc20 set_hook | 1,856,136 | 37,477 | 0 | 2 | 2 | 697 | 285 |
| WarpDrc20 set_ism | 1,855,836 | 37,473 | 0 | 2 | 2 | 696 | 284 |
| WarpDrc20 transfer_ownership | 1,857,384 | 37,986 | 0 | 2 | 2 | 742 | 295 |
| WarpDrc20Collateral enroll_remote_router, enrolled domain | 1,857,017 | 37,111 | 0 | 2 | 2 | 715 | 301 |
| WarpDrc20Collateral enroll_remote_router, new domain | 1,857,000 | 37,094 | 0 | 2 | 2 | 715 | 301 |
| WarpDrc20Collateral set_hook | 1,856,136 | 37,477 | 0 | 2 | 2 | 697 | 285 |
| WarpDrc20Collateral set_ism | 1,855,836 | 37,473 | 0 | 2 | 2 | 696 | 284 |
| WarpDrc20Collateral transfer_ownership | 1,857,384 | 37,986 | 0 | 2 | 2 | 742 | 295 |

## Queries

What agents and apps query, on a Mailbox with 2,304 dispatches (so a page
spans two 2,048-ID chunks), 2,304 IGP payment records (two 1,024-record
chunks), 256 announced validators, a validator with all 16 locations, and
an ISM with 255 validators, the most it takes.

| Query | Gas | Reply bytes |
|---|---:|---:|
| Mailbox default_hook | 204 | 32 |
| Mailbox default_ism | 204 | 32 |
| Mailbox delivered | 449 | 1 |
| Mailbox dispatched_block_height | 171 | 8 |
| Mailbox dispatched_id | 264 | 32 |
| Mailbox fee_credit | 287 | 8 |
| Mailbox latest_dispatched_id | 204 | 32 |
| Mailbox local_domain | 83 | 4 |
| Mailbox memory_bytes | 101 | 8 |
| Mailbox nonce | 83 | 4 |
| Mailbox owner | 828 | 33 |
| Mailbox quote_dispatch_for_sender | 48,537 | 8 |
| Mailbox recipient_ism | 965 | 32 |
| Mailbox required_hook | 204 | 32 |
| Mailbox state_version | 82 | 4 |
| MerkleTreeHook count | 83 | 4 |
| MerkleTreeHook hook_type | 82 | 1 |
| MerkleTreeHook inserted_block_height | 171 | 8 |
| MerkleTreeHook latest_checkpoint | 247,169 | 36 |
| MerkleTreeHook mailbox | 204 | 32 |
| MerkleTreeHook message_id_at | 264 | 32 |
| MerkleTreeHook message_ids, 256 across two chunks | 62,201 | 8,200 |
| MerkleTreeHook message_ids, 256 from 0 | 62,101 | 8,200 |
| MerkleTreeHook quote_dispatch | 305 | 8 |
| MerkleTreeHook root | 247,144 | 32 |
| MerkleTreeHook state_version | 82 | 4 |
| MerkleTreeHook tree | 4,866 | 1,028 |
| AggregationHook hook_type | 82 | 1 |
| AggregationHook hooks | 930 | 72 |
| AggregationHook mailbox | 204 | 32 |
| AggregationHook quote_dispatch | 22,422 | 8 |
| AggregationHook state_version | 82 | 4 |
| ProtocolFee beneficiary | 204 | 32 |
| ProtocolFee claimable_fees | 99 | 8 |
| ProtocolFee collected_fees | 99 | 8 |
| ProtocolFee hook_type | 82 | 1 |
| ProtocolFee mailbox | 204 | 32 |
| ProtocolFee max_protocol_fee | 99 | 8 |
| ProtocolFee owner | 828 | 33 |
| ProtocolFee protocol_fee | 99 | 8 |
| ProtocolFee quote_dispatch | 308 | 8 |
| ProtocolFee state_version | 82 | 4 |
| IGP beneficiary | 204 | 32 |
| IGP claimable_fees | 99 | 8 |
| IGP domain_gas_config | 256 | 24 |
| IGP gas_payment_at | 414 | 64 |
| IGP gas_payment_count | 83 | 4 |
| IGP gas_payments, 256 across two chunks | 103,157 | 16,392 |
| IGP gas_payments, 256 from 0 | 103,057 | 16,392 |
| IGP hook_type | 82 | 1 |
| IGP mailbox | 204 | 32 |
| IGP owner | 828 | 33 |
| IGP quote_dispatch | 1,441 | 8 |
| IGP quote_gas_payment | 913 | 8 |
| IGP quote_pay_for_gas | 904 | 8 |
| IGP state_version | 82 | 4 |
| IGP total_gas_payments | 99 | 8 |
| ISM module_type | 82 | 1 |
| ISM owner | 828 | 33 |
| ISM state_version | 82 | 4 |
| ISM threshold | 83 | 1 |
| ISM validators, 255 validators | 160,325 | 5,108 |
| ISM validators, 6 validators | 4,470 | 128 |
| ISM validators_and_threshold, 255 validators | 160,382 | 5,112 |
| ISM validators_and_threshold, 6 validators | 4,527 | 132 |
| ValidatorAnnounce announced_validator_count | 83 | 4 |
| ValidatorAnnounce get_announced_storage_location_hashes, 1 location | 2,505 | 40 |
| ValidatorAnnounce get_announced_storage_location_hashes, 16 locations | 6,392 | 520 |
| ValidatorAnnounce get_announced_validators, 256 | 44,701 | 5,128 |
| ValidatorAnnounce local_domain | 83 | 4 |
| ValidatorAnnounce mailbox | 204 | 32 |
| ValidatorAnnounce state_version | 82 | 4 |
| WarpNative enrolled_router | 878 | 32 |
| WarpNative hook | 204 | 32 |
| WarpNative interchain_security_module | 204 | 32 |
| WarpNative is_registered | 691 | 1 |
| WarpNative mailbox | 204 | 32 |
| WarpNative owner | 828 | 33 |
| WarpNative pending_balance | 287 | 8 |
| WarpNative pending_total | 99 | 8 |
| WarpNative quote_transfer_remote | 56,172 | 8 |
| WarpNative state_version | 82 | 4 |
| WarpDrc20 allowance | 24,668 | 8 |
| WarpDrc20 balance_of | 17,115 | 8 |
| WarpDrc20 decimals | 83 | 1 |
| WarpDrc20 enrolled_router | 878 | 32 |
| WarpDrc20 hook | 204 | 32 |
| WarpDrc20 interchain_security_module | 204 | 32 |
| WarpDrc20 is_registered | 691 | 1 |
| WarpDrc20 mailbox | 204 | 32 |
| WarpDrc20 name | 867 | 24 |
| WarpDrc20 owner | 828 | 33 |
| WarpDrc20 pending_balance | 287 | 8 |
| WarpDrc20 pending_total | 99 | 8 |
| WarpDrc20 quote_transfer_remote | 56,088 | 8 |
| WarpDrc20 state_version | 82 | 4 |
| WarpDrc20 symbol | 746 | 8 |
| WarpDrc20 total_supply | 99 | 8 |
| WarpDrc20Collateral enrolled_router | 878 | 32 |
| WarpDrc20Collateral hook | 204 | 32 |
| WarpDrc20Collateral interchain_security_module | 204 | 32 |
| WarpDrc20Collateral is_registered | 691 | 1 |
| WarpDrc20Collateral mailbox | 204 | 32 |
| WarpDrc20Collateral owner | 828 | 33 |
| WarpDrc20Collateral pending_balance | 287 | 8 |
| WarpDrc20Collateral pending_total | 99 | 8 |
| WarpDrc20Collateral quote_transfer_remote | 56,172 | 8 |
| WarpDrc20Collateral state_version | 82 | 4 |
| WarpDrc20Collateral wrapped_token | 204 | 32 |

- Every reply is at most 16,392 bytes, a page of 256 IGP payment records,
  a quarter of the 64 KiB a reply can take. The largest pages, 256 message
  IDs, 256 validators and 255 ISM validators, take 8,200, 5,128 and 5,112.
- The costliest query is `root` or `latest_checkpoint`, 247,169 gas: the
  hook computes the root on request. The relayer's simulation of `process`
  costs the call gas of the delivery it simulates, at most 1.41M. Both are
  far below the 3e9 a query may spend.
- Paged queries cost the same in any chunk: a page that spans two chunks
  costs 100 gas more. Lookups in maps (`delivered`, `fee_credit`,
  `is_registered`, balances) grow with the logarithm of the map's size.

## Memory per entry

What each permissionless path adds to a contract's memory, measured over
32,768 entries (4,096 for credit, top-ups and registrations; memory grows
in 64 KiB pages, so that resolves 2 and 16 bytes an entry). Gas per entry is
the marginal gas of one more call from a contract that makes many in one
transaction, or a whole transaction where each needs its own. Deliveries
and announcements are a transaction's own calls, and an approval takes a
transaction or a contract per 64: for those the profile measures the gas
of transactions and makes the rest of the entries with direct calls,
which run the same code and leave the same memory behind.

| Path | Contract | Bytes per entry | Gas per entry |
|---|---|---:|---:|
| Dispatch through the deployed hooks | Mailbox | 32 | 179,389 marginal |
|  | MerkleTreeHook | 32 |  |
|  | IGP | 64 |  |
| | ProtocolFee, AggregationHook | 0 | |
| Delivery to a recipient that is its own ISM | Mailbox | 26 | 1,844,727 per tx |
| New validator, 1-byte location | ValidatorAnnounce | 108 | 2,061,782 per tx |
| New validator, 1 KiB location | ValidatorAnnounce | 110 | 2,258,814 per tx |
| WarpDrc20 transfer to a new holder | WarpDrc20 | 62 | 56,161 marginal |
| WarpDrc20 approval of a new spender, 64 per owner | WarpDrc20 | 62 | 1,877,820 per tx |
| WarpNative escrow for a new recipient | WarpNative | 72 | 2,130,676 per tx |
|  | Mailbox | 26 |  |
| WarpDrc20 escrow for a new recipient | WarpDrc20 | 72 | 2,131,031 per tx |
|  | Mailbox | 26 |  |
| WarpDrc20Collateral escrow for a new recipient | WarpDrc20Collateral | 72 | 2,133,125 per tx |
|  | Mailbox | 26 |  |
| Mailbox credit for a new payer, 0.01 DUSK each | Mailbox | 64 | 1,831,142 per tx |
| IGP top-up (`pay_for_gas`) for any message ID | IGP | 64 | 1,832,296 per tx |
| WarpNative registration of a new account, by its claim | WarpNative | 352 | 1,875,811 per claim |
| WarpDrc20 registration of a new account, by its claim | WarpDrc20 | 416 | 1,885,159 per claim |
| WarpDrc20Collateral registration of a new account, by its claim | WarpDrc20Collateral | 352 | 1,900,010 per claim |

- Escrow is delivered as the relayer delivers it, signed at a threshold of
  1; its gas is the relayer's `process`. An escrow delivery also adds the
  Mailbox's 26 bytes for the delivery.
- A registration takes an escrow delivery, a funding transfer for a new
  key and the key's claim; the gas column is the claim's. A claim on
  WarpDrc20 also mints, so its 416 bytes are a registration and a balance,
  a new bridged account; the collateral route's claim adds a balance to
  the locked token instead.
- An approval's 62 bytes include the owner's own map of spenders, 64 to an
  owner (512 owners here). Its gas is a Moonlight owner's transaction: a
  contract owner would cost a deployment, at least 5M gas at 2,000 LUX
  (10 DUSK), per 64 approvals.
- The bytes per entry are for keys in random order. Keys inserted in
  increasing order, which an attacker can arrange, leave the B-trees'
  nodes about 55% full and take about 25% more ([storage.md](storage.md#key-order)).
- The announcement stores the location's hash, so the location's length only
  changes the gas.
- Apart from these paths, a single transaction in the profile grew a
  contract's memory in two cases. The first entry of a chunked log takes
  its whole 64 KiB chunk and the chunk table: 2 pages in each of the
  Mailbox, the MerkleTreeHook and the IGP on the first message, and in
  ValidatorAnnounce on the first announcement. And a call with about 63 KB
  of arguments raises the memory of each contract it passes them to: 2
  pages in the Mailbox and 1 in the ISM for the largest delivery, 2 in the
  Mailbox and 1 in each hook for the largest dispatch. That memory is free
  again after the call and reused, so it happens once per contract, and no
  call's arguments can take more than 64 KiB.

## Fee coverage

What a message pays for its delivery, against what delivering it costs
the relayer, and how the relayer checks it was paid. The pricing is
`deploy/testnet.env.example`'s; `bash deploy/testnet.sh fees` prints what
any settings charge, and `tests/tests/fee_pricing.rs` checks the Dusk IGP's
quote for them against the formula below.

### Pricing

Each side has an IGP of this deployment that charges the sender, in the
origin chain's token, for a gas amount on the destination:

```
payment = gas x gas price x token exchange rate / 1e10
```

The gas is the message's gas limit, from its hook metadata, plus the
destination's overhead. The gas price is in the destination's smallest
unit, and the exchange rate converts that into the origin's, scaled by
1e10. One ETH is 1e18 wei and buys `ETH_DUSK_PRICE_RATIO` DUSK of 1e9 LUX
each, so a wei is worth `ETH_DUSK_PRICE_RATIO` x 1e-9 LUX. Neither IGP has
a markup field, so the operator's margin, `OPERATOR_MARGIN_BPS` / 10,000,
goes into the exchange rate, rounded up:

| | Dusk IGP, on Dusk | EVM IGP, on Sepolia |
|---|---|---|
| Charges for delivery to | the EVM chain | Dusk |
| Payment in | LUX | wei |
| Gas price | `EVM_GAS_PRICE_WEI`, wei | `DUSK_AGENT_GAS_PRICE`, LUX |
| Token exchange rate | 10 x `ETH_DUSK_PRICE_RATIO` x (1 + margin) | 1e19 / `ETH_DUSK_PRICE_RATIO` x (1 + margin) |
| Gas a warp transfer pays for | 50,000 (no hook metadata) + `DUSK_IGP_GAS_OVERHEAD` | 0 (the route's destination gas) + `EVM_IGP_GAS_OVERHEAD` |

The Dusk routes send no hook metadata, which the Dusk IGP reads as 50,000
gas, and the Sepolia route asks for no gas of its own, so a transfer pays
for the overhead plus 50,000 one way and for the overhead the other. The
overheads cover the whole delivery:

- **`DUSK_IGP_GAS_OVERHEAD`**, 110,000: Mailbox `process` on the EVM chain
  with m-of-n MessageId multisig verification and the token's `handle`,
  less the 50,000. The local rehearsal measured a delivery at 140,569 gas
  with 2 of 3 signatures, through a StorageMessageIdMultisigIsm, minting to
  a new holder (the relayer's estimate was 141,343). `deploy/testnet.sh`
  takes 125,000 + 7,500 per signature + 2,500 per validator as the most a
  delivery takes, 147,500 at 2 of 3, and `preflight` stops when the
  overhead doesn't cover it at `DUSK_ORIGIN_THRESHOLD` of the
  `DUSK_ORIGIN_VALIDATORS`. 160,000 covers up to 3 of 5. Hyperlane's own
  overheads, from `multisigIsmVerifyCosts` in its SDK (159,736 gas at 2 of
  3, before the handle), are measured on its default ISM, an aggregation
  of routing, multisig and pausable ISMs, which costs more than the single
  MessageIdMultisig ISM the token uses.
- **`EVM_IGP_GAS_OVERHEAD`**, 2,170,000: a relayed `process` on Dusk,
  1,950,000 plus 220,000 per signature at `EVM_ORIGIN_THRESHOLD`, the
  route's `handle` included ([Relayed deliveries](#relayed-deliveries)).
  `preflight` stops below that, or above `DUSK_RELAYER_GAS_LIMIT`.

### What the defaults charge

Assumptions: 1 ETH = 25,000 DUSK (ETH at 2,500 USD, DUSK at 0.10 USD), EVM
gas at 1 gwei, Dusk gas at 1 LUX, a 5,000 bps (50%) margin. The ratio and
the gas price are the operator's to keep current
([operations.md](operations.md#relaying-fees)).

| | Dusk to Sepolia | Sepolia to Dusk |
|---|---|---|
| IGP pricing | overhead 110,000, rate 375,000, gas price 1e9 wei | overhead 2,170,000, rate 6e14, gas price 1 LUX |
| Gas paid for | 160,000 EVM gas | 2,170,000 Dusk gas |
| IGP fee | 6 DUSK | 1.302e11 wei (0.0000001302 ETH) |
| Other fee | 0.001 DUSK protocol fee, to the Dusk ProtocolFee's beneficiary | 1 wei, Hyperlane's own protocol fee on Sepolia |
| Delivery cost | 140,569 EVM gas at 2 of 3, measured: 0.00014 ETH, 3.5 DUSK | 2,143,593 Dusk gas for a payout, 2,128,546 for a new escrow, at threshold 1: 0.0021 DUSK, 8.6e10 wei |
| Covered | 1.71 times at 2 of 3; at least 1.5 times up to 3 of 5 | 1.52 times |
| In USD | 0.60 charged for 0.35 of gas | 0.00033 charged for 0.00021 |

The local rehearsal (`deploy/local-fees-check.sh`, Anvil standing in for
Sepolia, 2 of 3 Dusk-origin validators) measured both directions: a
transfer from Dusk paid 6 DUSK to the Dusk IGP (and 0.001 to the
ProtocolFee) and its delivery on Anvil took 140,569 gas minting to a new
holder, 106,193 to one that already held tokens; a transfer from
Anvil paid 130,200,000,000 wei to the EVM IGP and its delivery on Dusk took
2,141,372 gas, against the 2,170,000 paid for. A short message from Dusk to
a TestRecipient took 113,834 gas to deliver, and a message to Dusk's
TestRecipient 2,105,513. A 2 KiB message from Dusk paid for 160,000 gas at
dispatch, against the relayer's estimate of 1,616,077 for its delivery. It
wasn't delivered once a `dusk-tx pay-for-gas` top-up had brought that to
618,692, and was 181 seconds after a second one brought it to 1,169,123,
past two-thirds of the estimate; its delivery took 1,547,346 gas. The
top-ups paid a dispatch's 37,500 LUX per gas, 37.84 DUSK for 1,009,123 gas,
and the Dusk IGP's fees to claim came to exactly that plus the three
dispatches' 18 DUSK.

On testnet the ETH is Sepolia ETH, and the relayer pays Sepolia's gas
price, which can be far below or above 1 gwei; the pricing is set as it
would be on mainnet.

### What the relayer requires

The relayer config `deploy/testnet.sh agent-configs` writes relays any
message between Dusk and the EVM chain, whoever sent it and whatever the
recipient, once it is paid for. It has no whitelist, and its
`gasPaymentEnforcement` has one `onChainFeeQuoting` policy per origin
domain. The relayer reads payments only from this deployment's IGPs; the
EVM chain's `interchainGasPaymaster` in it is ours, not Hyperlane's. A message
without a payment there isn't relayed, nor is a message from any other
origin.

`onChainFeeQuoting` compares gas, not money. It holds the gas a message
paid for, less what the relayer has already spent on it, against a
fraction of the relayer's own estimate of its delivery, and leaves it to
the IGP's price to make that gas worth what it costs:

- **From Dusk**, the estimate is `eth_estimateGas` of the `process` call, so
  it follows each message, and a recipient whose `handle` costs more needs
  more gas paid for. The fraction is 1 / (1 + margin), `2/3` by default,
  so what was paid for, at the IGP's price with the margin, covers the
  estimate at that price without it. A warp transfer pays for 160,000 gas,
  enough for estimates up to 240,000; the rehearsal's was 141,343.
- **From the EVM chain**, `hyperlane-dusk` estimates every delivery at the
  relayer's gas limit, `DUSK_RELAYER_GAS_LIMIT`, because a Rusk query
  measures no gas. The fraction is `EVM_IGP_GAS_OVERHEAD` /
  `DUSK_RELAYER_GAS_LIMIT`, `217/500`, so a message must have paid for the
  overhead, 2,170,000 Dusk gas, as a warp transfer does.

A message that passes is sent with the larger of the estimate and the gas
it paid for as its gas limit. Anyone can pay more for a message at any
time, on either side: on the EVM IGP with `payForGas`, and on the Dusk IGP
with `pay_for_gas`, for the message's ID and destination
([operations.md](operations.md#topping-up-a-message)). Payments for one
message add up, and the relayer checks a message that hasn't paid enough
again later, less often the longer it waits.

`minimum`, the other policy that checks payments, compares a payment's
value with a fixed amount of the origin's token. It would have to change
with every price update, and can't tell a message whose `handle` costs ten
times more. `none` relays everything, which is what anyone could make the
relayer pay for before.

### What stays uncovered

- **Prices move.** The IGPs charge what they were last set to. If EVM gas
  or the ETH/DUSK price moves against the operator by more than the
  margin, Dusk-origin deliveries cost more than they pay until
  `deploy/testnet.sh dusk-igp --apply` updates the price. Hyperlane runs a
  service that updates its own oracles; here it is a manual step.
- **Dusk-bound messages that cost more than the overhead.** The relayer
  can't estimate a Dusk delivery, so a message whose `handle` takes more
  than the 2.17M gas it paid for is still delivered, with the relayer's 5M
  limit. It costs the relayer up to 0.005 DUSK, of which the payment covers
  0.0033 at the default margin.
- **Deliveries that fail.** One that passes the simulation and then fails
  (a recipient that tells the simulation from the transaction, or someone
  delivering first) pays the whole 5M limit, 0.005 DUSK, against the
  same 0.0033 paid. The relayer records the gas it used, and doesn't try
  that message again until more is paid for it.

## Attack costs

What it costs to add a byte of permanent state through each permissionless
path, at 1 LUX per gas, with the fees each entry pays. Deliveries,
announcements, credit, top-ups and approvals take a transaction per entry,
since no contract can make them in bulk ([storage.md](storage.md#who-can-add-entries));
dispatches and transfers to new holders can be batched from a contract.
The IGP's fees are at the profile's token-sized pricing unless the row
says otherwise: at the pricing `deploy/testnet.env.example` configures, a
dispatch through the IGP to the EVM chain pays 6.001 DUSK.

| Path | Contract | Bytes per entry | Gas per entry | Fees per entry | DUSK per MiB |
|---|---|---:|---|---|---:|
| Delivery to a recipient that is its own ISM | Mailbox | 26 | 1,845,867, a transaction | none | 74.4 |
| Dispatch with a free hook of the sender's | Mailbox | 32 | 154,582, batched | the protocol fee, 0.001 DUSK | 37.8 |
| Dispatch through the IGP | IGP | 64 | 179,389, batched | 0.001025 DUSK; 6.001 at the testnet pricing | 19.7; 98,323 at the testnet pricing |
| | MerkleTreeHook | 32 | | | not reachable, see below |
| Credit for a new payer | Mailbox | 64, and 32 for its dispatch | 1,831,142, a transaction, and a dispatch of about 2.0M | the protocol fee; the 0.01 DUSK minimum comes back | 52.8 to 72.8 |
| Top-up for any message ID | IGP | 64 | 1,832,296, a transaction | at least one gas's price, 37,500 LUX at the testnet pricing | 30.6 |
| New validator, 1-byte location | ValidatorAnnounce | 108 | 2,061,782, a transaction | none | 20.0 |
| Transfer to a new holder | WarpDrc20 | 62 | 56,161, batched | a token unit | 0.9 |
| Approval of a new spender | WarpDrc20 | 62 | 1,877,820, a transaction, and a funding transfer per 64 | none | 32.2 |
| Escrow for a new recipient | a route | 72 | 2,130,676, the relayer's `process` | an EVM transfer and its IGP fee | 31.0 |
| Registration of a new account | a route | 352 | 2,130,676 for the escrow's delivery, 1,875,811 for the claim, and a funding transfer, about 1.83M | an EVM transfer and its IGP fee | 11.9 to 17.4 |

- Each dispatch's entry lands in three contracts, and each fills on its
  own: the Mailbox at 32 bytes, the MerkleTreeHook at 32 and the IGP at
  64. The Mailbox closes dispatch at 3 GiB, before the MerkleTreeHook is
  full, and the cheaper way into the Mailbox's memory is a free hook of the
  sender's own instead of the IGP.
- An approval takes a transaction by a Moonlight owner, which holds at most
  64 allowances, so 64 approvals take a funded key; a contract owner takes
  a deployment, at least 10 DUSK, per 64.
- Escrow and registrations are paid for by the relayer, on Dusk, through
  its `process` at one signature, and by the claimant. The attacker pays an
  EVM transfer per entry, and the EVM IGP for the relayer's gas, without
  which the relayer doesn't deliver it.
- Credit for a new payer takes at least 0.01 DUSK, but it isn't locked:
  one dispatch through a hook the payer controls spends all of it but a
  LUX, the protocol fee to the ProtocolFee and the rest back to the
  payer's hook, and the entry stays. So a new entry costs the funding
  transaction, that dispatch, gas money for the payer's key and the
  protocol fee: 0.0049 to 0.0067 DUSK, and 96 to 112 bytes with the
  dispatch's. New payers stop at 3 GiB, as dispatch does, so credit can't
  reach the last GiB.

What filling each contract costs, with each way in. The ranges are keys
in increasing order, which an attacker can arrange and which take about
25% more memory per entry, to keys in random order
([storage.md](storage.md#key-order)); chunked logs don't depend on the
order.

| Contract | Path | Entries | DUSK | Notes |
|---|---|---:|---|---|
| Mailbox, dispatch closed at 3 GiB | dispatches with a free hook | about 100M | about 116,000: 15,500 of gas and 100,700 of protocol fees | 1.02M at a 0.01 DUSK protocol fee |
| Mailbox, dispatch closed at 3 GiB | deliveries to a recipient that is its own ISM | 97M to 122M | 179,000 to 225,000 | 11 to 14 days of full blocks |
| Mailbox, dispatch closed at 3 GiB | credit for new payers, each with a dispatch that takes the minimum back | 29M to 34M | 140,000 to 225,000 | still more than dispatches alone |
| Mailbox, full at 4 GiB | dispatches, then deliveries for the last GiB | 33M to 41M deliveries | 177,000 to 192,000 | 116,000 of it the dispatches; the deliveries take 3.8 to 4.8 days |
| Mailbox, full at 4 GiB | deliveries alone | 130M to 163M | 239,000 to 301,000 | 15 to 19 days |
| MerkleTreeHook | dispatches | not reachable |  | the Mailbox closes dispatch first |
| IGP, top-ups closed at 3 GiB | top-ups for any message ID | 50M | about 94,000: 92,200 of gas and 1,900 of payments at the testnet pricing | dispatches through the IGP still pay |
| IGP, full at 4 GiB | dispatches through the IGP | 67M | about 403 million at the testnet pricing; 12,000 of gas and 68,800 of fees at the profile's | each pays the protocol fee too |
| ValidatorAnnounce, full at 4 GiB | new validators, 1-byte locations | 36M to 40M | 74,000 to 82,000 | 4.1 to 4.6 days |
| WarpDrc20, new holders closed at 3 GiB | transfers to new holders | 42M to 52M | 2,300 to 2,900, plus a token unit each | then nobody can send to a new address; the synthetic route keeps the last GiB |
| WarpDrc20, new allowances closed at 3 GiB | approvals of new spenders | 42M to 52M | 79,000 to 99,000 | 650,000 to 810,000 funded keys; 4.8 to 6.0 days |
| A route, full at 4 GiB | escrow for new recipients | 48M to 60M | 101,000 to 127,000, the relayer's gas | an EVM transfer each: 6.2 to 7.8 ETH of IGP fees at the default pricing |
| A route, full at 4 GiB | registrations by claim | 9.8M to 12.2M | 39,000 to 71,000 of Dusk gas | an EVM transfer each: 1.3 to 1.6 ETH of IGP fees; WarpDrc20's claims also mint, 8.3M to 10.3M |
| ProtocolFee, AggregationHook, ISM | none |  |  | no permissionless state |

- ProtocolFee and AggregationHook have no state anyone can add to: neither
  grew over 32,768 dispatches. The ISM's state is its owner's validator
  set.
- The cheapest damage is to a WarpDrc20's new holders, and it is
  contained: past 3 GiB, transfers and approvals to new addresses fail,
  but transfers between holders and the synthetic route's mints, escrow
  and claims go on in the last GiB, which holds 2.1M to 2.6M new bridged
  accounts at a registration and a balance each
  ([storage.md](storage.md#the-warpdrc20-reserve)). A collateral route
  over the token would stop paying new accounts there, since it pays with
  the token's `transfer`; wrapping a WarpDrc20 is unsupported, and
  `dusk-tx deploy-hyperlane` refuses it. Pricing new holders of a token
  whose units are free takes a dust threshold or a price per byte from
  Rusk, later stages of the plan.
- The cheapest way to make `process` fail for good, which would mean
  replacing the Mailbox and every route on it, is to close dispatch with
  dispatches and then deliver for the last GiB: about 177,000 DUSK and 4
  days of full blocks for the deliveries. Raising the testnet protocol fee
  to 0.01 DUSK, which the owner can do without a redeploy, takes the
  dispatch part to 1.02M.
- Top-ups stop when the IGP's memory would pass 3 GiB (`IGP: storage full,
  top-ups are closed`), so the last GiB stays for the payments dispatches
  make. At the profile's pricing, dispatches are the cheaper way to fill
  the IGP. At the testnet pricing, top-ups are, and they only close
  top-ups: stuck messages can no longer be topped up. Making dispatches
  through the IGP revert then still takes the last GiB in dispatches,
  16.8M of them: about 20,200 DUSK at the profile's pricing, and 101
  million DUSK of IGP fees at the testnet pricing. The owner can then point
  the Mailbox's default hook at a new IGP.
- A route's escrow and registry grow only through deliveries from its
  enrolled router, each a transfer someone sent from the EVM chain and
  paid the EVM IGP for, and the relayer pays for each delivery on Dusk.
  The IGP fees pay the relayer's gas back about 1.5 times.

## Throughput

A block holds at most 3e9 gas, 1,000 transactions, and 1 MiB with its
header (the size column leaves the header out), and one comes at most
every 10 seconds. Block gas counts what transactions spend, not their
limits.

| Transaction | Tx gas | Tx bytes | Per block by gas | By size | By count | Per block |
|---|---:|---:|---:|---:|---:|---:|
| Relayed delivery, 1 signature | 2,143,593 | 544 | 1,399 | 1,882 | 1,000 | 1,000 |
| Relayed delivery, 6 signatures | 3,231,328 | 868 | 928 | 1,190 | 1,000 | 928 |
| Relayed delivery to a collateral route, 6 signatures | 3,272,066 | 868 | 916 | 1,190 | 1,000 | 916 |
| WarpNative transfer_remote | 2,131,122 | 404 | 1,407 | 2,514 | 1,000 | 1,000 |
| WarpDrc20Collateral transfer_remote | 2,191,157 | 404 | 1,369 | 2,514 | 1,000 | 1,000 |
| Dispatch from an account | 2,015,598 | 305 | 1,488 | 3,297 | 1,000 | 1,000 |

- Relayed deliveries are capped by the 1,000-transaction limit at one
  signature (gas would allow 1,399) and by gas at six: 928 to the native
  route, 916 to a collateral route. That is 100 or about 92 messages a
  second into Dusk.
- Transfers out of Dusk are capped at 1,000 a block by the transaction
  limit, 100 a second.
- From a contract, one transaction can make many dispatches, so gas is the
  only limit: 16,723 per block. Deliveries and announcements are a
  transaction's own calls, so a block holds at most 1,000 of each,
  whoever sends them. Closing the Mailbox's dispatch with deliveries, 97M
  to 122M of them, takes 97,000 to 122,000 full blocks, 11 to 14 days; it
  took 4 hours when one contract could make 85,401 in a block.

## Agents: API usage, latency and resources

What the relayer and the validators cost a Rusk node: the requests each
agent sends, at idle and per message, how long each stage of a message
takes, and the agents' CPU, memory, disk and log use. Measured on a local
rehearsal of the testnet deployment, with every agent's traffic going
through a logging proxy, twice: on 2026-11-15 with the agents at
`1733ab9120` (the previous run), and on 2026-11-16 with the agents at
`0d8af2dda2`, the heads of `testnet/dusk-agents` for the testnet launch
(this run), plus a shorter run of the same agents without the multicall.
[demo/bench/](../demo/bench/README.md) has the tooling and how to run it.

### What changed in the agents

Between the two runs, the agents' Dusk support changed in five ways a node
sees:

- **Pacing.** Every client of one endpoint in an agent process shares one
  pace for contract queries: 100 a second (`contractQueriesPerSecond`),
  bursts of 20 (`contractQueryBurst`), at most 8 in flight. A request the
  node refuses for its rate limit holds back its class for the
  `Retry-After` plus up to as long again at random, and goes again after a
  wait that doubles, also at random, so indexers don't come back together.
- **Shared reads per tick.** One `lastBlockPair` read per 5 s tick serves
  every indexer of a process, and what the agent computed from final data
  (record counts, the heights of final records, checkpoints, delivery
  checks) is kept until the node's last final block changes. An indexer
  looks up only the records past the last final one it knows, so idle
  requests no longer grow with history.
- **Multicall.** With `batchContractAddress` set to Multicall for Dusk,
  reads that don't depend on each other go as one `aggregate` query: the
  counts and delivery checks of each final block, a range's heights, IDs
  and pages, a checkpoint's checks, and the deployment check at startup.
  Record heights are searched 256 at a time.
- **Pipelined deliveries.** With `maxBatchSize` 16, the relayer sends the
  deliveries to Dusk it has ready as consecutive transactions of its
  account (`dusk-tx call --nonce N --no-wait`), each once the node holds
  the one before. Each delivery owns its nonce and transaction, kept in
  `eventCursorDir` until it settles, so a restarted relayer follows the
  deliveries in flight instead of sending them again. Since `d549a2fc73`
  the agents pipeline only with `pipelineDeliveries: true` as well, and the
  configs `agent-configs` writes have neither: pipelining is experimental,
  and deliveries go one at a time
  ([operations.md](operations.md#deliveries-to-dusk)).
- **Finality-only confirmation.** The Mailbox reports a delivery as done
  only once its `ProcessId` event is in a final block.

Also: a restarted relayer's indexers start above the records their
database holds instead of reading the history again, a delivery runs one
`process` simulation instead of three, and each endpoint's deployment
check runs once per process.

### Setup

- One local Rusk 1.7.3 node built with `archive` (the commit in
  `RUSK_VERSION`), with a block every 10 s, each final when the next one is
  accepted. Anvil 1.2.3, mining blocks as transactions arrive.
- The local rehearsal of the testnet deployment
  ([testnet.md](testnet.md#rehearse-locally)), deployed with
  `deploy/testnet.sh`: the native DUSK route and MessageIdMultisig ISMs on
  both sides.
- A relayer, three Dusk-origin validators (2 of 3) and one EVM-origin
  validator (1 of 1, standing in for Hyperlane's Sepolia validator), debug
  builds, with the configs `deploy/testnet.sh agent-configs` writes, run as
  `deploy/local-agents.sh` runs them. Validators run with `interval: 5`.
  Default log level (info), pretty format.
- Each agent reaches Rusk and Anvil through its own port of
  `demo/bench/proxy.py`, which logs every request and the reads inside
  each `aggregate`. The agents run `dusk-tx` through a shim that logs each
  run and tags its requests, so they count for the agent that ran it.
- Transfers as in [testnet.md](testnet.md#check-it-works):
  `dusk-tx transfer-remote --native` from the deployer's account, and
  `transferRemote` on the EVM token from Anvil's account #0, paying what
  the route quotes. They share the Dusk account with the agents, so Dusk
  to EVM transfers ran while the relayer had nothing to deliver to Dusk.

| | Previous run | This run | Without the multicall |
|---|---|---|---|
| Agents (`hyperlane-monorepo`) | `1733ab9120` | `0d8af2dda2` | `0d8af2dda2` |
| This repository | `39fe282` | `d90fa5b` | `d90fa5b` |
| `batchContractAddress` | none (no multicall on the local chain) | Multicall for Dusk v0.1.0 | none |
| Relayer `maxBatchSize` | 1 | 16 | 16 |
| Gas payments | not enforced, route only | enforced (`onChainFeeQuoting`) | enforced |
| Relayer paced at the proxy | 150, then 100 a second, from 10.5 minutes into its first restart until the outage | no | no |
| Phases | all | all | baseline, idle, Dusk to EVM, backlog, idle after |

| Phase | Previous, minutes | This run, minutes | What happened |
|---|---:|---:|---|
| Baseline | 3.0 | 3.0 | Node and Anvil, no agents |
| Idle | 11.0 | 11.0 | All five agents, no messages yet |
| Dusk to EVM | 20.3 | 20.3 | 60 transfers, each sent when the last was in a block |
| Backlog | 35.2 | 13.8 | Relayer stopped while 40 more were sent, then restarted |
| EVM to Dusk | 25.0 | 25.0 | 60 transfers, one every 25 s |
| Burst | 25.6 | 1.6 | 40 transfers at once, with a relayer restart while it delivered them |
| Outage | 3.7 | 3.0 | The agents' Rusk ports refused connections for 90 s while 6 transfers were sent |
| Idle after | 8.0 | 10.0 | No traffic, with 100 Dusk and 106 EVM messages in |

In the previous run's burst, 39 of the 40 transfers ran out of gas on Anvil
and were sent again 12.5 minutes later, and its relayer restart came while
nothing was waiting. In this run the burst's transfers carried a fixed gas
limit, and the relayer was stopped about 2 s after it sent the first of
their deliveries to Dusk, with the rest still to send. The host's load
averaged 1.2 (on 22 cores) during this run.

### Idle

Requests a minute to Rusk, on a new deployment and after the load (the
previous run after 100 messages, this run after 100 Dusk and 106 EVM
messages, the run without the multicall after 100 Dusk messages):

| Agent | Previous: new | after | This run: new | after | Without the multicall: new | after |
|---|---:|---:|---:|---:|---:|---:|
| Relayer | 75 | 305 | 21 | 33 | 33 | 33 |
| Each Dusk validator | 26 | 109 | 20 | 20 | 20 | 20 |
| All four Dusk agents | 153 | 632 | 81 | 93 | 93 | 93 |
| KiB a minute, all four | 58.4 | 218.1 | 35.1 | 43.0 | 36.6 | 36.7 |

On Anvil the relayer sent 40 to 50 requests a minute and the EVM validator
25 to 27, as before. Serving the Dusk agents took 0.29% of one node thread
on a new deployment and 0.42% after the load, against 0.4% and 2.4%.

Requests a minute by kind, this run:

| Request | Relayer, new | Relayer, after | Each Dusk validator, new | after |
|---|---:|---:|---:|---:|
| `lastBlockPair` (GraphQL) | 12 | 12 | 12 | 12 |
| Multicall `aggregate` | 6 | 6.5 | | |
| MerkleTreeHook `count` | | | 6 | 6 |
| Delivery confirmations: `checkBlock`, `finalizedEvents`, `delivered` | | 12 | | |
| Metrics: `blocks(last: 1)`, the gas price, the relayer's balance | 3 | 3 | 2 | 2 |

- One `lastBlockPair` per 5 s tick serves all of an agent's indexers, 12 a
  minute, where the relayer's three indexers read 36.
- Per newly final block (6 a minute) the relayer's one aggregate carries
  the Mailbox's `nonce`, the hook's `count` and the IGP's
  `gas_payment_count`, and the delivery checks due; a validator reads the
  hook's `count` alone, since an aggregate of one read gains nothing.
  Nothing else is read while no record is new.
- After the load, the relayer's 12 more a minute were delivery
  confirmations: Hyperlane checks a delivery again 10 minutes after
  submitting it, and the burst's and the outage's came due in that phase
  (101 `delivered` reads in one minute). In the minutes without any it sent
  21 to 25. The validators' rate didn't change with history.

### Per message

What each message added over the idle rate: Dusk to EVM over the rate of a
new deployment, EVM to Dusk over the rate after the load, which has the
same Dusk history. The previous run's Dusk to EVM figures over its rate
after 100 messages are in brackets: its idle rate grew with history, this
run's barely does.

| Agent | Dusk to EVM: Rusk req, previous | this run | Rusk KiB, previous | this run | Anvil req, previous | this run |
|---|---:|---:|---:|---:|---:|---:|
| Relayer | 128 (49) | 24.1 | 49 (22) | 18.8 | 24 to 28 | 28.2 |
| Each Dusk validator | 44 (16) | 6.0 | 16 (7) | 4.7 | | |
| The sender | 17 | 16.6 | 6.7 | 6.7 | | |

| Agent | EVM to Dusk: Rusk req, previous | this run | Rusk KiB, previous | this run | Anvil req, previous | this run |
|---|---:|---:|---:|---:|---:|---:|
| Relayer | 25 | 18.7 | 14.3 | 11.8 | 5.5 | 5.5 |
| Each Dusk validator | 0 | 0 | 0 | 0 | | |
| EVM validator | | | | | 3.2 | 3.2 |
| The sender | | | | | 8.0 | 8.4 |

For a Dusk to EVM message the relayer's 24 requests to Rusk were 7.1
`checkBlock` and 3.2 `finalizedEvents` pages (matching the records to their
transactions in the archive), 4.0 aggregates, 2.0 each of the IGP's
`gas_payment_at`, the Mailbox's `dispatched_block_height` and the hook's
`inserted_block_height` read alone, 2.0 `tx` reads and 1.9 IGP
`gas_payments`. The aggregates carried 29 `gas_payment_at` reads a message
on average (see What to change). Each validator's 6 were 2 aggregates (a
checkpoint's checks, `count`, `root`, `tree`, `message_id_at` and pages of
`message_ids`, and record heights), 2 `inserted_block_height`, a
`finalizedEvents` page and a `checkBlock`. Before, the binary searches over
every record's height were 94 of the relayer's 128 and 31 of each
validator's 44.

For an EVM to Dusk message, the relayer's 19 requests to Rusk were:

| Request | Previous | This run | Why |
|---|---:|---:|---|
| Mailbox `delivered` | 9.6 | 0.9 | The 0.5 s signature wait reads the answer kept for the last final block; new checks ride in the aggregate each final block sends anyway (3.8 reads a message) |
| `tx` | 6.0 | 9.3 | Following the delivery to its block: dusk-tx polled every 3 s, the relayer now polls itself (about every 1.7 s) |
| `dusk-tx`: `chain_id`, `account`, `preverify`, `propagate`, `block` | 5.0 | 3.0 | With `--no-wait` the helper only checks the chain, preverifies and propagates |
| `mempoolTx`, `account` status | | 2.9 | The batch's checks that the node holds the transaction before, and the account's next nonce |
| Mailbox `process`, as a query | 3.0 | 1.0 | One simulation where there were three; 3.1 a message in the burst, whose deliveries waited for earlier batches |
| ISM `validators_and_threshold`, the recipient's ISM, its type | 1.3 | 1.6 | |

### What the node spends its time on

All agents together:

| | Idle, new | Dusk to EVM | EVM to Dusk | Burst | Idle after the load |
|---|---:|---:|---:|---:|---:|
| Requests a minute, previous | 153 | 918 | 690 | | 632 |
| Requests a minute, this run | 81 | 205 | 138 | 479 | 93 |
| One node thread busy, previous | 0.4% | 2.9% | 2.8% | | 2.4% |
| One node thread busy, this run | 0.29% | 0.84% | 0.55% | 3.6% | 0.42% |
| Rusk CPU, one core, previous | 1.2% | 3.7% | 3.2% | | 2.7% |
| Rusk CPU, one core, this run | 1.4% | 2.6% | 1.8% | 5.2% | 1.6% |

"Busy" is the time Rusk took to answer, summed. Rusk used 1.1% of a core
with no agents in both runs; its CPU includes the transfers' own
transactions and the consensus work of a 10 s chain. At idle this run,
`lastBlockPair` was 59% of the requests and 47% of the node's time, the
validators' Merkle `count` 22% and 23%, the relayer's aggregates 7% and
21%. Under Dusk to EVM load the aggregates were 17% of the requests and
32% of the time, at 4.6 ms each against 1.4 to 3.8 ms for single requests.
In the burst the `process` simulations were 30% of the time. The binary
searches over every record's block height, 60 to 71% of the previous run's
requests once there was history, are gone at idle; under load, a lookup by
transaction still searches heights (see What to change).

### Latency

Seconds from the block that holds the dispatch, by the node's log, to each
stage. A validator has signed when its checkpoint file is written; the
relayer's metadata is ready at its first `process` call (an estimate on
Anvil, a simulation on Dusk); a Dusk delivery is submitted when the
relayer starts `dusk-tx`.

Dusk to EVM, 60 messages:

| Stage | Previous: median | p90 | max | This run: median | p90 | max |
|---|---:|---:|---:|---:|---:|---:|
| Final on Dusk | 10.1 | 10.1 | 10.1 | 10.1 | 10.1 | 10.2 |
| First validator signed | 14.5 | 18.0 | 18.5 | 17.0 | 17.8 | 18.1 |
| Second validator signed (quorum) | 15.1 | 18.5 | 18.8 | 17.7 | 18.3 | 18.5 |
| Third validator signed | 15.4 | 18.9 | 19.5 | 18.4 | 18.8 | 19.0 |
| Relayer metadata ready | 15.3 | 18.7 | 19.1 | 18.1 | 18.6 | 20.0 |
| Delivery submitted, and included on Anvil | 15.6 | 19.0 | 19.3 | 18.3 | 18.9 | 20.3 |

EVM to Dusk, 60 messages, one every 25 s:

| Stage | Previous: median | p90 | max | This run: median | p90 | max |
|---|---:|---:|---:|---:|---:|---:|
| EVM validator signed | 5.4 | 9.4 | 9.6 | 7.5 | 8.2 | 8.4 |
| Relayer metadata ready | 5.6 | 9.6 | 9.9 | 8.0 | 8.7 | 9.9 |
| Delivery submitted | 5.7 | 9.7 | 10.0 | 8.1 | 8.8 | 10.0 |
| Included on Dusk | 21.2 | 26.2 | 26.9 | 23.8 | 26.8 | 27.0 |
| Final on Dusk | 31.3 | 36.0 | 36.8 | 33.8 | 36.6 | 37.0 |

EVM to Dusk, the burst of 40 (39 in the previous run):

| Stage | Previous: max | This run: median | p90 | max |
|---|---:|---:|---:|---:|
| Relayer metadata ready | 8.5 | 8.3 | 8.4 | 8.5 |
| Included on Dusk | 778 | 56.0 | 85.5 | 85.7 |
| Final on Dusk | | 65.7 | 95.6 | 95.8 |

- A Dusk validator signs 6.8 s (median) after the block is final, against
  4.6 s before: finality is read once per 5 s tick and shared by the
  agent's indexers, so a validator sees a new final block up to a tick
  later. That is 2.7 s on the median Dusk to EVM delivery; the p90 is
  unchanged. The relayer has its metadata 0.3 s (median) and at most 2.3 s
  after the quorum.
- The EVM validator signs on its own 5 s tick, and the transfers went every
  25 s, so the offset between the two is the same for every message of a
  run: its median moves between runs, its p90 and maximum don't.
- A transaction sent to Rusk still waits 10 to 20 s for its block: the
  node builds each candidate block when its round starts, right after the
  previous block is accepted.

### Throughput into Dusk

**The relayer now delivers a burst in batches that share a block.** The
40 transfers of the burst went in four blocks, 6, 14, 15 and 5
deliveries, every other block, the last 85.7 s after its dispatch, with a
relayer restart in between. The previous run's 39 went in one every 20.3
s, the last 778 s after its dispatch: 3 messages a minute per relayer
key, against 1,000 a block can hold ([Throughput](#throughput)). Paced at
one every 25 s, every delivery went in a block of its own, as before.

A batch goes out about 2 s after the block that holds the batch before it,
when the next block's round has started, so batches land every other
block: at most 16 deliveries per 20 s, 48 a minute, with `maxBatchSize`
16. All 106 deliveries to Dusk were pipelined (`dusk-tx call --nonce N
--no-wait`), on consecutive nonces of the relayer's account. Pipelining is
experimental, and the testnet configs leave it off: there a relayer key
delivers one message every two blocks, as in the previous run.

### Agent resources

| Agent | CPU, idle: previous | this run | Dusk to EVM / EVM to Dusk: previous | this run | RSS, previous | this run |
|---|---:|---:|---:|---:|---:|---:|
| Relayer | 1.9% | 2.0% | 3.3% / 3.0% | 3.0% / 2.8% | 99 to 114 MiB | 104 to 124 MiB |
| Each Dusk validator | 0.1% | 0.1% | 0.3% / 0.2% | 0.2% / 0.1% | 72 to 76 MiB | 61 to 81 MiB |
| EVM validator | 0.1% | 0.1% | 0.1% / 0.2% | 0.1% / 0.2% | 78 MiB | 67 to 82 MiB |

CPU is a share of one core, the `dusk-tx` helpers included, for the debug
builds. The relayer peaked at 24% (5 s samples) after each restart and 34%
over the run, against 29% while catching up before. As in the previous
run, the agents' RSS fell part way through (the validators' from about 80
to 61 MiB), so the lower readings are not comparable.

Disk written per message:

| | Relayer, previous | this run | Each Dusk validator | EVM validator |
|---|---:|---:|---:|---:|
| Database | 61 KiB (Dusk origin), 44 KiB (EVM origin) | 62 KiB, 8.7 KiB | 0.3 KiB | 0.3 KiB |
| `eventCursorDir` | 2.6 KiB, 0.6 KiB | 2.6 KiB, 1.2 KiB | 0.8 KiB | |
| Checkpoints | | | 0.8 KiB, in a 4 KiB file | 0.8 KiB, in a 4 KiB file |
| Log | 95 KiB, 87 KiB | 97 KiB, 52 KiB | 3.6 KiB | 3.8 KiB |

The relayer's database growth is RocksDB's write-ahead log, what it
writes rather than what it keeps. Its `eventCursorDir` now also holds each
Dusk delivery's nonce and transaction until it settles, and its log of
deliveries. The relayer logs 42.5 KiB a minute at idle, 61 MB a day, as
before, almost all of it Hyperlane's EVM submitter (`lander`) reporting
"Processing transactions in finality pool" every second; 125 to 285 KiB a
minute under load, and 871 KiB a minute during the burst. Validators log
at most 2.5 KiB a minute at idle.

### Transactions and gas

| | Previous | This run |
|---|---:|---:|
| Relayer `process` transactions on Dusk | 106, one at a time | 106, all pipelined |
| Executed, at the first transaction | 106 | 106 |
| Failed in a block, or second copies | 0 | 0 |
| Gas, all deliveries to Dusk | 227.1M | 227,100,565 (0.227 DUSK at 1 LUX) |
| Gas, median delivery | 2,142,322 | 2,142,322 |
| Validator announcements | about 2,087,000 each | 2,070,201 to 2,070,791 each |
| Relayer deliveries on Anvil, gas median / max | 106,193 / 140,369 | 111,468 / 145,632 |

The run read every transaction the agents sent back from the chain: each
of the 106 messages to Dusk has exactly one `process` transaction in the
Mailbox's history, executed, and the relayer's 106 deliveries used nonces
117 to 222 of its account, each once.

### Recovery

**Relayer restart with a backlog.** The relayer was stopped while 40 Dusk
to EVM transfers were sent, then restarted with all 40 signed and
waiting, and 60 messages already in its database:

| | Previous | This run | Without the multicall |
|---|---:|---:|---:|
| First of the 40 delivered, after the restart | 16.7 s | 1.4 s | 2.5 s |
| Last of the 40 delivered | 21.8 min, after the relayer was throttled by hand | 22.3 s | 23.8 s |
| Relayer requests to Rusk, first 10 s | 887 | 705 | 1,484 |
| Relayer requests to Rusk until the last delivery | about 4,600 in the minute that read the range, after 20 minutes of refused retries | 731 | 1,579 |
| Of them contract queries | | 180 (133 aggregates carrying 8,348 reads) | 1,015 |
| Data both ways | 6.8 MB of archive pages in that minute | 4.5 MiB | 3.9 MiB |
| Rusk's time answering the relayer, first minute | | 1.8 s | 3.0 s |
| Relayer CPU, peak (5 s samples) | 29% | 24% | 42% |
| Requests Rusk refused (429), all agents | 48 | 0 | 0 |

- The relayer's indexers no longer read the history again: each starts
  above the records its database holds.
- The first 10 s were mostly the archive (`checkBlock` 281,
  `finalizedEvents` 179 in the first minute), which Rusk's GraphQL class
  allows 1,500 a second. Contract queries peaked at 169 in a 2 s window,
  under the 360 Rusk allows all clients together: the agent paces itself
  at 100 a second, and no request was refused in the whole run, without
  the proxy pacing anything.
- The aggregates were the IGP's: 7,922 `gas_payment_at` reads in 80
  aggregates of 99, every payment's height twice for each waiting message
  (see What to change).

**Relayer restart while sending a burst to Dusk.** About 2 s after the
relayer sent the first deliveries of the burst (nonces 177 to 180), it
was stopped. The new process started 11.4 s after the burst and sent two more
0.8 s later (181, 182), and the six went in the same block. It sent the
rest in batches of 14, 15 and 5, two blocks apart; nonces 177 to 216 were
each used once, no delivery was sent twice and none failed. The new
process prepared the four deliveries the old one had in flight again, and
simulated them before their block was final: Rusk refused the 8
simulations with `Mailbox: already delivered`, so nothing was sent for
them. In its first 10 s the new process sent 159 requests to Rusk. The
previous run's restart came while nothing was waiting.

**Endpoint outage.** For 90 s the agents' Rusk ports refused connections,
as from a stopped node, while 6 EVM to Dusk transfers were sent. The agents
logged connection warnings (87 from the relayer, 36 or 37 from each
validator, against 92 and 27 or 28), kept running and needed no restart. In
the first 10 s after the ports reopened the relayer sent 49 requests and
each validator 3 to 5, their idle rate, against 73 and 27 before. The 6
messages were on Dusk 82 s after the ports reopened (123 s before), in four
blocks.

**Node restart.** Not run, as before: `demo/start-env.sh` starts Rusk from
the genesis archive each time, so a restarted node is a new chain. The
outage shows the agents' side.

### Without the multicall

The same agents with `batchContractAddress` left out of every config
(`BATCH_READS=0`), over a new deployment, the 60 Dusk to EVM transfers,
the restart with 40 waiting, and 10 minutes idle after:

| | With the multicall | Without |
|---|---:|---:|
| Relayer at idle, requests a minute: new, after the load | 21, 33 (with confirmations due) | 33, 33 |
| Each Dusk validator at idle | 20 | 20 |
| Rusk's time answering the idle relayer, a minute | 64 ms | 99 ms |
| Dusk to EVM, requests a message: relayer, each validator | 24.1, 6.0 | 30.8, 9.1 |
| Dusk to EVM, Rusk's time a message: relayer, each validator | 64 ms, 14 ms | 64 ms, 19 ms |
| Dusk to EVM, included on Anvil: median, p90, max | 18.3, 18.9, 20.3 s | 18.0, 18.8, 19.1 s |
| Restart with 40 waiting: last delivered | 22.3 s | 23.8 s |
| Restart: relayer requests to Rusk until then (contract queries) | 731 (180) | 1,579 (1,015) |
| Restart: most contract queries in any 2 s | 169 | 219 |

- The relayer's aggregate takes its three counts per final block into one
  query: a third fewer requests at idle. A validator reads one count a
  block, which nothing can batch. Neither rate grew with history: after
  100 messages the relayer without the multicall still sent 33 a minute.
- Without the multicall the agents' own pacing kept the node from refusing
  anything: the restart's contract queries peaked at 219 in 2 s, the most
  the relayer's pace allows (100 a second, bursts of 20), under the 360
  Rusk allows all clients together.
- Through the multicall the restart read eight times more record heights
  than the binary searches did (8,348 reads against 1,015 queries), yet
  took the node about half the time. Per Dusk to EVM message the node
  spent the same 64 ms on the relayer either way: fewer requests, but each
  aggregate reads all of a range's heights (see What to change).

### The launch default, one delivery at a time

A last run on 2026-11-16 used the agents at `aac6b8606c` with the testnet
configs (`maxBatchSize` 1, no `pipelineDeliveries`), dusk-tx at `3c3ce7f`,
and the same tooling: 11 minutes idle on a new deployment, 5 Dusk to EVM
transfers, 40 EVM to Dusk transfers with the relayer restarted in the
middle of them, and 15 minutes idle after.

- **Idle.** The relayer sent 21 requests a minute to Rusk and each Dusk
  validator 20, as in the run above; after the load the relayer sent 29,
  33 to 37 while delivery confirmations came due. Rusk refused nothing
  (429) in the whole run.
- **Deliveries.** The 40 went in 40 blocks, every other block, 780 s from
  the first to the last: one message every 20 s per relayer key. Across
  this run and the end-to-end, rehearsal and restart runs before it, 63
  deliveries to Dusk took 63 transactions, none failed, and no delivery
  was sent twice; the 21 deliveries to Anvil all went through.
- **Restart.** The new process found the old one's transaction in the
  mempool and waited 20 s for it before signing the next delivery, and
  sent 143 requests to Rusk in its first 10 s (159 in the pipelined run
  above, 705 with a backlog). The interrupted message's own transaction
  landed meanwhile: its next attempt's simulation was refused with
  `Mailbox: already delivered` and nothing was sent, but its gas was not
  counted against the message, which matters only once a message is
  retried, and a delivered one is not.

### A flood of deliveries

A relayer keeps every non-reverted `ProcessId` row in its delivery log,
reads at most 16 pages of 200 rows per indexer call, and confirms a
delivery only once its log holds its row. While a contract could deliver,
one transaction could make about 85,000 deliveries to a recipient of its
own that names itself as its ISM: about 170,000 Mailbox rows in one
block, 850 pages to read, and about 25 MB of every relayer's event store
per block (an estimate from the log's keys). Confirmations of real
messages waited while the log read through them. Now a block holds at most
1,000 deliveries, 10 pages, which one indexer call reads, and each costs
its sender a transaction.

### What to change

Of the previous run's list, these are done: the agents pace their
contract queries and back off from 429s with jitter (no request was
refused here, without pacing at the proxy); they remember what is final
(the validators' idle rate stays at 20 a minute with history); a restart
doesn't read the history again (22 s and 731 requests for the backlog);
deliveries to Dusk can be pipelined (`maxBatchSize` 16, measured here: no
second copies, no failed deliveries, a restart in the middle of a batch
reconciled; experimental, and off in the testnet configs);
`lastBlockPair` is read once per tick for all indexers; and a delivery is
simulated once. Done since this run:

- **The heights of final records are kept.** Each contract's searches
  keep the first and last record index of each block they read, for the
  4,096 most recent blocks, and a search reads only between the closest
  kept records. Four Dusk to EVM messages cost 84 requests to Rusk after
  100 IGP payments and after 10,000 alike, with no `gas_payment_at` reads
  in aggregates (88 and 92 requests, 414 and 1,176 reads before), and a
  restart with 40 of 100 messages waiting reads each payment's height once
  (100 reads, against 8,100).
- **A delivery's transaction is looked up as often as a block can bring
  it.** At once, again one block later, every 2.5 s until 20 s, then once
  a block: 17 lookups over the 120 s wait instead of 61, and a transaction
  included 10 to 20 s after it was sent is found within 6 and at most
  2.5 s late. On the default path dusk-tx waits for inclusion itself,
  reading `tx` about every 3 s, about 6 reads a delivery.

What remains, in order of what it would save:

1. **Send the next batch before the last is in a block** (pipelined
   deliveries only). The relayer waits
   for a batch's block before it sends the next, which then misses the
   block whose round has just started: batches land every other block, 48
   deliveries a minute at most with `maxBatchSize` 16. Rusk queues 64 of an
   account's transactions ahead of its next nonce, so two batches in
   flight would fill every block, at the cost of more to reconcile when a
   batch fails.
2. **Let dusk-tx wait like the agent.** Its first `tx` read one block after
   propagation, then every 2.5 s for two blocks, then once a block, would
   take about 3.5 reads a delivery instead of 6, for about the same delay.
3. **Count a delivery's gas when its own transaction lands during a
   restart.** The relayer simulates a message before it looks up the
   transaction it kept for it, so one that landed meanwhile fails the
   simulation as already delivered and its gas goes uncounted (see above).
   Looking the kept transaction up first would report its outcome.
4. **Smaller items.** Validators sign 2.2 s later at the median, since they
   see finality once per 5 s tick. After a restart, the relayer simulates
   the deliveries the old process had in flight until their block is final
   (8 refused queries here, nothing sent). The relayer sometimes sees a
   dispatch before its IGP payment and logs `GasPaymentNotFound` (23 times
   here), with no delay to the delivery: the payment was indexed before the
   signatures came. Plan the relayer's log retention for about 60 MB a day
   at idle and 50 to 100 KiB per message at info level. In the tooling,
   `dusk-tx transfer-remote --native` asked the route for `wrapped_token`
   and `name`, which a native route lacks: two failed queries (500) a
   transfer. Done since: it now reads the route's `state_version`
   instead.
