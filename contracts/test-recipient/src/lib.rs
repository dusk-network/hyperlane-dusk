// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane TestRecipient contract for Dusk.

//! Simple message recipient for testing end-to-end message delivery.
//!
//! Implements the `handle(origin, sender, body)` interface that the
//! Mailbox calls when delivering a message. Stores the last received
//! message for verification in tests.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane TestRecipient contract.
#[dusk_forge::contract]
mod test_recipient {
    extern crate alloc;

    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::ReceiveFromContract;

    use hyperlane_dusk_types::{caller, MessageId, H256};

    /// Zero contract ID (meaning "no ISM override").
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// TestRecipient contract state.
    pub struct TestRecipient {
        /// The origin domain of the last received message.
        last_origin: u32,
        /// The sender of the last received message.
        last_sender: H256,
        /// The body of the last received message.
        last_body: Vec<u8>,
        /// Total number of messages received.
        handled_count: u32,
        /// ISM override (returned by `interchain_security_module`).
        /// Zero means "use mailbox default".
        ism: ContractId,
        /// Native DUSK received through the pending-escrow test callback.
        native_pending_received: u64,
    }

    impl TestRecipient {
        /// Creates a new empty TestRecipient state.
        pub const fn new() -> Self {
            Self {
                last_origin: 0,
                last_sender: [0u8; 32],
                last_body: Vec::new(),
                handled_count: 0,
                ism: ZERO_CONTRACT,
                native_pending_received: 0,
            }
        }

        // =================================================================
        // IMessageRecipient interface
        // =================================================================

        /// Handle an incoming Hyperlane message.
        ///
        /// Called by the Mailbox when a message is delivered to this contract.
        pub fn handle(&mut self, origin: u32, sender: H256, body: Vec<u8>) {
            self.last_origin = origin;
            self.last_sender = sender;
            self.last_body = body;
            self.handled_count += 1;
        }

        // =================================================================
        // ISpecifiesInterchainSecurityModule interface
        // =================================================================

        /// Returns the ISM contract to use for verifying messages to this recipient.
        ///
        /// Returns zero `ContractId` if no override is set, causing
        /// the Mailbox to use its default ISM.
        pub fn interchain_security_module(&self) -> ContractId {
            self.ism
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the persisted state layout version expected by deployment tooling.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            1
        }

        /// Returns the origin domain of the last received message.
        pub fn last_origin(&self) -> u32 {
            self.last_origin
        }

        /// Returns the sender of the last received message.
        pub fn last_sender(&self) -> H256 {
            self.last_sender
        }

        /// Returns the body of the last received message.
        pub fn last_body(&self) -> Vec<u8> {
            self.last_body.clone()
        }

        /// Returns the total number of messages handled.
        pub fn handled_count(&self) -> u32 {
            self.handled_count
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Set the ISM override. Pass zero `ContractId` to use the Mailbox's default.
        pub fn set_interchain_security_module(&mut self, ism: ContractId) {
            self.ism = ism;
        }

        // =================================================================
        // Dispatch proxy (for E2E / integration testing)
        // =================================================================

        /// Dispatch a message through the Mailbox.
        ///
        /// This contract becomes the message sender (via `abi::caller()`
        /// inside the Mailbox). Useful for E2E testing where the caller
        /// must be a contract.
        pub fn dispatch_message(
            &self,
            mailbox: ContractId,
            destination: u32,
            recipient: H256,
            body: Vec<u8>,
        ) -> MessageId {
            let id: MessageId =
                abi::call(mailbox, "dispatch_default", &(destination, recipient, body))
                    .expect("TestRecipient: dispatch_message failed");
            id
        }

        /// Claim collateral-route tokens escrowed for this contract ID.
        pub fn claim_collateral_pending(&self, route: ContractId) {
            let _: () = abi::call(route, "claim_pending_contract", &())
                .expect("TestRecipient: collateral claim failed");
        }

        /// Claim synthetic-route tokens pending for this contract ID.
        pub fn claim_synthetic_pending(&self, route: ContractId) {
            let _: () = abi::call(route, "claim_pending_contract", &())
                .expect("TestRecipient: synthetic claim failed");
        }

        /// Claim native DUSK escrowed for this contract ID.
        pub fn claim_native_pending(&self, route: ContractId) {
            let _: () = abi::call(route, "claim_pending_contract", &())
                .expect("TestRecipient: native claim failed");
        }

        /// Attempt a protocol-fee claim through this contract.
        pub fn claim_protocol_fees(&self, protocol_fee: ContractId) {
            let _: () = abi::call(protocol_fee, "claim", &())
                .expect("TestRecipient: protocol fee claim failed");
        }

        /// Attempt an IGP fee claim through this contract.
        pub fn claim_igp_fees(&self, igp: ContractId) {
            let _: () = abi::call(igp, "claim", &()).expect("TestRecipient: IGP fee claim failed");
        }

        /// Accept native DUSK released by a route's contract-claim path.
        pub fn receive_native_pending(&mut self, transfer: ReceiveFromContract) {
            assert!(
                caller::authentic_transfer_callback(transfer.contract, transfer.contract),
                "TestRecipient: unauthenticated native claim"
            );
            assert!(
                transfer.data.is_empty(),
                "TestRecipient: unexpected native claim data"
            );
            self.native_pending_received = self
                .native_pending_received
                .checked_add(transfer.value)
                .expect("TestRecipient: native claim overflow");
        }

        /// Return native DUSK received by the pending-escrow callback.
        pub fn native_pending_received(&self) -> u64 {
            self.native_pending_received
        }
    }
}
