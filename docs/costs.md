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
