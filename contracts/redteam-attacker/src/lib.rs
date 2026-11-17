// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Red-team attack contract for callback and reentrancy tests.

//! Test-only contract that plays every attacker-controlled callback role:
//! post-dispatch hook, message recipient, recipient-nominated ISM, contract
//! recipient of native escrow and holder of a token that calls its sender
//! back (`callback-fee-token`). At each callback point it replays a
//! configured list of raw calls, records whether each succeeded, and can
//! panic afterwards so the caller observes a failed callback. Each failed
//! call also emits a `call_failed` event with the function and the error,
//! which stays in the receipt, flagged, when the callback's state rolls
//! back. It can also make calls from its `init`, while it is deployed.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::needless_pass_by_value)]

/// Programmable attacker contract.
#[dusk_forge::contract]
mod redteam_attacker {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractError, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::ReceiveFromContract;

    use hyperlane_dusk_types::drc20::Account;
    use hyperlane_dusk_types::H256;

    /// Zero contract ID: "use the Mailbox default ISM".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Callback point: `quote_dispatch`.
    const QUOTE: u8 = 0;
    /// Callback point: `post_dispatch`.
    const POST: u8 = 1;
    /// Callback point: `receive_payment`.
    const PAYMENT: u8 = 2;
    /// Callback point: `interchain_security_module`.
    const ISM_LOOKUP: u8 = 3;
    /// Callback point: `verify`.
    const VERIFY: u8 = 4;
    /// Callback point: `handle`.
    const HANDLE: u8 = 5;
    /// Callback point: `receive_native_pending`.
    const NATIVE_PENDING: u8 = 6;
    /// Callback point: `tokens_to_send`.
    const TOKENS_TO_SEND: u8 = 7;

    /// One raw call: target, function name, rkyv-serialized argument.
    type Action = (ContractId, String, Vec<u8>);

    /// Topic of the event a failed call emits, with `(function, error)`.
    const CALL_FAILED: &str = "call_failed";

    /// Emits [`CALL_FAILED`] for a call that failed, with a panic's own
    /// message, which is cheap enough for callbacks run on a gas allowance.
    fn report(fn_name: &str, result: &Result<Vec<u8>, ContractError>) {
        if let Err(error) = result {
            let error = match error {
                ContractError::Panic(message) => message.clone(),
                other => format!("{other:?}"),
            };
            let failure: (String, String) = (String::from(fn_name), error);
            abi::emit(CALL_FAILED, failure);
        }
    }

    /// Attacker state.
    pub struct RedteamAttacker {
        /// Calls replayed at each callback point.
        actions: BTreeMap<u8, Vec<Action>>,
        /// Callback points that panic after replaying their calls.
        panic_after: BTreeMap<u8, bool>,
        /// Callback points that already fired (each fires once).
        fired: BTreeMap<u8, bool>,
        /// Success flag of every replayed call, in order.
        results: Vec<bool>,
        /// Fee returned from `quote_dispatch`.
        quote_fee: u64,
        /// ISM returned from `interchain_security_module`.
        ism: ContractId,
        /// Number of `handle` calls observed.
        handled: u32,
        /// Number of `post_dispatch` calls observed.
        posts: u32,
        /// Native DUSK received through payment or escrow callbacks.
        received: u64,
        /// Busy-loop rounds run by `interchain_security_module`.
        ism_burn: u64,
    }

    impl RedteamAttacker {
        /// Creates an unconfigured attacker.
        pub const fn new() -> Self {
            Self {
                actions: BTreeMap::new(),
                panic_after: BTreeMap::new(),
                fired: BTreeMap::new(),
                results: Vec::new(),
                quote_fee: 0,
                ism: ZERO_CONTRACT,
                handled: 0,
                posts: 0,
                received: 0,
                ism_burn: 0,
            }
        }

        /// Makes `actions` while the contract is deployed, panicking if any
        /// call fails. `init` runs with no caller of its own, so these calls
        /// come from a contract at the bottom of the call stack.
        pub fn init(&mut self, actions: Vec<Action>) {
            self.run(actions, true);
        }

        /// Sets the calls replayed at `point`, and whether it then panics.
        pub fn configure(&mut self, point: u8, actions: Vec<Action>, panic_after: bool) {
            self.actions.insert(point, actions);
            self.panic_after.insert(point, panic_after);
            self.fired.remove(&point);
        }

        /// Sets the hook quote and the nominated ISM.
        pub fn set_returns(&mut self, quote_fee: u64, ism: ContractId) {
            self.quote_fee = quote_fee;
            self.ism = ism;
        }

        /// Sets how many busy-loop rounds the ISM getter runs, to make it
        /// expensive.
        pub fn set_ism_burn(&mut self, rounds: u64) {
            self.ism_burn = rounds;
        }

        /// Runs `actions` now, panicking if any call fails and `strict` is set.
        pub fn run(&mut self, actions: Vec<Action>, strict: bool) {
            for (target, fn_name, args) in actions {
                let result = abi::call_raw(target, &fn_name, &args);
                report(&fn_name, &result);
                if strict {
                    if let Err(err) = &result {
                        panic!("RedteamAttacker: {fn_name} failed: {err:?}");
                    }
                }
                self.results.push(result.is_ok());
            }
        }

        /// Replays the calls configured for `point` once.
        fn fire(&mut self, point: u8) {
            if self.fired.get(&point).copied().unwrap_or(false) {
                return;
            }
            self.fired.insert(point, true);
            let actions = self.actions.get(&point).cloned().unwrap_or_default();
            for (target, fn_name, args) in actions {
                let result = abi::call_raw(target, &fn_name, &args);
                report(&fn_name, &result);
                self.results.push(result.is_ok());
            }
            assert!(
                !self.panic_after.get(&point).copied().unwrap_or(false),
                "RedteamAttacker: configured callback panic"
            );
        }

        /// Hook quote callback.
        pub fn quote_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) -> u64 {
            self.fire(QUOTE);
            self.quote_fee
        }

        /// Hook post-dispatch callback.
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) {
            self.posts += 1;
            self.fire(POST);
        }

        /// Hook payment callback (accepts any payment).
        pub fn receive_payment(&mut self, transfer: ReceiveFromContract) {
            self.received += transfer.value;
            self.fire(PAYMENT);
        }

        /// Recipient ISM lookup callback.
        pub fn interchain_security_module(&mut self) -> ContractId {
            let mut acc = 0u64;
            for round in 0..self.ism_burn {
                acc = core::hint::black_box(acc.wrapping_mul(31).wrapping_add(round));
            }
            core::hint::black_box(acc);
            self.fire(ISM_LOOKUP);
            self.ism
        }

        /// Nominated-ISM verification callback (approves everything).
        pub fn verify(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) -> bool {
            self.fire(VERIFY);
            true
        }

        /// Recipient handle callback.
        pub fn handle(&mut self, _origin: u32, _sender: H256, _body: Vec<u8>) {
            self.handled += 1;
            self.fire(HANDLE);
        }

        /// Native escrow callback from a route's contract-claim path.
        pub fn receive_native_pending(&mut self, transfer: ReceiveFromContract) {
            self.received += transfer.value;
            self.fire(NATIVE_PENDING);
        }

        /// Token callback before the attacker's tokens move.
        pub fn tokens_to_send(&mut self, _from: Account, _to: Account, _amount: u64) {
            self.fire(TOKENS_TO_SEND);
        }

        /// Returns the success flag of every replayed call.
        pub fn results(&self) -> Vec<bool> {
            self.results.clone()
        }

        /// Returns `(handled, posts, received)`.
        pub fn counters(&self) -> (u32, u32, u64) {
            (self.handled, self.posts, self.received)
        }
    }
}
