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
- **Release WASM builds**: Rust release builds normally do not panic on integer
  overflow unless overflow checks are enabled. This workspace explicitly sets
  `[profile.release] overflow-checks = true` in `Cargo.toml` for contract
  safety. Security-sensitive arithmetic still uses explicit checked operations
  so the invariant does not rely only on a workspace profile setting, and so
  the intended error is clear to reviewers and operators.

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

**Validation**: The current-Rusk VM harness can execute a public Moonlight call
with a transaction deposit. `test_warp_native_roundtrip_moves_real_dusk`
asserts that an exact deposit moves DUSK into WarpNative custody and that a
registered inbound transfer moves it back out. A second test proves an
amount/deposit mismatch reverts with zero residual contract balance. A live
cross-chain WarpNative agent E2E is still desirable, but the transfer-contract
custody invariant is no longer inferred only from Dusk's own tests.

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

### HIGH-3: WarpDrc20Collateral had no External account escrow

**File**: `contracts/warp-drc20-collateral/src/lib.rs`

**Before**: The collateral contract's `handle()` always treated the recipient H256 as a `ContractId`:
```rust
let recipient = Account::Contract(ContractId::from_bytes(msg.recipient));
```
If the recipient was an external Dusk account (identified by `keccak256(bls_public_key)`), the unlock would go to a non-existent contract address, effectively burning the tokens.

WarpDrc20 and WarpNative already had `register_account` / `registered_accounts`
support, but WarpDrc20Collateral was missing the registration and escrow path.

**After**: Added the full account registration pattern and a pending-transfer
escrow path:

```rust
registered_accounts: BTreeMap<H256, AccountPublicKey>,
pending_transfers: BTreeMap<H256, u64>,

pub fn register_account(&mut self) { ... }
pub fn is_registered(&self, h: H256) -> bool { ... }
pub fn claim_pending(&mut self) { ... }
pub fn pending_balance(&self, h: H256) -> u64 { ... }
```

Updated `handle()` to deliver registered recipients immediately and escrow
unregistered recipients:
```rust
if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
    abi::call(self.wrapped_token, "transfer", &(Account::External(*pk), msg.amount))
        .expect("WarpCollateral: transfer failed");
} else {
    let pending = self.pending_transfers.entry(msg.recipient).or_insert(0);
    *pending = pending.checked_add(msg.amount).expect("WarpCollateral: pending overflow");
}
```

Also added `dusk-bytes = "0.1.7"` to `Cargo.toml` (needed for `Serializable` trait on `AccountPublicKey`).

**Trade-off**: Unregistered collateral recipients now require a second step:
the recipient must call `register_account()` and then `claim_pending()`. This
keeps tokens in the collateral contract's DRC20 balance instead of transferring
to a possibly nonexistent `ContractId`, which is safer for Dusk's BLS-account
addressing model.

### HIGH-4: WarpDrc20 arithmetic overflow in `mint`, `burn`, and `do_transfer`

**File**: `contracts/warp-drc20/src/lib.rs`, `mint()`, `burn()`, and `do_transfer()`

**Before**: Both functions used plain `+=` for balance and supply updates:
```rust
fn mint(&mut self, account: Account, amount: u64) {
    *self.balances.entry(account).or_insert(0) += amount;
    self.supply += amount;
}
```
With Rust's default release profile, `u64` overflow wraps silently. This
workspace enables release overflow checks, but relying only on a profile
setting would make the contract safety property harder to audit and easier to
break through an alternate build path. An attacker should never be able to mint
tokens in a way that wraps the balance or supply counter back to a small
number, because that would break accounting invariants.

**After**: Additions use `checked_add`; supply burn uses `checked_sub`:
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

fn burn(&mut self, account: Account, amount: u64) {
    let balance = self.balances.get(&account).copied().unwrap_or(0);
    assert!(balance >= amount, "WarpDrc20: insufficient balance to burn");
    *self.balances.entry(account).or_insert(0) -= amount;
    self.supply = self.supply.checked_sub(amount).expect("WarpDrc20: supply underflow");
}
```

**Trade-off**: `checked_add` adds a branch per operation. In WASM this is negligible — a single comparison and conditional branch, costing a few gas units at most. The alternative (wrapping) is unacceptable.

**Note**: The balance subtraction in `burn()` remains guarded by
`assert!(balance >= amount)`. The supply subtraction also uses `checked_sub` so
a future accounting invariant bug cannot wrap supply downward in release WASM.

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
These accumulators are append-only counters tracking total fees/payments over
the lifetime of the contract. With enough dispatches, they could wrap under
Rust's default release arithmetic semantics, or become dependent on the
workspace release profile rather than a local contract invariant.

**After**: Changed to `checked_add`:
```rust
// ProtocolFee
self.collected_fees = self
    .collected_fees
    .checked_add(self.protocol_fee)
    .expect("ProtocolFee: collected fee overflow");

// IGP
self.total_gas_payments = self
    .total_gas_payments
    .checked_add(payment)
    .expect("IGP: total gas payment overflow");
```

**Trade-off**: Earlier review accepted `saturating_add` because the fields are
informational counters. The production-hardening pass now fails closed instead:
a lifetime accounting total that cannot represent the next payment rejects the
dispatch rather than silently pinning at `u64::MAX`. The overflow point still
requires pathological settings or practically unreachable volume, but explicit
rejection is easier for operators and auditors to reason about.

**Related query hardening**: `Mailbox::processed_count()` and
`InterchainGasPaymaster::gas_payment_count()` now use
`u32::try_from(...).expect(...)` instead of `len() as u32`, avoiding silent
count truncation if a future deployment ever exceeds the `u32` query surface.

### MEDIUM-2B: Mailbox quote total fee overflow

**File**: `contracts/mailbox/src/lib.rs`, `quote_dispatch()`

**Before**: The Mailbox summed required-hook and selected-hook quotes with plain
`u64` addition:

```rust
required_fee + hook_fee
```

Under Rust's default release arithmetic semantics, this could wrap silently if
a required hook and default or custom hook returned a combined fee above
`u64::MAX`, causing callers to see an under-quoted fee. Even with workspace
release overflow checks enabled, checked addition makes the failure mode
explicit and local to the Mailbox quote invariant.

**After**: The combined quote now uses checked addition:

```rust
required_fee
    .checked_add(hook_fee)
    .expect("Mailbox: fee overflow")
```

**Trade-off**: A pathological hook configuration now rejects the quote instead
of returning a wrapped value. This matches the IGP quote overflow policy: fail
closed rather than under-charge.

### MEDIUM-2C: Mailbox nonce overflow

**File**: `contracts/mailbox/src/lib.rs`, `dispatch()`

**Before**: The Mailbox incremented its Hyperlane message nonce with plain
`u32` addition:

```rust
self.nonce += 1;
```

The Hyperlane wire format stores the nonce as 4 bytes. Under Rust's default
release arithmetic semantics, plain `u32` overflow can wrap silently. After
`u32::MAX` dispatches, a wrapped nonce would reuse origin nonce space and could
repeat message IDs for identical sender/destination/recipient/body tuples.

**After**: Dispatch now fails closed on nonce exhaustion:

```rust
self.nonce = self
    .nonce
    .checked_add(1)
    .expect("Mailbox: nonce overflow");
```

**Trade-off**: A Mailbox with exhausted 32-bit nonce space must be redeployed
and routing/config migrated. This matches the protocol's fixed-width nonce
encoding better than silently wrapping.

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

### Event surface

Forge 0.3 declares the event surface once at the contract module level. The
contracts list their protocol and operational events with
`#[dusk_forge::contract(events = [...])]`, and the shared event types implement
Forge's `ContractEvent` trait:

- Mailbox `dispatch`, `dispatch_default`, `process`, `set_default_ism`,
  `set_default_hook`, `set_required_hook`, initialization, ownership transfer,
  and ownership renunciation.
- MerkleTreeHook initialization and `post_dispatch`.
- ProtocolFee initialization, fee updates, beneficiary updates, ownership
  transfer, and `post_dispatch`.
- InterchainGasPaymaster initialization, domain gas config updates,
  beneficiary updates, ownership transfer, and `post_dispatch`.
- MessageIdMultisigISM initialization and validator-set/threshold updates.
- ValidatorAnnounce initialization and announcements.
- WarpDrc20, WarpDrc20Collateral, and WarpNative initialization, account
  registration, remote-router enrollment, hook/ISM updates, ownership transfer,
  and remote send/receive paths.
- WarpNative pending-transfer claims.
- WarpDrc20 balance changes through regular transfer, remote-send burn, and
  remote-receive mint paths.

Initial operational config now emits the same config events used for later
updates: Mailbox initial hook/ISM values, ProtocolFee initial fee/beneficiary,
IGP initial domain gas configs and beneficiary, MessageIdMultisigISM initial
validator set, and initially enrolled warp routers.

Test contracts use the same module-level event declaration model. The removed
per-method `emits` and `no_event` attributes belong to the pre-0.3 Forge API.

### Hook fee collection is not implemented

`ProtocolFee` and `InterchainGasPaymaster` currently quote fees, increment
counters, and emit payment events, but neither contract collects or transfers
native DUSK. `Mailbox.dispatch` likewise does not claim a transaction deposit
or forward funds to the configured hooks. Each hook's `post_dispatch` method
can also be called directly, so an arbitrary caller can create an unbacked
accounting record.

This means the current fee surface is useful only as a functional model. It is
not production fee enforcement. A production design must define Dusk-native
deposit/forwarding semantics and bind each payment record to authenticated,
actually escrowed value before the fee counters or IGP events are trusted.
Hyperlane's Solidity `postDispatch` hook surface is also public, so simply
restricting the Dusk method to Mailbox would not by itself reproduce the value
semantics. DuskEVM's explicit router escrow/preload, exact consumption, and
post-call clear/refund pattern is a useful model. Any transfer-contract callback
must authenticate the callback context as well as the immediate caller.

The demo deployment does not currently exercise this fee model: Mailbox uses
MerkleTreeHook as its required hook and TestMock as its default hook. ProtocolFee
and IGP are deployed but not wired into dispatch.

### Deployed owner roles are not reachable

Mailbox, ProtocolFee, IGP, WarpNative, and WarpDrc20Collateral store their
owner as a `ContractId`, and MessageIdMultisigISM applies the same immediate-
contract-caller model to its 32-byte owner. Deployment initializes Mailbox with
itself as owner and the other contracts with Mailbox as owner, but Mailbox has
no admin-forwarding entrypoint. A Moonlight account therefore cannot call these
admin methods, and Mailbox cannot currently issue the required calls either.

WarpDrc20 is different: it stores an `H256` owner and resolves either the
Moonlight public sender or an inter-contract caller. DuskEVM's discriminated
external-account/contract address model provides a better template for a
consistent Hyperlane owner principal. This needs a deliberate ABI/state and
deployment migration; changing only the initialization arguments would either
leave roles frozen or weaken caller authentication.

### Placeholder panic scan

The production contract/runtime/tooling paths were scanned for placeholder or
direct panic macros:

```bash
rg -n "todo!|unimplemented!|panic!" contracts types data-driver dusk-tx e2e wasm-bindings demo -g '!target'
```

No matches remain. Mailbox sender resolution previously used a direct
`panic!("Mailbox: cannot determine sender")`; it now uses the same explicit
`expect("Mailbox: cannot determine sender")` revert style as the rest of the
contracts. Other contract reverts still intentionally use `assert!` and
`expect(...)` for invariant checks and failed external calls.

Production contract source is also scanned by `make gate-status` for direct
`.unwrap()` calls. WarpNative, WarpDrc20, and WarpDrc20Collateral inbound router
checks now use pattern matching instead of guarded `unwrap()` calls, so a future
unguarded or weakly-guarded unwrap is reported by the review gate.

## Security Assumptions and Solidity Deviations

These Dusk contracts intentionally do not attempt to be byte-for-byte Solidity
ports. The current implementation relies on the following assumptions and
documented deviations:

| Area | Dusk assumption/deviation | Security implication |
|---|---|---|
| Account model | Dusk uses BLS Moonlight senders and `ContractId`; there is no direct `msg.sender` equivalent. Contracts resolve direct Moonlight calls through the transfer contract to `keccak256(public_sender)` and resolve inter-contract calls to the immediate caller `ContractId`. | WarpDrc20 can represent either form through `H256`. The `ContractId`-only owner contracts cannot be administered by the deployed Moonlight account, and their Mailbox/self owners have no forwarding path. A shared explicit principal model is a release blocker. |
| Native value transfer | WarpNative uses the Dusk transfer contract's exact `deposit` check instead of Solidity `msg.value`. | Current-Rusk VM tests now execute Moonlight deposits and assert real transfer-contract custody, exact-match rollback, and registered-account release. Live cross-chain WarpNative remains outside the agent E2E route matrix. |
| Reverts | Dusk contract errors are explicit `assert!`/`expect(...)` panics that revert the full transaction. | This matches Dusk VM behavior but differs from Solidity custom errors. Error strings are part of test evidence and should stay stable enough for diagnostics. |
| Upgradeability | No proxy or in-place upgrade pattern is implemented for the Dusk contracts. Deterministic contract IDs are treated as immutable deployment identities. | Production upgrades require new deployments and routing/config migration. Dirty redeploy refusal is intentional and tested. |
| Events/indexing | Contracts declare protocol and operational events through Forge 0.3 module-level metadata. | Off-chain agents should rely on the exposed query surfaces and declared events documented here. |
| Address mapping | External Dusk recipients are represented by `keccak256(bls_public_key_bytes)` and must register their BLS public key on Dusk for account delivery. | The mapping is deterministic and non-updatable. Lost or compromised keys are a user/account-management issue, not recoverable by current contracts. |
| Unregistered recipients | WarpNative and WarpDrc20Collateral escrow unregistered recipients. | Inbound funds are not stranded at a synthetic contract account. Recipients must register the matching BLS key and call `claim_pending()`. |
| Multisig metadata | MessageIdMultisigISM requires sorted validator sets, a valid threshold, initialized state, and exact fixed-width signature metadata. | This is stricter than accepting trailing metadata bytes and is intended to prevent malformed metadata acceptance. |
| Fee accounting | ProtocolFee and IGP arithmetic fails closed on overflow, but the hooks do not collect DUSK and public `post_dispatch` calls can record payment without value. | Fee enforcement and payment indexing are not production-ready. Public hooks can be valid, but records need authenticated escrow/consumption semantics; the demo does not wire these hooks into Mailbox dispatch. |
| Secret handling | Demo/E2E configs use local dev keys and `/tmp` artifacts. `dusk-tx` supports `DUSK_CONSENSUS_PASSWORD_FILE`, password environment variables, and `--secret-key-stdin`; demo scripts no longer pass Dusk consensus passwords through CLI argv. `SECRET_HANDLING.md` and `make secret-hygiene` add source/artifact guardrails. Production use must still avoid logs, committed config, and CI artifact leakage for Dusk secrets. | This is a release gate outside the WASM contracts. Current scripts are acceptable only for local deterministic dev/test environments, and production signer storage/config generation needs operational sign-off. |

## Files Modified

| File | Change |
|---|---|
| `contracts/warp-native/src/lib.rs` | Added deposit verification, escrow pattern (`pending_transfers`, `claim_pending`, `pending_balance`) |
| `Cargo.toml` | Workspace release profile enables `overflow-checks = true`; contract code still uses explicit checked arithmetic for security-sensitive invariants |
| `contracts/warp-drc20/src/lib.rs` | `checked_add` in `mint`/`do_transfer`, `checked_sub` in `burn`, zero-amount check in `transfer_remote`, immediate-caller-aware owner resolution |
| `contracts/warp-drc20-collateral/src/lib.rs` | Added `registered_accounts`, `register_account`, `is_registered`, collateral escrow, `claim_pending`, `pending_balance`, and registered-recipient resolution in `handle` |
| `contracts/warp-drc20-collateral/Cargo.toml` | Added `dusk-bytes = "0.1.7"` dependency |
| `contracts/igp/src/lib.rs` | `u64::try_from(cost).expect(...)` instead of `cost as u64`; checked total gas payment accounting; checked `gas_payment_count` conversion |
| `contracts/ism-multisig/src/lib.rs` | Reject uninitialized verification state and partial trailing signature metadata |
| `contracts/protocol-fee/src/lib.rs` | Checked `collected_fees` accounting |
| `types/src/events.rs` | Added operational/admin, account registration, gas config, validator-set, pending-claim, and WarpDrc20 transfer events |
| `contracts/mailbox/src/lib.rs` | Explicit event annotations for dispatch/process, initialization, Mailbox hook/ISM setter, and ownership events; checked total-fee quotes; checked nonce increment; checked `processed_count` conversion |
| `contracts/merkle-tree-hook/src/lib.rs` | Explicit event annotations for initialization and Merkle insertion events |
| `contracts/validator-announce/src/lib.rs` | Explicit event annotations for initialization and validator announcement events |
| `contracts/warp-native/src/lib.rs` | Explicit event annotations for initialization, registration, pending claims, config/ownership, and remote send/receive events |
| `contracts/warp-drc20/src/lib.rs` | Explicit event annotations for initialization, registration, token transfer/mint/burn, config/ownership, and remote send/receive events |
| `contracts/warp-drc20-collateral/src/lib.rs` | Explicit event annotations for initialization, registration, config/ownership, and remote send/receive events |
| `tests/tests/integration.rs` | 27 new security tests (74 total, up from 47), including real-DUSK WarpNative custody and mismatch rollback |
| `tests/tests/test_session.rs` | Added Moonlight calls with deposits and transfer-contract custody queries |
| `demo/start-env.sh` | Uses an explicit state archive and consensus-key path, refuses mismatched contract/node Rusk checkouts, and avoids explorer assets when the explorer is skipped |
| `demo/stop-env.sh` | Stops only the Rusk process using the demo's exact state archive |
| `demo/deploy.sh` | Conditional `register_account` on collateral/native warp routes |

## Test Coverage for Security Fixes

| Test | What it validates |
|---|---|
| `test_warp_drc20_transfer_remote_rejects_zero_amount` | Zero-amount assertion fires before any state mutation |
| `test_warp_drc20_admin_rejects_non_owner` | WarpDrc20 admin path rejects a Moonlight sender that is not the configured owner |
| `test_warp_drc20_admin_accepts_owner_moonlight_sender` | WarpDrc20 direct Moonlight owner admin path still succeeds after caller-aware sender resolution |
| `test_warp_drc20_handle_rejects_invalid_token_message` | Synthetic warp route rejects a malformed inbound TokenMessage, leaves the message undelivered, and does not mint tokens |
| `test_warp_native_transfer_remote_rejects_zero_amount` | Native warp send rejects zero amounts before deposit handling |
| `test_warp_native_roundtrip_moves_real_dusk` | Exact Moonlight deposit enters WarpNative transfer-contract custody and an inbound message releases the same amount to a registered account |
| `test_warp_native_rejects_mismatched_deposit_without_custody` | Amount/deposit mismatch reverts and leaves the WarpNative contract balance at zero |
| `test_warp_native_handle_rejects_invalid_token_message` | Native warp route rejects a malformed inbound TokenMessage, leaves the message undelivered, and does not escrow funds |
| `test_warp_native_handle_escrows_unregistered_recipient` | Unregistered recipient goes to escrow instead of panicking; `pending_balance` returns correct amount |
| `test_warp_native_escrow_accumulates` | Multiple inbound messages to same unregistered recipient accumulate correctly |
| `test_warp_native_claim_pending_requires_pending` | `claim_pending` panics if no pending balance exists |
| `test_warp_collateral_register_account` | Registration round-trip: `is_registered` returns false before, true after |
| `test_warp_collateral_handle_rejects_invalid_token_message` | Collateral warp route rejects a malformed inbound TokenMessage, leaves the message undelivered, and does not escrow funds |
| `test_warp_collateral_handle_rejects_insufficient_locked_balance` | Collateral unlock fails if the route does not hold enough wrapped-token balance |
| `test_warp_collateral_handle_escrows_unregistered_recipient` | Unregistered collateral recipient goes to escrow instead of a synthetic contract account |
| `test_warp_collateral_claim_pending_transfers_after_registration` | Registered recipient can claim escrowed collateral tokens and the collateral balance decreases exactly once |
| `test_warp_collateral_handle_resolves_registered_external` | End-to-end: pre-fund collateral with DRC20, register BLS key, process inbound message, verify DRC20 tokens unlock to External account |
| `test_multisig_ism_init_rejects_no_validators` | Init fails if validator set is empty |
| `test_multisig_ism_init_rejects_invalid_threshold` | Init fails if threshold exceeds validator count |
| `test_multisig_ism_init_rejects_unsorted_validators` | Init fails if validator list is not strictly sorted |
| `test_multisig_ism_verify_rejects_short_metadata` | Verify fails before parsing metadata shorter than the fixed header |
| `test_multisig_ism_verify_rejects_partial_signature_bytes` | Verify fails when signature metadata has trailing partial bytes |
| `test_multisig_ism_verify_rejects_insufficient_signatures` | Verify fails when metadata contains fewer signatures than threshold |
| `test_multisig_ism_verify_rejects_corrupt_signature_bytes` | Verify fails when fixed-width signature metadata is corrupt and cannot be recovered |
| `test_multisig_ism_admin_rejects_unauthorized_caller` | Validator-set admin update is owner-gated |
| `test_mailbox_quote_dispatch_rejects_fee_overflow` | Mailbox rejects a combined required-hook plus default-hook quote that would overflow `u64` |
| `test_protocol_fee_rejects_collected_fee_overflow` | ProtocolFee rejects lifetime collected-fee accounting overflow instead of saturating silently |
| `test_igp_rejects_total_gas_payment_overflow` | IGP rejects lifetime gas-payment accounting overflow instead of saturating silently |

Additional event annotation verification:

```bash
make all
make clippy-contracts
cargo test -p hyperlane-dusk-types
cargo test -p hyperlane-dusk-integration-tests
```

All commands passed after the explicit event annotation cleanup, Mailbox fee
overflow regression, fee-accounting overflow regression, and targeted clippy
cleanup for the production contract/type surface. The type package reported
`29 passed; 0 failed; 0 ignored`; the integration package reported
`74 passed; 0 failed; 0 ignored` on current Rusk.

The production contract crates allow Clippy's `needless_pass_by_value` lint at
crate level because Dusk ABI entrypoints and cross-contract call payloads use
owned `Vec` values. This keeps the public contract ABI stable while still
running the rest of the pedantic lint set for the wasm contract surface.

### Test Gaps

| Gap | Why |
|---|---|
| Mailbox nonce overflow panic path | The nonce is private Mailbox state with no production setter; reaching `u32::MAX` requires billions of successful dispatches. The code now uses `checked_add`, and the security property is reviewed statically rather than driven through the VM harness with a test-only state mutation hook. |
| Live cross-chain WarpNative and WarpDrc20Collateral routes | The current live agent E2E deploys and transfers WarpDrc20. WarpNative custody/registered release and collateral custody are covered in the VM, but not through a full EVM-agent-Dusk route. |
| ProtocolFee and IGP value enforcement | The hooks record counters/events without collecting DUSK, and the demo does not wire them into Mailbox dispatch. This requires an explicit payment architecture, not just another assertion test. |
| IGP `u64::try_from` panic path | Would need gas oracle config that produces a fee > `u64::MAX`. The `checked_mul` calls before it would panic first in practice. |
| WarpDrc20 checked arithmetic panic paths | Would need to mint or burn an amount that desyncs total supply beyond `u64` bounds, which requires either > `u64::MAX` inbound messages or a pre-existing impossible supply/balance invariant violation. Not practically testable. |

## Open Production Review Decisions

These items are not hidden TODOs in runtime code, but they are decisions that
should be explicitly accepted or changed before any production release. Track
the final sign-off in https://github.com/dusk-network/hyperlane-dusk/issues/2.

| Decision | Recommended release stance | Rationale and evidence | Reviewer action |
|---|---|---|---|
| Owner principal and admin forwarding | Block release until one principal model and a reachable administration path are selected. | Deployed Mailbox/self `ContractId` owners cannot be invoked by a Moonlight account, and Mailbox has no generic admin forwarder. WarpDrc20's caller-aware `H256` model and DuskEVM's discriminated external/contract address model demonstrate viable directions, but the contracts should not keep inconsistent ownership semantics. | Choose account, contract, or discriminated ownership; define transfer/renunciation semantics; migrate all privileged contracts and deployment arguments; add positive and negative Moonlight/inter-contract admin tests. |
| Mailbox `resolve_sender` when called from the transfer contract | Accept the current special case for Moonlight contract-call transactions. | In the current Rusk execution model, a Moonlight transaction that targets a contract reaches the target through `TRANSFER_CONTRACT`, while `abi::public_sender()` exposes the BLS key that signed the transaction. The Mailbox maps that direct-user path to `keccak256(public_sender)` and maps every other immediate caller to the caller `ContractId`. `test_dispatch_via_transaction` asserts the exact account hash; `test_dispatch_via_recipient_proxy` asserts an inter-contract dispatch uses the proxy contract ID. | Confirm with Rusk maintainers that `TRANSFER_CONTRACT` cannot call arbitrary user contracts for non-user-initiated reasons with an unrelated `public_sender`, or request a Rusk-level discriminator before release. |
| `registered_accounts` has no deregistration | Accept immutable registration for v1. | The registered key is stored under `keccak256(pk.to_bytes())`, so replacing a compromised key at the same H256 is not meaningful: a new key produces a new H256/recipient. Deleting a registration would not recover funds already addressed to the old hash. Keeping registrations append-only avoids admin-controlled recipient remapping. | Confirm product/docs will tell users that Dusk recipients are bound to the BLS key hash used as the remote recipient. |
| WarpNative/WarpDrc20Collateral `pending_transfers` has no admin drain | Accept no admin drain for v1. | Escrow is keyed by the recipient hash and can only be claimed by the matching BLS key. An admin drain would add a privileged path that can seize pending user funds and would require a governance/timelock/dispute process that is out of scope for this minimal bridge. If a user loses the private key after bridging to that hash, the funds remain locked. | Confirm Dusk wants this non-custodial failure mode, or design a separate governed recovery mechanism before production. |

## Build & Test Verification

```
# All 11 contract WASMs compile
make all    # in dusk/ directory

# Static lint pass for the production wasm contract/type surface
make clippy-contracts

# 29 unit tests pass
cargo test -p hyperlane-dusk-types

# 74 integration tests pass (47 pre-existing + 27 new)
cargo test -p hyperlane-dusk-integration-tests
```

On 2026-07-20, fresh live agent E2Es also passed against clean current Rusk
`bc281d2cd1e789db92e99bc59849c92363524e37`, synchronized Hyperlane monorepo
`eaa43c3c4decdf007085b19ec6b7d586f150457e`, and Dusk source
`68fe3a80499e3a097c7a32c320c920c068b8c7da` plus the local harness-only changes
documented above:

- TestMock run `1784509481`: EVM -> Dusk delivered 3 wDUSK and Dusk -> EVM
  delivered 1 wDUSK.
- MessageIdMultisig run `1784510116`: the same bidirectional delivery passed
  with a live validator and checkpoint-signature metadata.

These live runs validate the synthetic WarpDrc20 agent route. They do not prove
live WarpNative/Collateral routing or fee enforcement.

Agent-based local E2E evidence is tracked in `TEST_REPORT.md`, including exact
commands, commit SHAs, and `/tmp` log/artifact paths for:

- EVM -> Dusk and Dusk -> EVM using the TestMock/null-style ISM.
- EVM -> Dusk and Dusk -> EVM using MessageIdMultisigISM with validator output.
- Dirty redeploy refusal against a non-reset local Dusk chain.
- Relayer restart/backlog recovery with clean-Rusk 50 EVM -> Dusk and 50
  Dusk -> EVM transfers.
- Delayed validator startup for MessageIdMultisigISM: relayer observes missing
  metadata, Dusk-side delivery remains blocked during the configured validator
  delay, and delivery succeeds after validator checkpoint output is available.
- Corrupted MessageIdMultisig checkpoint metadata: relayer-side delivery remains
  blocked while local checkpoint signature fields are corrupt, then succeeds
  after the valid checkpoint is restored.
- Low Dusk destination signer balance: EVM -> Dusk delivery remains blocked
  when Dusk preverification rejects `process` transactions for insufficient
  signer balance, and delivery succeeds after restarting with the funded local
  signer.
- Origin RPC failure: EVM -> Dusk delivery remains blocked while the relayer's
  Anvil RPC endpoint is unreachable, and delivery succeeds after restarting the
  relayer with the healthy origin RPC config.
- Destination RPC failure: EVM -> Dusk delivery remains blocked while the
  relayer's Dusk RUES endpoint is unreachable, and delivery succeeds after
  restarting the relayer with the healthy destination RPC config.
- Duplicate relayer attempt: two relayers observe the same EVM -> Dusk message,
  Dusk rejects duplicate transaction submissions at preverification/mempool
  level, and token supply remains single-delivery stable.
