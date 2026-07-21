// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Adversarial hook used to test Mailbox dispatch reentrancy.

//! Test-only hook that attempts one nested Mailbox dispatch while quoting.

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

    use hyperlane_dusk_types::MessageId;

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
    }

    impl ReentrantHook {
        /// Creates an uninitialized test hook.
        pub const fn new() -> Self {
            Self {
                mailbox: ZERO_CONTRACT,
                attempted: false,
                nested_succeeded: false,
                post_dispatch_count: 0,
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

        /// Attempts one nested dispatch and returns a zero fee quote.
        pub fn quote_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) -> u64 {
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
            0
        }

        /// Records the successful outer hook callback.
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) {
            self.post_dispatch_count = self
                .post_dispatch_count
                .checked_add(1)
                .expect("ReentrantHook: post-dispatch count overflow");
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
            1
        }
    }
}
