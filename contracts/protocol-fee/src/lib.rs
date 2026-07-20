// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane ProtocolFee hook contract for Dusk.

//! Post-dispatch hook that charges a fixed protocol fee per message.
//!
//! This is the Dusk equivalent of `ProtocolFee.sol`. Each dispatched message
//! incurs a configurable fee (capped by `max_protocol_fee`). The Mailbox sends
//! the exact quoted native DUSK value through the transfer contract before a
//! payment is recorded.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane ProtocolFee hook contract.
#[dusk_forge::contract(events = [
    events::BeneficiarySet,
    events::Initialized,
    events::OwnershipTransferred,
    events::ProtocolFeePaid,
    events::ProtocolFeeSet,
])]
mod protocol_fee {
    extern crate alloc;

    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::{ContractToAccount, ReceiveFromContract, TRANSFER_CONTRACT};

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::H256;

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// ProtocolFee contract state.
    pub struct ProtocolFee {
        /// Fee charged per dispatch (in LUX).
        protocol_fee: u64,
        /// Maximum allowed fee (immutable after init).
        max_protocol_fee: u64,
        /// Mailbox authorized to prepare and send hook payments.
        mailbox: ContractId,
        /// Moonlight account hash that may claim collected fees.
        beneficiary: H256,
        /// Owner identity (Moonlight account hash or contract ID).
        owner: Option<H256>,
        /// Lifetime total of native DUSK fees collected.
        collected_fees: u64,
        /// Native DUSK collected and not yet claimed by the beneficiary.
        claimable_fees: u64,
        /// Payment prepared by an authenticated Mailbox call and awaiting the
        /// matching transfer-contract callback.
        pending_payment: Option<(H256, u64)>,
    }

    impl ProtocolFee {
        /// Creates a new empty ProtocolFee state.
        pub const fn new() -> Self {
            Self {
                protocol_fee: 0,
                max_protocol_fee: 0,
                mailbox: ZERO_CONTRACT,
                beneficiary: [0u8; 32],
                owner: None,
                collected_fees: 0,
                claimable_fees: 0,
                pending_payment: None,
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the ProtocolFee hook.
        ///
        /// Must be called once after deployment. Panics if already
        /// initialized.
        pub fn init(
            &mut self,
            protocol_fee: u64,
            max_protocol_fee: u64,
            mailbox: ContractId,
            beneficiary: H256,
            owner: H256,
        ) {
            assert!(self.owner.is_none(), "ProtocolFee: already initialized");
            assert!(
                protocol_fee <= max_protocol_fee,
                "ProtocolFee: fee exceeds maximum"
            );
            assert!(
                mailbox != ZERO_CONTRACT,
                "ProtocolFee: mailbox cannot be zero"
            );
            assert!(
                beneficiary != [0u8; 32],
                "ProtocolFee: beneficiary cannot be zero"
            );
            self.protocol_fee = protocol_fee;
            self.max_protocol_fee = max_protocol_fee;
            self.mailbox = mailbox;
            self.beneficiary = beneficiary;
            self.owner = Some(owner);
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_PROTOCOL_FEE,
                    owner,
                    mailbox: mailbox.to_bytes(),
                    local_domain: 0,
                },
            );
            abi::emit(
                events::ProtocolFeeSet::TOPIC,
                events::ProtocolFeeSet { fee: protocol_fee },
            );
            abi::emit(
                events::BeneficiarySet::TOPIC,
                events::BeneficiarySet { beneficiary },
            );
        }

        // =================================================================
        // IPostDispatchHook interface
        // =================================================================

        /// Called by the Mailbox after a message is dispatched.
        ///
        /// Prepares the exact payment expected from the Mailbox.
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, encoded_message: Vec<u8>) {
            assert!(
                abi::caller() == Some(self.mailbox),
                "ProtocolFee: caller is not mailbox"
            );
            assert!(
                self.pending_payment.is_none(),
                "ProtocolFee: payment already pending"
            );
            if self.protocol_fee == 0 {
                return;
            }
            let sender = message::sender(&encoded_message);
            self.pending_payment = Some((sender, self.protocol_fee));
        }

        /// Authenticate and record the native DUSK sent by the Mailbox.
        pub fn receive_payment(&mut self, transfer: ReceiveFromContract) {
            assert!(
                caller::authentic_transfer_callback(transfer.contract, self.mailbox),
                "ProtocolFee: unauthenticated payment"
            );
            assert!(transfer.data.is_empty(), "ProtocolFee: unexpected payment data");
            let (sender, expected_fee) = self
                .pending_payment
                .take()
                .expect("ProtocolFee: no payment pending");
            assert!(
                transfer.value == expected_fee,
                "ProtocolFee: incorrect payment"
            );
            self.collected_fees = self
                .collected_fees
                .checked_add(transfer.value)
                .expect("ProtocolFee: collected fee overflow");
            self.claimable_fees = self
                .claimable_fees
                .checked_add(transfer.value)
                .expect("ProtocolFee: claimable fee overflow");

            abi::emit(
                events::ProtocolFeePaid::TOPIC,
                events::ProtocolFeePaid {
                    sender,
                    fee: transfer.value,
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

        /// Returns the Mailbox authorized to send payments.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }

        /// Returns the beneficiary Moonlight account hash.
        pub fn beneficiary(&self) -> H256 {
            self.beneficiary
        }

        /// Returns the owner identity.
        pub fn owner(&self) -> Option<H256> {
            self.owner
        }

        /// Returns the total collected fees (accounting).
        pub fn collected_fees(&self) -> u64 {
            self.collected_fees
        }

        /// Returns collected native DUSK not yet claimed by the beneficiary.
        pub fn claimable_fees(&self) -> u64 {
            self.claimable_fees
        }

        /// Transfer all claimable native DUSK to the beneficiary account.
        pub fn claim(&mut self) {
            assert!(
                caller::effective_caller() == self.beneficiary,
                "ProtocolFee: caller is not beneficiary"
            );
            let account = abi::public_sender().expect("ProtocolFee: beneficiary must be Moonlight");
            let amount = self.claimable_fees;
            assert!(amount > 0, "ProtocolFee: no fees to claim");
            self.claimable_fees = 0;
            let transfer = ContractToAccount { account, value: amount };
            let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_account", &transfer)
                .expect("ProtocolFee: claim transfer failed");
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Set the protocol fee. Owner only.
        ///
        /// Panics if the new fee exceeds `max_protocol_fee`.
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
        pub fn set_beneficiary(&mut self, beneficiary: H256) {
            self.only_owner();
            assert!(
                beneficiary != [0u8; 32],
                "ProtocolFee: beneficiary cannot be zero"
            );
            self.beneficiary = beneficiary;
            abi::emit(
                events::BeneficiarySet::TOPIC,
                events::BeneficiarySet { beneficiary },
            );
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            let previous_owner = self.owner.expect("ProtocolFee: no owner set");
            self.owner = Some(new_owner);
            abi::emit(
                events::OwnershipTransferred::TOPIC,
                events::OwnershipTransferred {
                    previous_owner,
                    new_owner,
                },
            );
        }

        // =================================================================
        // Internal helpers
        // =================================================================

        /// Panics if the caller is not the owner.
        fn only_owner(&self) {
            let owner = self.owner.expect("ProtocolFee: no owner set");
            assert!(
                caller::effective_caller() == owner,
                "ProtocolFee: caller is not the owner"
            );
        }
    }
}
