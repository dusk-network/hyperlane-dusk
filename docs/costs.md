# Costs

What every public entry point of the production contracts costs on chain,
the budgets the tests hold each cost to, whether the deployed fees cover
what relaying costs, what it costs to fill each contract's memory on
purpose, and how much traffic a block takes. All numbers are measured in
the VM by `tests/tests/cost_profile.rs`, on the contracts configured as
`deploy/testnet.sh` deploys them: the MessageIdMultisig ISM as the default
ISM, MerkleTreeHook and ProtocolFee (0.001 DUSK) aggregated as the required
hook, the IGP as the default hook with the testnet pricing for the EVM
chain, and the three warp routes. Gas prices are 1 LUX per gas, the
minimum, so 1M gas is 0.001 DUSK.

## Measuring

```bash
make all canonical-drc20-fixture
cargo test -p hyperlane-dusk-integration-tests --test cost_profile -- --nocapture

# Memory per entry, over 32,768 entries a path (4,096 for those that take a
# transaction each). About 6 minutes in release mode, after the build.
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
within 2,500 gas over all 108 transactions measured: 1.82M for a 250-byte
call, 1.84M for a relayed delivery at one signature, 1.87M at six, and
6.9M for a 63.5 KB transaction. Call gas is tx gas minus this.

## Transactions

### Users

`transfer_remote` deposits are the dispatch fee, 1,025,000 LUX (the 0.001
DUSK protocol fee and the IGP's 25,000 LUX), plus the amount on the native
route. Dispatching from an account spends the same from its prepaid
credit. The largest body is the largest whose transaction fits 64 KiB.

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| WarpNative transfer_remote | 2,131,717 | 302,075 | 27 | 7 | 14 | 1,937 | 404 |
| WarpNative transfer_remote, first message on the Mailbox | 2,136,133 | 306,513 | 27 | 7 | 14 | 1,937 | 404 |
| WarpDrc20 transfer_remote | 2,169,148 | 339,447 | 27 | 7 | 15 | 2,351 | 404 |
| WarpDrc20Collateral transfer_remote | 2,191,752 | 362,051 | 30 | 7 | 15 | 2,351 | 404 |
| Mailbox dispatch_default, empty body | 2,016,241 | 194,426 | 16 | 6 | 10 | 1,399 | 305 |
| Mailbox dispatch_default, 64-byte body | 2,070,565 | 243,643 | 16 | 6 | 10 | 1,463 | 369 |
| Mailbox dispatch_default, 1 KiB body | 3,008,655 | 1,104,883 | 16 | 6 | 10 | 2,423 | 1,329 |
| Mailbox dispatch_default, largest body (63,252 bytes) | 62,352,491 | 55,470,050 | 16 | 6 | 10 | 64,651 | 63,557 |
| Mailbox dispatch, 64-byte body, free hook instead of the IGP | 2,052,907 | 223,791 | 14 | 6 | 8 | 1,312 | 401 |
| Mailbox fund_dispatch, new payer | 1,828,799 | 8,779 | 1 | 3 | 3 | 971 | 298 |
| Mailbox fund_dispatch, existing payer | 1,828,643 | 8,623 | 1 | 3 | 3 | 971 | 298 |
| Mailbox withdraw_dispatch_credit | 58,696,331 | 56,862,325 | 1 | 3 | 3 | 1,010 | 477 |
| WarpNative withdraw_dispatch_credit | 58,699,182 | 56,865,239 | 2 | 4 | 3 | 1,010 | 477 |
| WarpDrc20 withdraw_dispatch_credit | 58,699,182 | 56,865,239 | 2 | 4 | 3 | 1,010 | 477 |
| WarpDrc20Collateral withdraw_dispatch_credit | 58,699,737 | 56,865,794 | 2 | 4 | 3 | 1,010 | 477 |
| WarpDrc20 transfer, to a new holder | 1,874,197 | 41,553 | 0 | 2 | 2 | 1,071 | 461 |
| WarpDrc20 transfer, to a holder | 1,874,506 | 41,862 | 0 | 2 | 2 | 1,071 | 461 |
| WarpDrc20 approve, a new spender | 1,873,133 | 40,785 | 0 | 2 | 2 | 1,071 | 460 |
| WarpDrc20 approve, an existing allowance | 1,872,878 | 40,530 | 0 | 2 | 2 | 1,071 | 460 |
| WarpDrc20 transfer_from | 1,909,302 | 60,909 | 0 | 2 | 2 | 1,071 | 658 |
| WarpNative register_account, new account | 1,851,356 | 35,675 | 0 | 2 | 2 | 707 | 261 |
| WarpNative register_account, registered account | 1,851,391 | 35,710 | 0 | 2 | 2 | 707 | 261 |
| WarpNative claim_pending | 1,868,500 | 52,777 | 1 | 3 | 4 | 1,030 | 258 |
| WarpNative claim_pending_for | 58,656,747 | 56,824,224 | 1 | 3 | 4 | 1,030 | 462 |
| WarpNative claim_pending_contract, by the red-team contract | 1,844,844 | 22,950 | 3 | 5 | 3 | 813 | 332 |
| WarpDrc20 register_account, new account | 1,851,482 | 35,675 | 0 | 2 | 2 | 707 | 261 |
| WarpDrc20 register_account, registered account | 1,851,517 | 35,710 | 0 | 2 | 2 | 707 | 261 |
| WarpDrc20 claim_pending | 1,874,624 | 58,901 | 0 | 2 | 4 | 1,185 | 258 |
| WarpDrc20 claim_pending_for | 58,666,842 | 56,834,319 | 0 | 2 | 4 | 1,185 | 462 |
| WarpDrc20 claim_pending_contract, by the red-team contract | 1,839,306 | 17,412 | 1 | 3 | 3 | 1,135 | 332 |
| WarpDrc20Collateral register_account, new account | 1,851,545 | 35,675 | 0 | 2 | 2 | 707 | 261 |
| WarpDrc20Collateral register_account, registered account | 1,851,580 | 35,710 | 0 | 2 | 2 | 707 | 261 |
| WarpDrc20Collateral claim_pending | 1,878,294 | 62,634 | 5 | 3 | 4 | 1,185 | 258 |
| WarpDrc20Collateral claim_pending_for | 58,671,271 | 56,838,748 | 5 | 3 | 4 | 1,185 | 462 |
| WarpDrc20Collateral claim_pending_contract, by the red-team contract | 1,850,382 | 28,488 | 6 | 4 | 3 | 1,135 | 332 |

- A transfer makes 27 to 30 inter-contract calls, 7 frames deep: route,
  Mailbox, quotes from every hook, the fee forwarding through the transfer
  contract, and the AggregationHook's payment to the ProtocolFee.
- The first message on a fresh Mailbox costs 4,416 gas more than the next:
  starting the first chunk of every log takes 2 pages in each of the
  Mailbox, the MerkleTreeHook and the IGP. Memory isn't priced by size.
- A dispatch costs about 980 gas per body byte beyond 64.
- Withdrawing credit and sponsored claims decode a public key on chain,
  about 57M gas ([security.md](security.md), LOW-7 and CRITICAL-1).

### Relayed deliveries

The relayer's `process`, signed by the last validators of a sorted set of
6, the worst case for the ISM's scan. Tx gas:

| Delivery | 1 signature | 3 signatures | 6 signatures | Calls | Depth | Events | Event bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| WarpNative, payout to a registered account | 2,142,500 | 2,579,020 | 3,231,735 | 5 | 4 | 5 | 1,105 |
| WarpNative, new escrow | 2,127,810 | 2,563,061 | 3,215,673 | 4 | 4 | 4 | 846 |
| WarpNative, existing escrow | 2,127,696 | 2,562,940 | 3,216,244 | 4 | 4 | 4 | 846 |
| WarpDrc20, mint to a registered account | 2,146,485 | 2,582,095 | 3,233,752 | 3 | 3 | 5 | 1,260 |
| WarpDrc20, new escrow | 2,126,269 | 2,561,313 | 3,213,933 | 3 | 3 | 4 | 846 |
| WarpDrc20, existing escrow | 2,125,870 | 2,561,243 | 3,214,992 | 3 | 3 | 4 | 846 |
| WarpDrc20Collateral, payout to a registered account | 2,183,366 | 2,617,945 | 3,270,197 | 9 | 4 | 5 | 1,260 |
| WarpDrc20Collateral, new escrow | 2,128,361 | 2,562,989 | 3,215,242 | 4 | 4 | 4 | 846 |
| WarpDrc20Collateral, existing escrow | 2,128,375 | 2,562,979 | 3,215,616 | 4 | 4 | 4 | 846 |
| A recipient contract, empty body | 2,105,628 | 2,539,295 | 3,191,356 | 3 | 3 | 3 | 774 |

Each signature adds about 218,000 gas: 208,000 in the ISM's check, 4,900
in passing it to the ISM, and 5,200 for the 65 bytes it adds to the
transaction.

| Signatures | Tx bytes | Call gas, WarpNative payout |
|---:|---:|---:|
| 1 | 544 | 303,517 |
| 3 | 672 | 729,797 |
| 6 | 868 | 1,366,832 |

Every delivery to a route fits the relayer's 5M limit with 1.5x headroom
up to 6 signatures, and stays under the least limit `deploy/testnet.sh`
accepts at its signature count. The script's estimate itself, 1.95M plus
0.22M per signature, is below the costliest delivery, a payout from the
collateral route that locks the upstream DRC20, by 13,366 gas at one
signature, 7,945 at three and 197 at six; its 1.5x margin covers that.

A route's messages have a 64-byte body. A message to another recipient
contract can be larger, and each body byte costs about 290 gas, 80 of it in
the signature check:

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| Mailbox process, recipient contract, 1 KiB body, 6 of 6 signatures | 3,493,138 | 1,551,494 | 3 | 3 | 3 | 774 | 1,828 |
| Mailbox process, recipient contract, 16 KiB body, 6 of 6 signatures | 7,992,868 | 4,822,424 | 3 | 3 | 3 | 774 | 17,188 |
| Mailbox process, recipient contract, largest body (62,761 bytes), 6 of 6 signatures | 21,583,095 | 14,702,083 | 3 | 3 | 3 | 774 | 63,564 |
| Mailbox process, recipient that is its own ISM | 1,844,055 | 20,558 | 3 | 3 | 3 | 774 | 348 |

At the 5M limit a delivery can carry about 9,900 body bytes at one
signature and 6,200 at six; with the 1.5x headroom, 4,200 and 480. The
generated relayer config only relays this deployment's routes.

### Validators

| Call | Tx gas | Call gas | Calls | Depth | Events | Event bytes | Tx bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| ISM verify, 1 of 6 signatures | 2,075,485 | 241,137 | 0 | 2 | 1 | 657 | 479 |
| ISM verify, 3 of 6 signatures | 2,501,234 | 656,664 | 0 | 2 | 1 | 657 | 607 |
| ISM verify, 6 of 6 signatures | 3,140,433 | 1,280,183 | 0 | 2 | 1 | 657 | 803 |
| ISM verify, 6 of 255 signatures | 3,151,499 | 1,291,308 | 0 | 2 | 1 | 657 | 803 |
| ValidatorAnnounce announce, new validator, 50-byte location | 2,068,800 | 240,662 | 0 | 2 | 2 | 759 | 405 |
| ValidatorAnnounce announce, second location of a validator | 2,069,589 | 240,611 | 0 | 2 | 2 | 759 | 409 |
| ValidatorAnnounce announce, new validator, 1 KiB location | 2,252,203 | 345,542 | 0 | 2 | 2 | 1,731 | 1,381 |

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
| IGP claim | 1,867,033 | 52,453 | 1 | 3 | 2 | 916 | 250 |
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
| Mailbox delivered | 321 | 1 |
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
| WarpNative pending_balance | 266 | 8 |
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
| WarpDrc20 pending_balance | 266 | 8 |
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
| WarpDrc20Collateral pending_balance | 266 | 8 |
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
32,768 entries (4,096 where each entry takes a transaction; memory grows in
64 KiB pages, so that resolves 2 and 16 bytes an entry). Gas per entry is
the marginal gas of one more call from a contract that makes many in one
transaction, or a whole transaction where each needs its own.

| Path | Contract | Bytes per entry | Gas per entry |
|---|---|---:|---:|
| Dispatch through the deployed hooks | Mailbox | 32 | 180,027 marginal |
|  | MerkleTreeHook | 32 |  |
|  | IGP | 64 |  |
| | ProtocolFee, AggregationHook | 0 | |
| Delivery to a recipient that is its own ISM | Mailbox | 26 | 35,128 marginal |
| New validator, 1-byte location | ValidatorAnnounce | 108 | 253,089 marginal |
| New validator, 1 KiB location | ValidatorAnnounce | 110 | 449,262 marginal |
| WarpDrc20 transfer to a new holder | WarpDrc20 | 62 | 56,304 marginal |
| WarpDrc20 approval of a new spender | WarpDrc20 | 60 | 53,043 marginal |
| WarpNative escrow for a new recipient | WarpNative | 74 | 57,887 marginal |
|  | Mailbox | 26 |  |
| WarpDrc20 escrow for a new recipient | WarpDrc20 | 74 | 58,069 marginal |
|  | Mailbox | 26 |  |
| WarpDrc20Collateral escrow for a new recipient | WarpDrc20Collateral | 74 | 59,886 marginal |
|  | Mailbox | 26 |  |
| Mailbox credit for a new payer | Mailbox | 64 | 1,831,125 per tx |
| WarpNative registration of a new account | WarpNative | 336 | 1,854,495 per tx |
| WarpDrc20 registration of a new account | WarpDrc20 | 336 | 1,854,613 per tx |
| WarpDrc20Collateral registration of a new account | WarpDrc20Collateral | 336 | 1,854,613 per tx |

- An escrow delivery also adds the Mailbox's 26 bytes for the delivery;
  the gas is the contract side of a delivery with no signatures to check.
  The relayer pays a whole `process` for each (see
  [Relayed deliveries](#relayed-deliveries)).
- These agree with [storage.md](storage.md#costs-per-operation) within the
  2 bytes a page leaves unresolved, and with the registry's 338.6 bytes in
  [security.md](security.md#open-registry-growth) within 16. The
  announcement stores the location's hash, so the location's length only
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

What the deployed configuration charges, against what delivering costs,
per warp transfer. Assumptions: an EVM gas price of 1 gwei; 1 ETH = 30,000
DUSK (ETH at 3,000 USD, DUSK at 0.10 USD); the Dusk-origin validator set of
the local rehearsal, 2 of 3. The verdicts don't depend on them within
orders of magnitude.

### Dusk to Sepolia

- **Charged, on Dusk:** 1,025,000 LUX, 0.001025 DUSK. The ProtocolFee's
  0.001 DUSK goes to its beneficiary and pays nothing toward delivery. The
  IGP charges 25,000 LUX for 250,000 destination gas: the routes send no
  hook metadata, which the IGP reads as 50,000 gas, plus
  `DUSK_IGP_GAS_OVERHEAD`, 200,000, at `DUSK_IGP_GAS_PRICE` 1 gwei and
  `DUSK_IGP_TOKEN_EXCHANGE_RATE` 1, which is 1e-10 LUX per wei. The local
  rehearsal sources the same values (`deploy/local.env`).
- **Cost, on Sepolia:** the relayer's `process` of a synthetic DUSK mint.
  Hyperlane prices Mailbox `process` with MessageId multisig verification at
  151,966 gas for 1 of 1, 159,736 for 2 of 3 and 187,506 for 6 of 6
  (`multisigIsmVerifyCosts` in the monorepo's SDK, which its IGP overheads
  use), and a synthetic route's `handle` at 64,000 (its default destination
  gas; in the Solidity gas snapshot, the test that benchmarks
  `HypERC20.handle` takes 58,924). For 2 of 3: about 224,000 gas,
  0.000224 ETH at 1 gwei, which is 6.7 DUSK at the assumed rate.
- **Verdict:** the relayer loses about 0.000224 ETH, 6.7 DUSK, on every
  message, and the IGP covers 1/270,000 of it. The 250,000 gas it charges
  for is about right (it covers up to 5 signatures; 6 of 6 needs 251,506);
  the price is off by the exchange rate. Charging the cost needs
  `DUSK_IGP_GAS_PRICE` at the EVM gas price in wei and
  `DUSK_IGP_TOKEN_EXCHANGE_RATE` at 10 x (ETH price / DUSK price), 300,000
  at the assumed prices: (50,000 + 200,000) x 1e9 x 300,000 / 1e10 =
  7.5e9 LUX, 7.5 DUSK. On testnet the ETH is Sepolia ETH, so the loss is
  the faucet ETH the relayer's account needs, 0.000224 per message.

### Sepolia to Dusk

- **Charged, on Sepolia:** nothing for delivery. Sepolia's IGP has no gas
  oracle for the Dusk domain, and the route's hook falls back to plain
  Merkle insertion, so a transfer pays only Hyperlane's 1 wei protocol fee
  ([testnet.md](testnet.md#what-gets-deployed)).
- **Cost, on Dusk:** the relayer's `process` at `EVM_ORIGIN_THRESHOLD` 1:
  2,142,500 gas for a payout from the native route and 2,127,810 for a new
  escrow, 0.0021 DUSK. At 6 signatures, up to 3,231,735 on the native route
  (3,270,197 on a collateral route). A delivery that passes the simulation
  and fails pays the relayer's whole 5M limit, 0.005 DUSK.
- **Verdict:** the relayer loses the whole cost, about 0.0021 DUSK (0.0002
  USD at the assumed price) per message, and anyone can make it pay that
  by sending transfers from Sepolia, where they cost Sepolia gas only.
  Charging it needs a Dusk gas oracle in the EVM IGP: 2.15M gas plus 0.22M
  per signature above one, at a gas price of 1 (LUX) and a token exchange
  rate of 1e19 / (ETH price / DUSK price), 3.3e14 at the assumed prices,
  about 7.2e10 wei per message.

The relayer config `deploy/testnet.sh` writes enforces no gas payments, so
the relayer delivers either way. Both directions need pricing, and
`gasPaymentEnforcement` turned on, before mainnet
([testnet.md](testnet.md#before-mainnet)).

## Attack costs

What it costs to add a byte of permanent state through each permissionless
path, at 1 LUX per gas, with the fees each entry pays. "Batched" is from a
contract that makes many calls in one transaction, where the path allows
it; "one tx each" is a transaction per entry.

| Path | Contract | Bytes per entry | Gas per entry, batched | Gas per entry, one tx each | Fees per entry | DUSK per MiB, batched | DUSK per MiB, one tx each |
|---|---|---:|---:|---:|---:|---:|---:|
| Delivery to a recipient that is its own ISM | Mailbox | 26 | 35,128 | 1,844,055 | none | 1.4 | 74.4 |
| Dispatch through the IGP | Mailbox, MerkleTreeHook, IGP | 128 | 180,027 | 2,016,241 | 0.001025 DUSK | 9.9 | 24.9 |
| Credit for a new payer | Mailbox | 64 | no | 1,831,125 | 1 LUX | no | 30.0 |
| New validator, 1-byte location | ValidatorAnnounce | 108 | 253,089 | 2,068,800 | none | 2.5 | 20.1 |
| Transfer to a new holder | WarpDrc20 | 62 | 56,304 | 1,874,197 | 1 token unit | 1.0 | 31.7 |
| Approval of a new spender | WarpDrc20 | 60 | 53,043 | 1,873,133 | none | 0.9 | 32.7 |
| Registration of a new account | a route | 336 | no | 1,854,495 | a funded key | no | 5.8 |
| Escrow for a new recipient | a route | 74 | no | 2,127,810 | an EVM transfer | no | 30.2 |

- The one-tx gas of an announcement is for a 50-byte location; a 1-byte one
  costs about the same.
- A registration needs a transaction signed by the new key, and the key
  needs DUSK for its gas first: with a funding transfer per key, about
  3.7M gas and 11.6 DUSK per MiB.
- Escrow is paid for by the relayer, on Dusk, through its `process` at one
  signature; the attacker pays an EVM transfer per entry.

What filling each contract to its ceiling costs, with the cheapest path:

| Contract | Cheapest path | Entries | DUSK | Notes |
|---|---|---:|---|---|
| Mailbox, dispatch closed at 3 GiB | deliveries to a recipient that is its own ISM | about 122M | about 4,300 | [storage.md](storage.md#ceilings) |
| Mailbox, full at 4 GiB | the same | about 163M | about 5,700 | storage.md |
| Mailbox, dispatch closed at 3 GiB | credit for new payers | 50M | about 92,200 | new; one transaction each |
| Mailbox, dispatch closed at 3 GiB | dispatches with a free hook | about 100M | about 15,500 of gas plus 100,000 of protocol fees | storage.md |
| MerkleTreeHook | dispatches | not reachable |  | the Mailbox closes dispatch first |
| IGP, full at 4 GiB | dispatches through the IGP | 67M | about 12,100 of gas plus 68,800 of fees | each pays the protocol fee too |
| ValidatorAnnounce, full at 4 GiB | new validators, 1-byte locations | 40M | about 10,100 | storage.md's 17,400 is for 1 KiB locations |
| WarpDrc20, full at 4 GiB | transfers to new holders | 69M | about 3,900, plus a token unit each |  |
| WarpDrc20, full at 4 GiB | approvals of new spenders | 72M | about 3,800 |  |
| A route, full at 4 GiB | registrations | 12.8M | about 23,700; 47,400 with a funding transfer each | [security.md](security.md#open-registry-growth) |
| A route, full at 4 GiB | escrow for new recipients | 58M | about 123,000, paid by the relayer | plus an EVM transfer each |
| ProtocolFee, AggregationHook, ISM | none |  |  | no permissionless state |

- ProtocolFee and AggregationHook have no state anyone can add to: neither
  grew over 32,768 dispatches. The ISM's state is its owner's validator
  set.
- A route's escrow grows only through deliveries from its enrolled router,
  each a transfer someone sent from the EVM chain, and the relayer pays for
  each delivery on Dusk. Filling a native route's 4 GiB with escrow takes
  about 58M transfers: about 123,000 DUSK of the relayer's gas, and 58M EVM
  transactions from the attacker.

## Throughput

A block holds at most 3e9 gas, 1,000 transactions, and 1 MiB with its
header (the size column leaves the header out), and one comes at most
every 10 seconds. Block gas counts what transactions spend, not their
limits.

| Transaction | Tx gas | Tx bytes | Per block by gas | By size | By count | Per block |
|---|---:|---:|---:|---:|---:|---:|
| Relayed delivery, 1 signature | 2,142,500 | 544 | 1,400 | 1,882 | 1,000 | 1,000 |
| Relayed delivery, 6 signatures | 3,231,735 | 868 | 928 | 1,190 | 1,000 | 928 |
| Relayed delivery to a collateral route, 6 signatures | 3,270,197 | 868 | 917 | 1,190 | 1,000 | 917 |
| WarpNative transfer_remote | 2,131,717 | 404 | 1,407 | 2,514 | 1,000 | 1,000 |
| WarpDrc20Collateral transfer_remote | 2,191,752 | 404 | 1,368 | 2,514 | 1,000 | 1,000 |
| Dispatch from an account | 2,016,241 | 305 | 1,487 | 3,297 | 1,000 | 1,000 |

- Relayed deliveries are capped by the 1,000-transaction limit at one
  signature (gas would allow 1,400) and by gas at six: 928 to the native
  route, 917 to a collateral route. That is 100 or 93 messages a second
  into Dusk.
- Transfers out of Dusk are capped at 1,000 a block by the transaction
  limit, 100 a second.
- From a contract, one transaction can make many calls, so gas is the only
  limit: 16,664 dispatches or 85,401 deliveries to a recipient that is its
  own ISM per block. Closing the Mailbox's dispatch with deliveries, about
  122M of them, takes about 1,430 full blocks, 4 hours.

## Agents: API usage, latency and resources

What the relayer and the validators cost a Rusk node: the requests each
agent sends, at idle and per message, how long each stage of a message
takes, and the agents' CPU, memory, disk and log use. Measured on one run
of a local rehearsal of the testnet deployment, with every agent's traffic
going through a logging proxy. [demo/bench/](../demo/bench/README.md) has
the tooling and how to run it.

### Setup

- One local Rusk 1.7.3 node built with `archive` (the commit in
  `RUSK_VERSION`), with a block every 10 s, each final when the next one is
  accepted. Anvil 1.2.3, mining blocks as transactions arrive.
- The local rehearsal of the testnet deployment
  ([testnet.md](testnet.md#rehearse-locally)), deployed with
  `deploy/testnet.sh`: the native DUSK route and MessageIdMultisig ISMs on
  both sides.
- The agents at `hyperlane-monorepo` `1733ab9120`, debug build, with the
  configs `deploy/testnet.sh agent-configs` writes, run as
  `deploy/local-agents.sh` runs them: a relayer, three Dusk-origin
  validators (2 of 3) and one EVM-origin validator (1 of 1, standing in for
  Hyperlane's Sepolia validator). Validators run with `interval: 5`.
  Default log level (info), pretty format.
- This repository at `39fe282`, whose relayer config enforced no gas
  payments and relayed only the route. Since `274431a` the relayer enforces
  IGP payments (`onChainFeeQuoting`) and relays any paid message. It reads
  the payments from the same IGP index, which the measured relayer kept
  anyway (`igpIndexingEnabled` defaults to true).
- Each agent reaches Rusk and Anvil through its own port of
  `demo/bench/proxy.py`, which logs every request. The agents run `dusk-tx`
  through a shim that logs each run and tags its requests, so they count
  for the agent that ran it.
- Transfers as in [testnet.md](testnet.md#check-it-works):
  `dusk-tx transfer-remote --native` from the deployer's account, and
  `transferRemote` on the EVM token from Anvil's account #0. They share the
  Dusk account with the agents, so Dusk to EVM transfers ran while the
  relayer had nothing to deliver to Dusk.

| Phase | Minutes | What happened |
|---|---:|---|
| Baseline | 3.0 | Node and Anvil, no agents |
| Idle | 11.0 | All five agents, no messages yet |
| Dusk to EVM | 20.3 | 60 transfers, each sent when the last was in a block |
| Backlog | 35.2 | Relayer stopped while 40 more were sent, then restarted |
| EVM to Dusk | 25.0 | 60 transfers, one every 25 s |
| Burst | 25.6 | 40 transfers at once (see below), with a relayer restart |
| Outage | 3.7 | The agents' Rusk ports refused connections for 90 s while 6 transfers were sent |
| Idle after | 8.0 | No traffic, with 100 Dusk and 106 EVM messages in |

In the burst, 39 of the 40 transfers ran out of gas on Anvil, because each
`cast send` estimated its gas before any of them was mined and later Merkle
insertions cost more. The 39 were sent again at once 12.5 minutes later
with a fixed gas limit, which `load.sh` now sets. The relayer restart in
that phase came while nothing was waiting.

### Idle

| Agent | Rusk req/min, new deployment | KiB/min | Rusk req/min, after 100 messages | KiB/min | Anvil req/min |
|---|---:|---:|---:|---:|---:|
| Relayer | 75 | 28.7 | 305 | 106.8 | 39 to 51 |
| Each Dusk validator | 26 | 9.9 | 109 | 37.1 | |
| EVM validator | | | | | 25 to 27 |
| All five | 153 | 58.4 | 632 | 218.1 | 64 to 78 |

Serving these took 0.4% of one node thread on a new deployment and 2.4%
after 100 messages, by the time Rusk spent answering. Rusk itself used 1.1%
of a core with no agents, 1.2% with the agents idle on a new deployment and
2.7% after 100 messages.

Requests a minute by kind, on a new deployment and after 100 messages:

| Request | Relayer, new | Relayer, after | Each Dusk validator, new | Each Dusk validator, after |
|---|---:|---:|---:|---:|
| `lastBlockPair` (GraphQL) | 36 | 38 | 12 | 14 |
| MerkleTreeHook `count` | 12 | 12 | 12 | 13 |
| Mailbox `nonce` | 12 | 12 | | |
| IGP `gas_payment_count` | 12 | 12 | | |
| MerkleTreeHook `inserted_block_height` | | 72 | | 78 |
| Mailbox `dispatched_block_height` | | 72 | | |
| IGP `gas_payment_at` | | 72 | | |
| Other: metrics, `delivered`, `checkBlock`, `finalizedEvents` | 3 | 15 | 2 | 4 |

- Each Dusk validator's Merkle insertion indexer reads `lastBlockPair` and
  the hook's `count` every 5 s, the indexers' poll interval once they are
  caught up. The validator reads the latest checkpoint as well, about once
  a minute when nothing changes. The relayer's three Dusk indexers
  (dispatches, Merkle insertions and IGP payments) each read
  `lastBlockPair` and their contract's count every 5 s.
- Once a minute every agent reports chain metrics: `blocks(last: 1)`, the
  mempool gas price and, for the relayer, its balance.
- The idle rate grows with the deployment's history. To learn how much of
  a log is final, every tick binary-searches the block heights of all its
  records for the first one past the last finalized block: about
  log2(count) + 1 lookups. With 100 messages that is 6 lookups a tick, 72 a
  minute per indexer, and it was 71% of what the node served at idle.
  Validators search again whenever they read the latest checkpoint.

### Per message

What each message added over the idle rate. Dusk to EVM grew the Dusk
history from 0 to 60 messages, so its range is over the idle rate of a new
deployment (the high end) and over the rate after 100 messages (the low
end). EVM to Dusk left the Dusk history alone and is over the rate after
100 messages.

| Agent | Dusk to EVM: Rusk req | Rusk KiB | Anvil req | Anvil KiB | EVM to Dusk: Rusk req | Rusk KiB | Anvil req | Anvil KiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Relayer | 49 to 128 | 22 to 49 | 24 to 28 | 34 to 36 | 25 | 14.3 | 5.5 | 23.9 |
| Each Dusk validator | 16 to 44 | 7 to 16 | | | 0 | 0 | | |
| EVM validator | | | 0 | 0 | | | 3.2 | 2.8 |
| The sender | 17 | 6.7 | | | | | 8.0 | 11.4 |

For a Dusk to EVM message, over the idle rate of a new deployment, the
relayer's 128 requests were 35 IGP `gas_payment_at`, 32 Merkle
`inserted_block_height` and 27 Mailbox `dispatched_block_height`. These are
the binary searches above, plus the same searches to find the message's IGP
payment and Merkle insertion by its dispatch transaction. The rest were 9
`lastBlockPair`, 7 `checkBlock`, 3 `finalizedEvents` pages, 8 counts, and 7
reads of the new records (`dispatched_id`, `message_id_at`, `gas_payments`,
`tx`). Each validator's 44 were 31 `inserted_block_height`, 4
`lastBlockPair`, 3 `count` and 6 reads of the new insertion and its row.
On Anvil the relayer's largest item was `delivered`, 8.6 times a message.
Hyperlane re-prepares a message every 0.5 s for 10 s while it waits for
signatures, and every attempt starts by asking the destination whether the
message was delivered.

For an EVM to Dusk message, the relayer's 25 requests to Rusk were:

| Request | Per message | Why |
|---|---:|---|
| Mailbox `delivered` | 9.6 | The same 0.5 s signature wait, now on Rusk |
| `dusk-tx`: `tx` status | 6.0 | The helper polls every 3 s until the transaction is in a block |
| `dusk-tx`: `chain_id`, `account`, `preverify`, `propagate`, `block` | 5.0 | One each per submission |
| Mailbox `process`, as a query | 3.0 | Simulated by the relayer's prepare, again by its dry run before submitting, and again in the Dusk Mailbox's `process` |
| Other: ISM `validators_and_threshold`, the relayer's own `tx` read | 1.3 | |

The relayer ran `dusk-tx` once per EVM to Dusk message and never otherwise.
All 106 of its `process` transactions were included at the first attempt,
at 2,141,379 to 2,146,175 gas, median 2,142,322 (0.0021 DUSK at 1 LUX),
227.1M in all. The validators each sent one transaction, their
announcement at startup, at about 2,087,000 gas. On Anvil the relayer's 100
deliveries took 106,193 gas at the median and 140,369 at most.

### What the node spends its time on

Share of requests and of node time, all agents together:

| Request | Idle, new | Dusk to EVM | EVM to Dusk | Idle, after 100 messages |
|---|---:|---:|---:|---:|
| MerkleTreeHook `inserted_block_height` | | 40% / 41% | 44% / 39% | 48% / 46% |
| Mailbox `dispatched_block_height` and IGP `gas_payment_at` | | 20% / 22% | 20% / 21% | 22% / 25% |
| `lastBlockPair` | 47% / 35% | 15% / 9% | 11% / 7% | 13% / 9% |
| MerkleTreeHook `count` | 31% / 36% | 9% / 11% | 7% / 9% | 8% / 11% |
| Requests a minute | 153 | 918 | 690 | 632 |
| One node thread busy | 0.4% | 2.9% | 2.8% | 2.4% |
| Rusk CPU, one core | 1.2% | 3.7% | 3.2% | 2.7% |

The block-height lookups were 60 to 71% of the requests and of the node's
time whenever any history existed. No request kind was slow. Every kind
took under 7 ms on average except the simulated `process`, 9 ms, and the
archive's `finalizedEvents` pages were 0.9 KiB at the median because each
scan starts from a saved position. Rusk's CPU includes the transfers' own
transactions and the consensus work of a 10 s chain.

### Latency

Seconds from the block that holds the dispatch, by the node's log, to each
stage. A validator has signed when its checkpoint file is written; the
relayer's metadata is ready at its first `process` call (an estimate on
Anvil, a simulation on Dusk); a Dusk delivery is submitted when the relayer
starts `dusk-tx`.

Dusk to EVM, 60 messages:

| Stage | Median | p90 | Max |
|---|---:|---:|---:|
| Final on Dusk | 10.1 | 10.1 | 10.1 |
| First validator signed | 14.5 | 18.0 | 18.5 |
| Second validator signed (quorum) | 15.1 | 18.5 | 18.8 |
| Third validator signed | 15.4 | 18.9 | 19.5 |
| Relayer metadata ready | 15.3 | 18.7 | 19.1 |
| Delivery submitted, and included on Anvil | 15.6 | 19.0 | 19.3 |

EVM to Dusk, 60 messages, one every 25 s:

| Stage | Median | p90 | Max |
|---|---:|---:|---:|
| EVM validator signed | 5.4 | 9.4 | 9.6 |
| Relayer metadata ready | 5.6 | 9.6 | 9.9 |
| Delivery submitted | 5.7 | 9.7 | 10.0 |
| Included on Dusk | 21.2 | 26.2 | 26.9 |
| Final on Dusk | 31.3 | 36.0 | 36.8 |

- A validator signs 4.6 s (median) and up to 8.4 s after the block is
  final; it looks every 5 s. The relayer polls for signatures every 0.5 s
  and has its metadata within 0.2 s of the quorum.
- A transaction sent to Rusk waits 10 to 20 s for its block. The node
  builds each candidate block when its round starts, right after the
  previous block is accepted, so a transaction waits for the next round.
  The Dusk to EVM transfers above, each sent just after the last one's
  block, waited 18.3 s (median) before the dispatch's block.
- **The relayer delivers to Dusk one message per two blocks.** It submits
  one message at a time, and `dusk-tx` returns once the transaction is in a
  block, so the next one misses the round that has just started. The 39
  burst messages had their signatures and metadata within 8.5 s, then went
  in one every 20.3 s, the last 778 s after its dispatch.
  That is 3 messages a minute per relayer key, against the 1,000 a block
  can hold ([Throughput](#throughput)).
- The relayer checks a delivery 10 minutes after submitting it
  (Hyperlane's `CONFIRM_DELAY`), so it only logs Dusk deliveries as
  confirmed then.

### Agent resources

| Agent | CPU, idle | CPU, Dusk to EVM / EVM to Dusk | RSS |
|---|---:|---:|---:|
| Relayer | 1.9% | 3.3% / 3.0% | 99 MiB idle, 114 MiB at most |
| Each Dusk validator | 0.1% | 0.3% / 0.2% | 72 to 76 MiB |
| EVM validator | 0.1% | 0.1% / 0.2% | 78 MiB |

CPU is a share of one core, the `dusk-tx` helpers included, for the debug
build. The relayer peaked at 29% while catching up after its restart. Its
memory grew 10 MiB over the run. The host reclaimed memory halfway through,
so later RSS readings (the validators fell to 58 to 63 MiB) are not
comparable.

Disk written per message:

| | Relayer | Each Dusk validator | EVM validator |
|---|---:|---:|---:|
| Database | 61 KiB (Dusk origin), 44 KiB (EVM origin) | 0.3 KiB | 0.3 KiB |
| `eventCursorDir` | 2.6 KiB, 0.6 KiB | 0.8 KiB | |
| Checkpoints | | 0.8 KiB, in a 4 KiB file | 0.8 KiB, in a 4 KiB file |
| Log | 95 KiB, 87 KiB | 3.6 KiB | 3.7 KiB |

The relayer's database growth is RocksDB's write-ahead log. No memtable was
flushed during the run, so it measures what the relayer writes, not what
it keeps. A validator only writes for its own origin's messages.

At idle the relayer logs 42.5 KiB a minute, 61 MB a day, almost all of it
Hyperlane's EVM submitter (`lander`) reporting "Processing transactions in
finality pool" every second. Under load it logged 210 to 280 KiB a minute;
per message most of it is the signature wait ("List of validators and
threshold for message" and "Could not fetch metadata: Unable to reach
quorum" every 0.5 s) and the submitter's status lines. Validators log at
most 2.5 KiB a minute at idle.

### Recovery

**Relayer restart with a backlog.** The relayer was stopped while 40 Dusk
to EVM transfers were sent, then restarted with all 40 signed and waiting.
The newest was delivered 16.7 s after the restart. The other 39 waited 21.8
minutes, and without the intervention below would have kept waiting:

- On start, each of the relayer's three Dusk indexers starts a backward
  cursor at the tip, which re-read sequences 0 to 98 in one range
  (`index.chunk` defaults to 1999), although the database already held the
  first 60. On Dusk a range costs a few contract queries per record, plus
  the searches for each record's transaction, against one `eth_getLogs` on
  an EVM chain.
- The indexers sent 887 requests in the first 10 s and 439 in the next.
  Rusk 1.7.3's default HTTP policy allows 360 contract queries in any 2 s,
  for all clients together (`contract_query`: 180 a second, burst 360, at
  most 12 at once), and answers the rest `429 too_many_requests`.
- An indexer gives up its range at the first 429 and backs off 5, 10, 20,
  40, 80, 160 and then 300 s. All three back off alike, so they retry
  together and hit the limit together. The relayer was refused 1, 7, 17,
  38, 78, 158, 319 and 619 s after the restart.
- The limit is global, so the same bursts made Rusk refuse each validator's
  polls as well, 3 each. The node logged 48 refusals.

10.5 minutes in, the relayer's Rusk port was moved to a second proxy
instance that holds its contract queries to 150 a second, which still drew
5 refusals (a full bucket plus 150 a second exceeds 360 in a window), then
to 100 a second. The next retry, 1,221 s after the restart, read the range
without a refusal. It took about 4,600 requests and 6.8 MB of archive pages
in a minute, 46 requests and 69 KB per message of history (each record's
lookup reads its archive page from the nearest saved position, up to 200
rows). The 39 were delivered within 90 s. The relayer stayed behind that
proxy until the outage. `demo/bench` paces it the same way by default.

**Relayer restart with nothing waiting.** The second restart read no
history, because its backward cursors had covered it since. It sent 97
requests in the first 10 s and then its usual 50. It re-checked the 23
deliveries it had submitted in the 10 minutes before on their original
schedule.

**Endpoint outage.** For 90 s the agents' Rusk ports refused connections,
as from a stopped node, while 6 EVM to Dusk transfers were sent. The agents
logged connection warnings (92 from the relayer, 27 or 28 from each
validator), kept running and needed no restart. In the first 10 s after the
ports reopened the relayer sent 73 requests and each validator 27, against
51 and 18 at idle with that history. The 6 messages were on Dusk 123 s
after the ports reopened, one every 20 s.

**Node restart.** Not run. `demo/start-env.sh` starts Rusk with `-s` and
the genesis archive, which unpacks into a new temporary directory each
time, so a restarted node is a new chain without the contracts, and no
demo script restarts a node on its existing state. The outage shows the
agents' side.

### What to change

In order of what they save:

1. **Pace the agents' Rusk requests and back off on 429 with jitter.** A
   relayer starting against a deployment with history goes far past the
   node's global limit, and three indexers that back off in step keep
   failing together. A client-side limit in `RuesClient` (100 contract
   queries a second held here), honouring `Retry-After` with random
   jitter, fixes that. An agent's own node can also raise
   `[http.policy.global_limits.classes.contract_query]`, but a public node
   keeps its default.
2. **Remember what is final.** Each indexer tick, and each validator
   checkpoint read, binary-searches every record's block height. Keeping the
   last finalized count and only checking the records after it would cost
   one or two lookups a tick whatever the history. At 100 messages, idle
   would fall from 632 to about 300 requests a minute, and a Dusk to EVM
   message would cost near the low end of its range above.
3. **Don't re-read history on start.** Start the backward cursors at the
   lowest sequence the database is missing, or give the Dusk indexers a
   range read that pages (`message_ids`, `gas_payments`) instead of
   looking up each record. At 32 contract queries per message of history,
   a restart after 100,000 messages would take about 5 hours at the 180 a
   second the node allows, and read gigabytes of archive pages.
4. **Keep the IGP index cheap.** The relayer needs it now that it
   enforces payments. It was a third of the relayer's idle lookups and 40
   of its 128 requests per Dusk to EVM message, nearly all binary searches
   that item 2 removes. The searches by dispatch transaction come from
   `txIdIndexingEnabled` (default true; the demo's relayer config turns it
   off), which a later run could measure without.
5. **Pipeline Dusk deliveries.** Submitting the next delivery before the
   last is in a block, with the account's next nonce (Rusk keeps up to 64
   future nonces per account), or batching deliveries, would lift the 3 a
   minute. Until then a relayer key delivers at most about 4,300 messages a
   day to Dusk.
6. **Cache `lastBlockPair` for a block.** Every indexer looks every 5 s,
   twice per 10 s block, and `lastBlockPair` is the most frequent request
   on a new deployment. Keeping it until the next block is due would cut
   the relayer's 36 a minute to about 6 and each validator's 12 to 6, a
   third of the idle requests on a new deployment.
7. **Smaller items.** The relayer simulates each Dusk delivery three times
   in a row (prepare, dry run, and the Mailbox's own `process`), and `dusk-tx`
   polls a transaction every 3 s where a block takes 10. Plan the relayer's
   log retention for about 60 MB a day at idle and 90 KiB per message at
   info level.
