// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane TestMock contract for Dusk.

//! Combined Null ISM + No-op Hook for integration testing.
//!
//! This contract implements both the ISM interface (`verify` always returns
//! `true`) and the Hook interface (`post_dispatch` is a no-op). This allows
//! integration tests to deploy a single contract that serves as the default
//! ISM and/or default hook for the Mailbox.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane TestMock contract (NullISM + NoopHook).
#[dusk_forge::contract]
mod test_mock {
    extern crate alloc;

    use alloc::vec::Vec;

    /// TestMock contract state (stateless).
    pub struct TestMock {
        /// Number of times verify was called.
        verify_count: u32,
        /// Number of times post_dispatch was called.
        post_dispatch_count: u32,
    }

    impl TestMock {
        /// Creates a new TestMock state.
        pub const fn new() -> Self {
            Self {
                verify_count: 0,
                post_dispatch_count: 0,
            }
        }

        // =================================================================
        // IInterchainSecurityModule interface (NullISM)
        // =================================================================

        /// Always returns `true` — no verification performed.
        pub fn verify(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) -> bool {
            self.verify_count += 1;
            true
        }

        /// Returns ISM module type 6 (Null).
        #[allow(clippy::unused_self)]
        pub fn module_type(&self) -> u8 {
            6 // IsmType::Null
        }

        // =================================================================
        // IPostDispatchHook interface (NoopHook)
        // =================================================================

        /// No-op post dispatch. Does nothing.
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, _message: Vec<u8>) {
            self.post_dispatch_count += 1;
        }

        /// Always returns 0 — no fee required.
        #[allow(clippy::unused_self)]
        pub fn quote_dispatch(&self, _metadata: Vec<u8>, _message: Vec<u8>) -> u64 {
            0
        }

        // =================================================================
        // ISpecifiesInterchainSecurityModule (returns self as ISM)
        // =================================================================

        /// Returns zero (no ISM override — use Mailbox default).
        #[allow(clippy::unused_self)]
        pub fn interchain_security_module(&self) -> [u8; 32] {
            [0u8; 32]
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the number of times `verify` was called.
        pub fn verify_count(&self) -> u32 {
            self.verify_count
        }

        /// Returns the number of times `post_dispatch` was called.
        pub fn post_dispatch_count(&self) -> u32 {
            self.post_dispatch_count
        }
    }
}
