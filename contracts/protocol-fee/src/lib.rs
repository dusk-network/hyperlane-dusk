// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane ProtocolFee hook contract for Dusk.

//! Post-dispatch hook that charges a fixed protocol fee per message.
//!
//! This is the Dusk equivalent of `ProtocolFee.sol`. Each dispatched message
//! incurs a configurable fee (capped by `max_protocol_fee`). Fees are tracked
//! via an accounting model; actual DUSK collection is handled externally.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane ProtocolFee hook contract.
#[dusk_forge::contract]
mod protocol_fee {
    extern crate alloc;

    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// ProtocolFee contract state.
    pub struct ProtocolFee {
        /// Fee charged per dispatch (in LUX).
        protocol_fee: u64,
        /// Maximum allowed fee (immutable after init).
        max_protocol_fee: u64,
        /// Beneficiary that receives collected fees.
        beneficiary: ContractId,
        /// Contract owner (can update fee and beneficiary).
        owner: Option<ContractId>,
        /// Running total of fees owed (accounting-based).
        collected_fees: u64,
    }

    impl ProtocolFee {
        /// Creates a new empty ProtocolFee state.
        pub const fn new() -> Self {
            Self {
                protocol_fee: 0,
                max_protocol_fee: 0,
                beneficiary: ZERO_CONTRACT,
                owner: None,
                collected_fees: 0,
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the ProtocolFee hook.
        ///
        /// Must be called once after deployment. Panics if already
        /// initialized.
        #[contract(emits = [
            (events::Initialized::TOPIC, events::Initialized),
            (events::ProtocolFeeSet::TOPIC, events::ProtocolFeeSet),
            (events::BeneficiarySet::TOPIC, events::BeneficiarySet)
        ])]
        pub fn init(
            &mut self,
            protocol_fee: u64,
            max_protocol_fee: u64,
            beneficiary: ContractId,
            owner: ContractId,
        ) {
            assert!(self.owner.is_none(), "ProtocolFee: already initialized");
            assert!(
                protocol_fee <= max_protocol_fee,
                "ProtocolFee: fee exceeds maximum"
            );
            assert!(
                beneficiary != ZERO_CONTRACT,
                "ProtocolFee: beneficiary cannot be zero"
            );
            self.protocol_fee = protocol_fee;
            self.max_protocol_fee = max_protocol_fee;
            self.beneficiary = beneficiary;
            self.owner = Some(owner);
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_PROTOCOL_FEE,
                    owner: owner.to_bytes(),
                    mailbox: ZERO_CONTRACT.to_bytes(),
                    local_domain: 0,
                },
            );
            abi::emit(
                events::ProtocolFeeSet::TOPIC,
                events::ProtocolFeeSet { fee: protocol_fee },
            );
            abi::emit(
                events::BeneficiarySet::TOPIC,
                events::BeneficiarySet {
                    beneficiary: beneficiary.to_bytes(),
                },
            );
        }

        // =================================================================
        // IPostDispatchHook interface
        // =================================================================

        /// Called by the Mailbox after a message is dispatched.
        ///
        /// Records the protocol fee and emits a `ProtocolFeePaid` event.
        #[contract(emits = [(events::ProtocolFeePaid::TOPIC, events::ProtocolFeePaid)])]
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, encoded_message: Vec<u8>) {
            let sender = message::sender(&encoded_message);
            self.collected_fees = self.collected_fees.saturating_add(self.protocol_fee);

            abi::emit(
                events::ProtocolFeePaid::TOPIC,
                events::ProtocolFeePaid {
                    sender,
                    fee: self.protocol_fee,
                },
            );
        }

        /// Returns the fee required for this hook.
        #[allow(clippy::unused_self)]
        pub fn quote_dispatch(&self, _metadata: Vec<u8>, _message: Vec<u8>) -> u64 {
            self.protocol_fee
        }

        /// Returns the hook type identifier.
        #[allow(clippy::unused_self)]
        pub fn hook_type(&self) -> u8 {
            6 // HookType::ProtocolFee
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the current protocol fee.
        pub fn protocol_fee(&self) -> u64 {
            self.protocol_fee
        }

        /// Returns the maximum allowed protocol fee.
        pub fn max_protocol_fee(&self) -> u64 {
            self.max_protocol_fee
        }

        /// Returns the beneficiary contract ID.
        pub fn beneficiary(&self) -> ContractId {
            self.beneficiary
        }

        /// Returns the owner contract ID.
        pub fn owner(&self) -> Option<ContractId> {
            self.owner
        }

        /// Returns the total collected fees (accounting).
        pub fn collected_fees(&self) -> u64 {
            self.collected_fees
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Set the protocol fee. Owner only.
        ///
        /// Panics if the new fee exceeds `max_protocol_fee`.
        #[contract(emits = [(events::ProtocolFeeSet::TOPIC, events::ProtocolFeeSet)])]
        pub fn set_protocol_fee(&mut self, fee: u64) {
            self.only_owner();
            assert!(
                fee <= self.max_protocol_fee,
                "ProtocolFee: fee exceeds maximum"
            );
            self.protocol_fee = fee;
            abi::emit(
                events::ProtocolFeeSet::TOPIC,
                events::ProtocolFeeSet { fee },
            );
        }

        /// Set the beneficiary. Owner only.
        #[contract(emits = [(events::BeneficiarySet::TOPIC, events::BeneficiarySet)])]
        pub fn set_beneficiary(&mut self, beneficiary: ContractId) {
            self.only_owner();
            assert!(
                beneficiary != ZERO_CONTRACT,
                "ProtocolFee: beneficiary cannot be zero"
            );
            self.beneficiary = beneficiary;
            abi::emit(
                events::BeneficiarySet::TOPIC,
                events::BeneficiarySet {
                    beneficiary: beneficiary.to_bytes(),
                },
            );
        }

        /// Transfer ownership. Owner only.
        #[contract(emits = [(events::OwnershipTransferred::TOPIC, events::OwnershipTransferred)])]
        pub fn transfer_ownership(&mut self, new_owner: ContractId) {
            self.only_owner();
            let previous_owner = self.owner.expect("ProtocolFee: no owner set");
            self.owner = Some(new_owner);
            abi::emit(
                events::OwnershipTransferred::TOPIC,
                events::OwnershipTransferred {
                    previous_owner: previous_owner.to_bytes(),
                    new_owner: new_owner.to_bytes(),
                },
            );
        }

        // =================================================================
        // Internal helpers
        // =================================================================

        /// Panics if the caller is not the owner.
        fn only_owner(&self) {
            let caller = abi::caller().expect("ProtocolFee: cannot determine caller");
            let owner = self.owner.expect("ProtocolFee: no owner set");
            assert!(caller == owner, "ProtocolFee: caller is not the owner");
        }
    }
}
