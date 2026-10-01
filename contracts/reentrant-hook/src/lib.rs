// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Adversarial callbacks used to test Mailbox dispatch and process reentrancy.

//! Test-only hook that attempts nested dispatch from quote, post-dispatch,
//! or native-payment callbacks, and can reject a post-dispatch callback.
//! It also acts as a recipient that replays a message during ISM resolution.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]

/// Adversarial post-dispatch hook for Mailbox integration tests.
#[dusk_forge::contract]
mod reentrant_hook {
    extern crate alloc;

    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};

    use dusk_core::transfer::ReceiveFromContract;

    use hyperlane_dusk_types::{caller, MessageId, H256};

    /// Zero contract ID selects the Mailbox default hook.
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Test-only hook state.
    pub struct ReentrantHook {
        /// Mailbox targeted by the nested dispatch.
        mailbox: ContractId,
        /// Whether a nested dispatch has been attempted.
        attempted: bool,
        /// Whether the nested dispatch succeeded.
        nested_succeeded: bool,
        /// Number of successful outer post-dispatch callbacks.
        post_dispatch_count: u32,
        /// Callback to attack: quote (0), post-dispatch (1), or payment (2).
        stage: u8,
        /// Native fee quoted for the outer dispatch.
        fee: u64,
        /// Whether the post-dispatch callback should revert.
        fail_post_dispatch: bool,
        /// Inbound message to replay during recipient ISM resolution.
        inbound_message: Vec<u8>,
        /// Number of inbound deliveries observed.
        handled_count: u32,
    }

    impl ReentrantHook {
        /// Creates an uninitialized test hook.
        pub const fn new() -> Self {
            Self {
                mailbox: ZERO_CONTRACT,
                attempted: false,
                nested_succeeded: false,
                post_dispatch_count: 0,
                stage: 0,
                fee: 0,
                fail_post_dispatch: false,
                inbound_message: Vec::new(),
                handled_count: 0,
            }
        }

        /// Binds the hook to the Mailbox under test.
        pub fn init(&mut self, mailbox: ContractId) {
            assert!(
                self.mailbox == ZERO_CONTRACT,
                "ReentrantHook: already initialized"
            );
            assert!(
                mailbox != ZERO_CONTRACT,
                "ReentrantHook: mailbox cannot be zero"
            );
            self.mailbox = mailbox;
        }

        /// Configures the next test attack and clears its observation flags.
        pub fn configure(&mut self, stage: u8, fee: u64, fail_post_dispatch: bool) {
            assert!(stage <= 2, "ReentrantHook: invalid stage");
            self.stage = stage;
            self.fee = fee;
            self.fail_post_dispatch = fail_post_dispatch;
            self.attempted = false;
            self.nested_succeeded = false;
        }

        /// Attempts reentry from the quote callback when configured.
        pub fn quote_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) -> u64 {
            if self.stage == 0 {
                self.attempt_reentry();
            }
            self.fee
        }

        /// Attempts one nested dispatch, recording its result.
        fn attempt_reentry(&mut self) {
            if !self.attempted {
                self.attempted = true;
                let nested: Result<MessageId, _> = abi::call(
                    self.mailbox,
                    "dispatch",
                    &(
                        1u32,
                        [0xA5u8; 32],
                        Vec::<u8>::new(),
                        Vec::<u8>::new(),
                        ZERO_CONTRACT,
                    ),
                );
                self.nested_succeeded = nested.is_ok();
            }
        }

        /// Records the outer callback and optionally attempts reentry or reverts.
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) {
            if self.stage == 1 {
                self.attempt_reentry();
            }
            assert!(!self.fail_post_dispatch, "ReentrantHook: rejected callback");
            self.post_dispatch_count = self
                .post_dispatch_count
                .checked_add(1)
                .expect("ReentrantHook: post-dispatch count overflow");
        }

        /// Attempts reentry from an authenticated native-payment callback.
        pub fn receive_payment(&mut self, transfer: ReceiveFromContract) {
            assert!(
                caller::authentic_transfer_callback(transfer.contract, self.mailbox),
                "ReentrantHook: unauthenticated payment"
            );
            assert_eq!(transfer.value, self.fee, "ReentrantHook: wrong payment");
            if self.stage == 2 {
                self.attempt_reentry();
            }
        }

        /// Configures an inbound replay during recipient ISM resolution.
        pub fn configure_inbound(&mut self, encoded_message: Vec<u8>) {
            self.inbound_message = encoded_message;
            self.attempted = false;
            self.nested_succeeded = false;
        }

        /// Attempts a same-message replay before selecting the default ISM.
        pub fn interchain_security_module(&mut self) -> ContractId {
            if !self.inbound_message.is_empty() && !self.attempted {
                self.attempted = true;
                let nested: Result<(), _> = abi::call(
                    self.mailbox,
                    "process",
                    &(Vec::<u8>::new(), self.inbound_message.clone()),
                );
                self.nested_succeeded = nested.is_ok();
            }
            ZERO_CONTRACT
        }

        /// Records an inbound delivery from the Mailbox.
        pub fn handle(&mut self, _origin: u32, _sender: H256, _body: Vec<u8>) {
            assert_eq!(abi::caller(), Some(self.mailbox));
            self.handled_count = self
                .handled_count
                .checked_add(1)
                .expect("ReentrantHook: handled count overflow");
        }

        /// Returns the observed inbound delivery count.
        pub fn handled_count(&self) -> u32 {
            self.handled_count
        }

        /// Returns whether the hook attempted its nested dispatch.
        pub fn attempted(&self) -> bool {
            self.attempted
        }

        /// Returns whether the attempted nested dispatch succeeded.
        pub fn nested_succeeded(&self) -> bool {
            self.nested_succeeded
        }

        /// Returns the number of successful post-dispatch callbacks.
        pub fn post_dispatch_count(&self) -> u32 {
            self.post_dispatch_count
        }

        /// Returns the persisted state layout version for this test contract.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            2
        }
    }
}
