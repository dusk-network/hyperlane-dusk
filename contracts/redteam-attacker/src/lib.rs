// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Red-team attack contract for callback and reentrancy tests.

//! Test-only contract that plays every attacker-controlled callback role:
//! post-dispatch hook, message recipient, recipient-nominated ISM and
//! contract recipient of native escrow. At each callback point it replays a
//! configured list of raw calls, records whether each succeeded, and can
//! panic afterwards so the caller observes a failed callback.

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
    use alloc::string::String;
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::ReceiveFromContract;

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

    /// One raw call: target, function name, rkyv-serialized argument.
    type Action = (ContractId, String, Vec<u8>);

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
            }
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

        /// Runs `actions` now, panicking if any call fails and `strict` is set.
        pub fn run(&mut self, actions: Vec<Action>, strict: bool) {
            for (target, fn_name, args) in actions {
                let result = abi::call_raw(target, &fn_name, &args);
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
                let ok = abi::call_raw(target, &fn_name, &args).is_ok();
                self.results.push(ok);
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
