// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane InterchainGasPaymaster hook contract for Dusk.

//! Post-dispatch hook that calculates interchain gas costs.
//!
//! This is the Dusk equivalent of `InterchainGasPaymaster.sol`. It stores
//! per-domain gas oracle data and computes the DUSK cost of relaying
//! a message to a remote chain based on the gas limit, remote gas price,
//! and token exchange rate.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane InterchainGasPaymaster hook contract.
#[dusk_forge::contract(events = [
    events::BeneficiarySet,
    events::DomainGasConfigSet,
    events::GasPayment,
    events::Initialized,
    events::OwnershipTransferred,
])]
mod igp {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::{ContractToAccount, ReceiveFromContract, TRANSFER_CONTRACT};

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::metadata;
    use hyperlane_dusk_types::DomainGasConfig;
    use hyperlane_dusk_types::GasPaymentRecord;
    use hyperlane_dusk_types::H256;

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Scale factor for token exchange rates (1e10).
    const TOKEN_EXCHANGE_RATE_SCALE: u128 = 10_000_000_000;
    /// Maximum destination gas limit accepted by this u64-priced route.
    const MAX_GAS_LIMIT: u64 = 1_000_000_000;

    /// InterchainGasPaymaster contract state.
    pub struct InterchainGasPaymaster {
        /// Owner identity (Moonlight account hash or contract ID).
        owner: Option<H256>,
        /// Mailbox authorized to prepare and send hook payments.
        mailbox: ContractId,
        /// Moonlight account hash that may claim gas payments.
        beneficiary: H256,
        /// Per-domain gas configuration (oracle data + overhead).
        domain_gas_configs: BTreeMap<u32, DomainGasConfig>,
        /// Total gas payments recorded (accounting).
        total_gas_payments: u64,
        /// Stored gas payment records (for off-chain indexing).
        gas_payments: Vec<GasPaymentRecord>,
        /// Native DUSK collected and not yet claimed by the beneficiary.
        claimable_fees: u64,
        /// Payment prepared by Mailbox and awaiting its authenticated native
        /// DUSK transfer callback.
        pending_payment: Option<GasPaymentRecord>,
    }

    impl InterchainGasPaymaster {
        /// Creates a new empty IGP state.
        pub const fn new() -> Self {
            Self {
                owner: None,
                mailbox: ZERO_CONTRACT,
                beneficiary: [0u8; 32],
                domain_gas_configs: BTreeMap::new(),
                total_gas_payments: 0,
                gas_payments: Vec::new(),
                claimable_fees: 0,
                pending_payment: None,
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the IGP hook.
        ///
        /// Must be called once after deployment. Optionally accepts initial
        /// gas configurations for known domains.
        pub fn init(
            &mut self,
            mailbox: ContractId,
            owner: H256,
            beneficiary: H256,
            initial_configs: Vec<(u32, DomainGasConfig)>,
        ) {
            assert!(self.owner.is_none(), "IGP: already initialized");
            assert!(owner != [0u8; 32], "IGP: owner cannot be zero");
            assert!(mailbox != ZERO_CONTRACT, "IGP: mailbox cannot be zero");
            assert!(beneficiary != [0u8; 32], "IGP: beneficiary cannot be zero");
            self.mailbox = mailbox;
            self.owner = Some(owner);
            self.beneficiary = beneficiary;
            for (domain, config) in initial_configs {
                Self::validate_domain_gas_config(config);
                self.domain_gas_configs.insert(domain, config);
                abi::emit(
                    events::DomainGasConfigSet::TOPIC,
                    events::DomainGasConfigSet { domain, config },
                );
            }
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_IGP,
                    owner,
                    mailbox: mailbox.to_bytes(),
                    local_domain: 0,
                },
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
        /// Calculates and prepares the exact payment expected from Mailbox.
        pub fn post_dispatch(&mut self, hook_metadata: Vec<u8>, encoded_message: Vec<u8>) {
            assert!(
                abi::caller() == Some(self.mailbox),
                "IGP: caller is not mailbox"
            );
            assert!(
                self.pending_payment.is_none(),
                "IGP: payment already pending"
            );
            let destination = message::destination(&encoded_message);
            let gas_limit = metadata::gas_limit(&hook_metadata);
            let payment = self.quote_gas_payment(destination, gas_limit);
            if payment == 0 {
                return;
            }
            let message_id = message::id(&encoded_message);
            self.pending_payment = Some(GasPaymentRecord {
                message_id,
                destination,
                gas_limit,
                payment,
                block_height: 0,
            });
        }

        /// Authenticate and record the native DUSK sent by Mailbox.
        pub fn receive_payment(&mut self, transfer: ReceiveFromContract) {
            assert!(
                caller::authentic_transfer_callback(transfer.contract, self.mailbox),
                "IGP: unauthenticated payment"
            );
            assert!(transfer.data.is_empty(), "IGP: unexpected payment data");
            let mut record = self
                .pending_payment
                .take()
                .expect("IGP: no payment pending");
            assert!(transfer.value == record.payment, "IGP: incorrect payment");

            self.total_gas_payments = self
                .total_gas_payments
                .checked_add(transfer.value)
                .expect("IGP: total gas payment overflow");
            self.claimable_fees = self
                .claimable_fees
                .checked_add(transfer.value)
                .expect("IGP: claimable fee overflow");
            record.block_height = abi::block_height();
            self.gas_payments.push(record);

            abi::emit(
                events::GasPayment::TOPIC,
                events::GasPayment {
                    message_id: record.message_id,
                    gas_limit: record.gas_limit,
                    payment: record.payment,
                },
            );
        }

        /// Returns the fee required for this hook.
        pub fn quote_dispatch(&self, hook_metadata: Vec<u8>, encoded_message: Vec<u8>) -> u64 {
            let destination = message::destination(&encoded_message);
            let gas_limit = metadata::gas_limit(&hook_metadata);
            self.quote_gas_payment(destination, gas_limit)
        }

        /// Returns the hook type identifier.
        #[allow(clippy::unused_self)]
        pub fn hook_type(&self) -> u8 {
            4 // HookType::Igp
        }

        /// Returns the persisted state layout version expected by deployment tooling.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            2
        }

        // =================================================================
        // Gas Payment Calculation
        // =================================================================

        /// Calculate the gas payment for a destination domain and gas limit.
        ///
        /// Formula: `(adjusted_gas * gas_price * exchange_rate) / 1e10`
        /// where `adjusted_gas = gas_limit + gas_overhead`.
        pub fn quote_gas_payment(&self, destination: u32, gas_limit: u64) -> u64 {
            assert!(gas_limit > 0, "IGP: gas limit cannot be zero");
            assert!(
                gas_limit <= MAX_GAS_LIMIT,
                "IGP: gas limit exceeds supported maximum"
            );
            let config = self
                .domain_gas_configs
                .get(&destination)
                .expect("IGP: destination is not configured");
            let adjusted_gas = u128::from(gas_limit) + u128::from(config.gas_overhead);
            let cost = adjusted_gas
                .checked_mul(u128::from(config.gas_price))
                .expect("IGP: gas price overflow")
                .checked_mul(u128::from(config.token_exchange_rate))
                .expect("IGP: exchange rate overflow")
                / TOKEN_EXCHANGE_RATE_SCALE;
            let payment = u64::try_from(cost).expect("IGP: fee exceeds u64");
            assert!(payment > 0, "IGP: configured payment rounds to zero");
            payment
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the gas configuration for a domain.
        pub fn domain_gas_config(&self, domain: u32) -> DomainGasConfig {
            self.domain_gas_configs
                .get(&domain)
                .copied()
                .unwrap_or_default()
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

        /// Returns the total gas payments recorded.
        pub fn total_gas_payments(&self) -> u64 {
            self.total_gas_payments
        }

        /// Returns the number of gas payment records stored.
        pub fn gas_payment_count(&self) -> u32 {
            u32::try_from(self.gas_payments.len()).expect("IGP: gas payment count overflow")
        }

        /// Returns the Nth gas payment record.
        ///
        /// Panics if the index is out of range.
        pub fn gas_payment_at(&self, index: u32) -> GasPaymentRecord {
            self.gas_payments[index as usize]
        }

        /// Returns a bounded consecutive page of gas-payment records.
        pub fn gas_payments(&self, start: u32, limit: u32) -> Vec<GasPaymentRecord> {
            const MAX_PAGE_SIZE: usize = 256;
            let start = start as usize;
            assert!(
                start <= self.gas_payments.len(),
                "IGP: gas payment index out of bounds"
            );
            let limit = usize::try_from(limit).expect("IGP: page size overflow");
            assert!(limit <= MAX_PAGE_SIZE, "IGP: page too large");
            let end = start.saturating_add(limit).min(self.gas_payments.len());
            self.gas_payments[start..end].to_vec()
        }

        /// Returns collected native DUSK not yet claimed by the beneficiary.
        pub fn claimable_fees(&self) -> u64 {
            self.claimable_fees
        }

        /// Transfer all claimable native DUSK to the beneficiary account.
        pub fn claim(&mut self) {
            assert!(
                caller::effective_caller() == self.beneficiary,
                "IGP: caller is not beneficiary"
            );
            let account = abi::public_sender().expect("IGP: beneficiary must be Moonlight");
            let amount = self.claimable_fees;
            assert!(amount > 0, "IGP: no fees to claim");
            self.claimable_fees = 0;
            let transfer = ContractToAccount {
                account,
                value: amount,
            };
            let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_account", &transfer)
                .expect("IGP: claim transfer failed");
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Set the gas configuration for a single domain. Owner only.
        pub fn set_domain_gas_config(&mut self, domain: u32, config: DomainGasConfig) {
            self.only_owner();
            Self::validate_domain_gas_config(config);
            self.domain_gas_configs.insert(domain, config);
            abi::emit(
                events::DomainGasConfigSet::TOPIC,
                events::DomainGasConfigSet { domain, config },
            );
        }

        /// Set gas configurations for multiple domains. Owner only.
        pub fn set_domain_gas_configs(&mut self, configs: Vec<(u32, DomainGasConfig)>) {
            self.only_owner();
            for (domain, config) in configs {
                Self::validate_domain_gas_config(config);
                self.domain_gas_configs.insert(domain, config);
                abi::emit(
                    events::DomainGasConfigSet::TOPIC,
                    events::DomainGasConfigSet { domain, config },
                );
            }
        }

        /// Set the beneficiary. Owner only.
        pub fn set_beneficiary(&mut self, beneficiary: H256) {
            self.only_owner();
            assert!(beneficiary != [0u8; 32], "IGP: beneficiary cannot be zero");
            self.beneficiary = beneficiary;
            abi::emit(
                events::BeneficiarySet::TOPIC,
                events::BeneficiarySet { beneficiary },
            );
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            assert!(new_owner != [0u8; 32], "IGP: new owner cannot be zero");
            let previous_owner = self.owner.expect("IGP: no owner set");
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
            let owner = self.owner.expect("IGP: no owner set");
            assert!(
                caller::effective_caller() == owner,
                "IGP: caller is not the owner"
            );
        }

        fn validate_domain_gas_config(config: DomainGasConfig) {
            assert!(config.gas_price > 0, "IGP: gas price cannot be zero");
            assert!(
                config.token_exchange_rate > 0,
                "IGP: token exchange rate cannot be zero"
            );
            // A dispatch gas limit must be nonzero and bounded. Prove that
            // every valid input from 1 through MAX_GAS_LIMIT is executable when the config is
            // accepted, rather than storing a configuration that later
            // overflows or rounds its smallest quote to zero.
            let minimum = (1u128 + u128::from(config.gas_overhead))
                .checked_mul(u128::from(config.gas_price))
                .and_then(|value| value.checked_mul(u128::from(config.token_exchange_rate)))
                .expect("IGP: configured quote arithmetic overflows")
                / TOKEN_EXCHANGE_RATE_SCALE;
            assert!(minimum > 0, "IGP: configured payment rounds to zero");

            let maximum = (u128::from(MAX_GAS_LIMIT) + u128::from(config.gas_overhead))
                .checked_mul(u128::from(config.gas_price))
                .and_then(|value| value.checked_mul(u128::from(config.token_exchange_rate)))
                .expect("IGP: configured quote arithmetic overflows")
                / TOKEN_EXCHANGE_RATE_SCALE;
            assert!(
                u64::try_from(maximum).is_ok(),
                "IGP: configured quote exceeds u64"
            );
        }
    }
}
