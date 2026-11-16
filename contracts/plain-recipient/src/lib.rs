// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane PlainRecipient test contract for Dusk.

//! A message recipient that implements only `handle`, with no
//! `interchain_security_module` getter. Tests use it to check that the
//! Mailbox falls back to its default ISM cheaply for such recipients.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]

/// Hyperlane PlainRecipient contract.
#[dusk_forge::contract]
mod plain_recipient {
    extern crate alloc;

    use alloc::vec::Vec;

    use hyperlane_dusk_types::H256;

    /// PlainRecipient contract state.
    pub struct PlainRecipient {
        /// Total number of messages received.
        handled_count: u32,
    }

    impl PlainRecipient {
        /// Creates a new empty PlainRecipient state.
        pub const fn new() -> Self {
            Self { handled_count: 0 }
        }

        /// Handle an incoming Hyperlane message.
        pub fn handle(&mut self, _origin: u32, _sender: H256, _body: Vec<u8>) {
            self.handled_count = self
                .handled_count
                .checked_add(1)
                .expect("PlainRecipient: handled count overflow");
        }

        /// Returns the number of messages received.
        pub fn handled_count(&self) -> u32 {
            self.handled_count
        }
    }
}
