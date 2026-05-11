# Hyperlane Dusk Contracts - Security Review & Changes

## Scope

All on-chain WASM contracts in `dusk/contracts/`:

| Contract | Path | Purpose |
|---|---|---|
| Mailbox | `contracts/mailbox/src/lib.rs` | Central dispatch/process hub |
| MerkleTreeHook | `contracts/merkle-tree-hook/src/lib.rs` | Incremental merkle tree for message indexing |
| MessageIdMultisigISM | `contracts/ism-multisig/src/lib.rs` | ECDSA-based message verification |
| ValidatorAnnounce | `contracts/validator-announce/src/lib.rs` | Validator storage location registry |
| ProtocolFee | `contracts/protocol-fee/src/lib.rs` | Fixed fee hook per dispatch |
| InterchainGasPaymaster | `contracts/igp/src/lib.rs` | Dynamic gas pricing hook |
| WarpDrc20 | `contracts/warp-drc20/src/lib.rs` | Synthetic DRC20 (mint/burn) |
| WarpDrc20Collateral | `contracts/warp-drc20-collateral/src/lib.rs` | Collateral DRC20 (lock/unlock) |
| WarpNative | `contracts/warp-native/src/lib.rs` | Native DUSK warp route (deposit/withdraw) |
| TestRecipient | `contracts/test-recipient/src/lib.rs` | Test-only handle() |
| TestMock | `contracts/test-mock/src/lib.rs` | NullISM + NoopHook for testing |

Also reviewed: `types/src/token_message.rs`, `types/src/message.rs`, `types/src/merkle.rs`, `demo/deploy.sh`, `demo/bridge.sh`

## Platform Context

Dusk contracts compile to `wasm32-unknown-unknown` and run on the `piecrust` VM inside `rusk`. Key platform properties relevant to security:

- **No EVM-style reentrancy**: Dusk VM executes contract calls synchronously within a single transaction. State is committed only on success. If any sub-call panics, the entire transaction reverts. There is no `call` opcode that returns control to the caller mid-execution.
- **Cross-contract calls via `abi::call`**: Type-safe, blocking. Panics propagate up the callstack.
- **Moonlight transactions**: Public (non-shielded) transactions with BLS key auth. The `deposit` field on a Moonlight TX is the mechanism for transferring DUSK from an account into a contract.
- **Transfer contract**: System contract that manages account balances. Contracts interact with it for deposits (`"deposit"`), withdrawals (`"contract_to_account"`), and balance queries.
- **`abi::caller()`**: Returns `Option<ContractId>` - the immediate calling contract. Returns `None` only in `direct_call` test mode.
- **`abi::public_sender()`**: Returns the BLS public key of the Moonlight TX signer. Only available within a Moonlight TX context.
- **Release WASM builds**: Rust release builds for WASM do NOT panic on integer overflow. Arithmetic wraps silently. This is different from debug builds where overflow panics. This means explicit checked arithmetic is mandatory for security-sensitive operations.

## Issues Found & Fixes Applied

### HIGH-1: WarpNative `transfer_remote` did not verify DUSK deposit

**File**: `contracts/warp-native/src/lib.rs`, `transfer_remote()` (line 160-201)

**Before**: `transfer_remote(destination, recipient, amount)` took `amount` as a parameter but had no mechanism to verify that the caller actually deposited that amount of DUSK into the contract. A user could call `transfer_remote` with `amount: 1_000_000` but include `deposit: 0` in their Moonlight TX. The contract would emit a cross-chain message claiming 1M DUSK was locked, and the remote chain would mint 1M tokens, while zero DUSK was actually deposited. This is a fund-draining vulnerability.

**After**: Added explicit deposit verification using the Dusk transfer contract pattern:

```rust
let _: () = abi::call(TRANSFER_CONTRACT, "deposit", &amount)
    .expect("WarpNative: deposit failed — TX deposit must equal amount");
```

The transfer contract's `deposit()` function (in `rusk/contracts/transfer/src/state.rs:330-358`) validates:
1. There is an active `Deposit::Available { sender, target, value }` in the transitory state
2. `value == amount` (exact match, no partial deposits)
3. `target == caller` (the contract calling deposit must be the intended recipient)

If any check fails, the transfer contract panics, which propagates up and reverts the entire transaction.

**Trade-off**: The transfer contract's `deposit()` requires exact match (`deposit_value == requested_amount`). This means a user cannot "over-deposit" (e.g., deposit 100 DUSK but only bridge 50). This is intentional — it prevents confusion about where excess funds go. The Solidity `HypNative` has the same pattern via `msg.value`.

**Trade-off**: This fix cannot be integration-tested with `direct_call` because there's no Moonlight TX context in `direct_call` mode (the transfer contract's transitory deposit state is only populated during `execute()` of a real Moonlight TX). Full coverage requires e2e testing against a live rusk node. The deposit path is well-tested in Dusk's own test suite (the Charlie contract pattern) so we trust the transfer contract's behavior.

### HIGH-2: WarpNative `handle` panicked for unregistered recipients

**File**: `contracts/warp-native/src/lib.rs`, `handle()` (line 212-261)

**Before**: When an inbound cross-chain message arrived with a recipient H256 that had no registered BLS key, the contract panicked with `"WarpNative: recipient not registered"`. This caused the entire Mailbox.process() transaction to revert. The message would be marked as undelivered and the relayer would retry indefinitely, burning gas on every attempt. The funds were stuck — they existed on the source chain as a burn/lock, but could never be delivered on Dusk.

**After**: Introduced an escrow pattern. If the recipient H256 is not registered, funds are held in a `pending_transfers: BTreeMap<H256, u64>` mapping. The recipient can later:
1. Call `register_account()` to register their BLS key
2. Call `claim_pending()` to transfer escrowed DUSK to their account

New state field:
```rust
pending_transfers: BTreeMap<H256, u64>,
```

New public methods:
```rust
pub fn claim_pending(&mut self)       // Transfers escrowed DUSK to caller's account
pub fn pending_balance(&self, h: H256) -> u64  // Query escrow balance
```

**Trade-off**: This adds storage that grows with the number of unique unregistered recipients. In practice this is bounded — each unique H256 is 32 bytes key + 8 bytes value = 40 bytes. The alternative (panic-and-retry) was strictly worse. The escrow is cleaned up when `claim_pending` is called (the entry is `remove()`d).

**Trade-off**: `claim_pending` uses `abi::public_sender()` to determine the caller's BLS key, then computes `keccak256(pk.to_bytes())` to look up pending transfers. This means the caller must use the same BLS key that was used as the recipient H256 on the source chain. If a user bridges to `keccak256(pk_A)` but tries to claim with `pk_B`, the claim will fail. This is the correct behavior — it prevents theft.

**Trade-off**: Between `register_account` and `claim_pending`, there is a two-step process. We chose NOT to auto-register during `claim_pending` to keep the registration explicit and because `register_account` is also needed for future inbound transfers (not just pending ones). The user could call both in consecutive blocks.

**What was NOT changed**: The registered-path (`contract_to_account`) still panics if the transfer contract rejects the transfer (e.g., contract has insufficient balance). This is correct — if the contract doesn't have enough DUSK locked, something is fundamentally broken.

### HIGH-3: WarpDrc20Collateral had no External account support

**File**: `contracts/warp-drc20-collateral/src/lib.rs` (entire account registration section, lines 160-179) and `handle()` (lines 262-269)

**Before**: The collateral contract's `handle()` always treated the recipient H256 as a `ContractId`:
```rust
let recipient = Account::Contract(ContractId::from_bytes(msg.recipient));
```
If the recipient was an external Dusk account (identified by `keccak256(bls_public_key)`), the unlock would go to a non-existent contract address, effectively burning the tokens.

WarpDrc20 and WarpNative already had `register_account` / `registered_accounts` support, but WarpDrc20Collateral was missing it entirely.

**After**: Added the full account registration pattern (identical to WarpDrc20/WarpNative):

```rust
registered_accounts: BTreeMap<H256, AccountPublicKey>,

pub fn register_account(&mut self) { ... }
pub fn is_registered(&self, h: H256) -> bool { ... }
```

Updated `handle()` to resolve the recipient:
```rust
let recipient_account =
    if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
        Account::External(*pk)
    } else {
        Account::Contract(ContractId::from_bytes(msg.recipient))
    };
```

Also added `dusk-bytes = "0.1.7"` to `Cargo.toml` (needed for `Serializable` trait on `AccountPublicKey`).

**Trade-off**: Unlike WarpNative (HIGH-2), the collateral contract does NOT have an escrow pattern for unregistered recipients. If tokens are sent to an unregistered H256 that is not a valid contract, the DRC20 `transfer` call will succeed (DRC20 doesn't validate contract existence), but the tokens will sit in an `Account::Contract(some_address)` that nobody controls. This is the same behavior as ERC20 transfers to non-existent addresses on EVM. We chose not to add escrow here because:
1. The collateral contract delegates token transfers to the wrapped DRC20 via `abi::call(wrapped_token, "transfer", ...)`. Adding escrow would mean holding DRC20 tokens in a separate internal ledger, duplicating accounting logic.
2. The fix ensures the happy path works (registered external accounts get tokens). The edge case (unregistered H256 that isn't a contract) is a user error, same as sending ERC20 to a wrong address.

**Potential follow-up**: Consider adding an escrow pattern similar to WarpNative for the collateral contract. This would require the collateral to hold DRC20 tokens in its own balance and release them on claim, rather than calling `transfer` immediately. This is more complex because it involves cross-contract DRC20 accounting.

### HIGH-4: WarpDrc20 arithmetic overflow in `mint` and `do_transfer`

**File**: `contracts/warp-drc20/src/lib.rs`, `mint()` (lines 409-418) and `do_transfer()` (lines 395-406)

**Before**: Both functions used plain `+=` for balance and supply updates:
```rust
fn mint(&mut self, account: Account, amount: u64) {
    *self.balances.entry(account).or_insert(0) += amount;
    self.supply += amount;
}
```
In release WASM builds, `u64` overflow wraps silently (Rust release mode does not panic on overflow for `wasm32-unknown-unknown`). An attacker could mint tokens in a way that wraps the balance or supply counter back to a small number, breaking accounting invariants.

**After**: All additions now use `checked_add`:
```rust
fn mint(&mut self, account: Account, amount: u64) {
    let balance = self.balances.entry(account).or_insert(0);
    *balance = balance.checked_add(amount).expect("WarpDrc20: balance overflow");
    self.supply = self.supply.checked_add(amount).expect("WarpDrc20: supply overflow");
}

fn do_transfer(&mut self, from: Account, to: Account, value: u64) {
    let from_balance = self.balances.get(&from).copied().unwrap_or(0);
    assert!(from_balance >= value, "WarpDrc20: insufficient balance");
    *self.balances.entry(from).or_insert(0) -= value;
    let to_balance = self.balances.entry(to).or_insert(0);
    *to_balance = to_balance.checked_add(value).expect("WarpDrc20: balance overflow");
}
```

**Trade-off**: `checked_add` adds a branch per operation. In WASM this is negligible — a single comparison and conditional branch, costing a few gas units at most. The alternative (wrapping) is unacceptable.

**Note**: `burn()` uses subtraction (`-= amount`) which is safe because it's preceded by `assert!(balance >= amount)`. The subtraction cannot underflow given the assert passes. We did not change `burn()`.

**Note**: `supply -= amount` in `burn()` also cannot underflow in practice because supply >= balance >= amount. However, if there were ever a bug in the minting logic that desynced supply from balances, this could theoretically underflow. We did not add `checked_sub` here because it would mask a more fundamental invariant violation that should be caught earlier.

### MEDIUM-1: IGP `quote_gas_payment` used `cost as u64` truncation

**File**: `contracts/igp/src/lib.rs`, `quote_gas_payment()` (line 153)

**Before**:
```rust
let cost = adjusted_gas * gas_price * exchange_rate / TOKEN_EXCHANGE_RATE_SCALE;
cost as u64
```
The intermediate calculation is done in `u128`, but the final cast `as u64` silently truncates if the result exceeds `u64::MAX`. With adversarial gas oracle configs (e.g., very high `gas_price` or `token_exchange_rate`), the fee could be calculated as a huge number but truncated to near-zero, letting messages through for free.

**After**:
```rust
u64::try_from(cost).expect("IGP: fee exceeds u64")
```

Now panics if the fee doesn't fit in `u64`, which prevents under-charging. The gas oracle admin would need to fix their config.

**Trade-off**: A panic here means the message cannot be dispatched if the calculated fee is absurdly large. This is the correct behavior — it's better to reject the dispatch than to under-charge. The admin can fix the gas oracle config.

### MEDIUM-2: ProtocolFee and IGP accounting overflow

**Files**: `contracts/protocol-fee/src/lib.rs` line 98, `contracts/igp/src/lib.rs` line 104

**Before**: Both used `+=` for their accounting accumulators:
```rust
// ProtocolFee
self.collected_fees += self.protocol_fee;

// IGP
self.total_gas_payments += payment;
```
These accumulators are append-only counters tracking total fees/payments over the lifetime of the contract. With enough dispatches, they could wrap in release builds.

**After**: Changed to `saturating_add`:
```rust
// ProtocolFee
self.collected_fees = self.collected_fees.saturating_add(self.protocol_fee);

// IGP
self.total_gas_payments = self.total_gas_payments.saturating_add(payment);
```

**Trade-off**: We chose `saturating_add` over `checked_add` here because these are accounting-only counters. They are not used in any security-critical logic — no funds are gated on these values. If they saturate at `u64::MAX`, the only consequence is that the counter stops incrementing, which is an acceptable degradation for a field that would require ~18.4 quintillion dispatches to overflow. Using `checked_add` would panic and prevent message dispatch, which is worse.

### MEDIUM-3: MessageIdMultisigISM accepted malformed signature metadata shape

**File**: `contracts/ism-multisig/src/lib.rs`, `verify()`

**Before**: The verifier computed signature count using integer division:

```rust
let sig_count = (metadata.len() - SIGNATURES_OFFSET) / SIGNATURE_LENGTH;
```

This ignored trailing bytes after the last full 65-byte signature. Verification
only consumed the first `threshold` full signatures, so malformed metadata with
partial trailing signature data could be accepted if the threshold signatures
were otherwise valid. The verifier also did not explicitly reject an empty
validator/threshold state before metadata parsing. That state should not be
reachable through normal Dusk deployment because the init args are required,
but the runtime check makes the invariant explicit.

**After**: `verify()` now rejects uninitialized state and requires the signature
section length to be an exact multiple of 65 bytes before signature counting:

```rust
assert!(
    self.threshold > 0 && !self.validators.is_empty(),
    "MultisigISM: not initialized"
);
assert!(
    (metadata.len() - SIGNATURES_OFFSET) % SIGNATURE_LENGTH == 0,
    "MultisigISM: metadata signature length mismatch"
);
```

**Trade-off**: This is intentionally stricter than silently ignoring trailing
metadata. Valid Hyperlane multisig metadata is unchanged because validator
signatures are fixed-width.

### LOW-1: WarpDrc20 `transfer_remote` accepted zero amounts

**File**: `contracts/warp-drc20/src/lib.rs`, `transfer_remote()` (line 245)

**Before**: A user could call `transfer_remote` with `amount: 0`. This would burn 0 tokens (no-op), dispatch a cross-chain message, and the remote chain would mint 0 tokens. While not directly exploitable, it wastes gas on both chains and pollutes event logs.

**After**: Added input validation:
```rust
assert!(amount > 0, "WarpDrc20: amount must be > 0");
```

Also added the same check to WarpNative's `transfer_remote` (line 166).

**Trade-off**: None. This is strictly defensive. The assert is placed before any other logic so it fails fast.

## Issues Considered But Not Changed

### Mailbox unbounded storage growth

The Mailbox stores all dispatched messages in `dispatched_messages: Vec<Vec<u8>>` and all processed message IDs in `processed_ids: Vec<MessageId>`. These grow unboundedly. In the current architecture, these are needed for off-chain agent indexing (the Hyperlane relayer reads `dispatched_message(nonce)` to get messages to relay).

**Why not fixed**: This is an architectural choice inherited from the Solidity Mailbox, which also stores messages. The Dusk VM charges gas proportional to state size changes, which provides natural backpressure. A proper fix would require a data availability layer or event-based indexing, which is out of scope for a security fix.

### Mailbox `dispatch` is callable by anyone

Any contract or account can call `Mailbox.dispatch()`. This is by design — Hyperlane is a permissionless messaging protocol. The security model relies on the ISM (Interchain Security Module) at the receiving end to verify message authenticity, not on restricting who can dispatch.

### ISM replay protection

The Mailbox's `process()` marks messages as delivered before calling the ISM and recipient:
```rust
self.delivered.insert(id, DeliveryRecord { ... });
// ... then verify and handle
```
This follows checks-effects-interactions and prevents replay. A message ID cannot be processed twice because `delivered.contains_key(&id)` is checked at the top of `process()`.

### Reentrancy

Dusk VM does not support reentrancy. When contract A calls contract B, contract A's execution is suspended until B returns. B cannot call back into A during the same transaction. This eliminates an entire class of vulnerabilities.

## Files Modified

| File | Change |
|---|---|
| `contracts/warp-native/src/lib.rs` | Added deposit verification, escrow pattern (`pending_transfers`, `claim_pending`, `pending_balance`) |
| `contracts/warp-drc20/src/lib.rs` | `checked_add` in `mint`/`do_transfer`, zero-amount check in `transfer_remote` |
| `contracts/warp-drc20-collateral/src/lib.rs` | Added `registered_accounts`, `register_account`, `is_registered`, recipient resolution in `handle` |
| `contracts/warp-drc20-collateral/Cargo.toml` | Added `dusk-bytes = "0.1.7"` dependency |
| `contracts/igp/src/lib.rs` | `u64::try_from(cost).expect(...)` instead of `cost as u64`; `saturating_add` for accounting |
| `contracts/ism-multisig/src/lib.rs` | Reject uninitialized verification state and partial trailing signature metadata |
| `contracts/protocol-fee/src/lib.rs` | `saturating_add` for `collected_fees` |
| `tests/tests/integration.rs` | 13 new security tests (60 total, up from 47) |
| `demo/deploy.sh` | Conditional `register_account` on collateral/native warp routes |

## Test Coverage for Security Fixes

| Test | What it validates |
|---|---|
| `test_warp_drc20_transfer_remote_rejects_zero_amount` | Zero-amount assertion fires before any state mutation |
| `test_warp_native_handle_escrows_unregistered_recipient` | Unregistered recipient goes to escrow instead of panicking; `pending_balance` returns correct amount |
| `test_warp_native_escrow_accumulates` | Multiple inbound messages to same unregistered recipient accumulate correctly |
| `test_warp_native_claim_pending_requires_pending` | `claim_pending` panics if no pending balance exists |
| `test_warp_collateral_register_account` | Registration round-trip: `is_registered` returns false before, true after |
| `test_warp_collateral_handle_resolves_registered_external` | End-to-end: pre-fund collateral with DRC20, register BLS key, process inbound message, verify DRC20 tokens unlock to External account |
| `test_multisig_ism_init_rejects_no_validators` | Init fails if validator set is empty |
| `test_multisig_ism_init_rejects_invalid_threshold` | Init fails if threshold exceeds validator count |
| `test_multisig_ism_init_rejects_unsorted_validators` | Init fails if validator list is not strictly sorted |
| `test_multisig_ism_verify_rejects_short_metadata` | Verify fails before parsing metadata shorter than the fixed header |
| `test_multisig_ism_verify_rejects_partial_signature_bytes` | Verify fails when signature metadata has trailing partial bytes |
| `test_multisig_ism_verify_rejects_insufficient_signatures` | Verify fails when metadata contains fewer signatures than threshold |
| `test_multisig_ism_admin_rejects_unauthorized_caller` | Validator-set admin update is owner-gated |

### Test Gaps

| Gap | Why |
|---|---|
| WarpNative `transfer_remote` deposit verification | Cannot test with `direct_call` — requires real Moonlight TX with `deposit` field. The transfer contract's deposit validation logic is tested in Dusk's own test suite. Needs e2e test against live rusk. |
| WarpNative `claim_pending` happy path (actual DUSK transfer) | Requires the contract to hold DUSK balance, which requires a prior successful `transfer_remote` with real deposit. Same limitation as above. |
| IGP `u64::try_from` panic path | Would need gas oracle config that produces a fee > `u64::MAX`. The `checked_mul` calls before it would panic first in practice. |
| WarpDrc20 `checked_add` overflow panic path | Would need to mint > `u64::MAX` tokens, which requires > `u64::MAX` inbound messages. Not practically testable. |

## Open Questions for Further Review

1. **WarpDrc20 `burn()` subtraction**: `self.supply -= amount` in `burn()` does not use `checked_sub`. If supply somehow became desynced from the sum of balances (e.g., due to a bug in minting), this could wrap. Should it use `checked_sub`?

2. **WarpDrc20Collateral escrow**: Should WarpDrc20Collateral have an escrow pattern for unregistered recipients (like WarpNative), or is the current "treat as Contract address" fallback acceptable?

3. **WarpDrc20 `only_owner` uses `public_sender()`**: The owner check in WarpDrc20 uses `abi::public_sender()` (BLS key from Moonlight TX), while WarpNative and WarpDrc20Collateral use `abi::caller()` (contract ID). This means WarpDrc20 admin functions can only be called via direct Moonlight TX, not from other contracts. Is this intentional asymmetry acceptable?

4. **Mailbox `resolve_sender` when called from transfer contract**: When a Moonlight TX targets the Mailbox directly, `abi::caller()` returns `TRANSFER_CONTRACT`. The Mailbox special-cases this to derive the sender from `public_sender()`. But if the transfer contract ever calls the Mailbox for non-user-initiated reasons, this would misattribute the sender. Is this a concern?

5. **`registered_accounts` has no deregistration**: Once a BLS key is registered, it cannot be unregistered or updated. If a user's key is compromised, they cannot re-register with a new key for the same H256 (since H256 = keccak256(pk) is deterministic). Is this acceptable?

6. **WarpNative `pending_transfers` has no admin drain**: If a user loses their private key after funds are escrowed, the funds are locked forever. There is no admin function to recover stuck escrow. Is this intentional?

## Build & Test Verification

```
# All 11 contract WASMs compile
make all    # in dusk/ directory

# 28 unit tests pass
cargo test -p hyperlane-dusk-types

# 60 integration tests pass (47 pre-existing + 13 new)
cargo test -p hyperlane-dusk-integration-tests
```

Agent-based local E2E evidence is tracked in `TEST_REPORT.md`, including exact
commands, commit SHAs, and `/tmp` log/artifact paths for:

- EVM -> Dusk and Dusk -> EVM using the TestMock/null-style ISM.
- EVM -> Dusk and Dusk -> EVM using MessageIdMultisigISM with validator output.
- Dirty redeploy refusal against a non-reset local Dusk chain.
- Relayer restart/backlog recovery with 5 EVM -> Dusk and 5 Dusk -> EVM
  transfers.
