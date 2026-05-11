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
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane InterchainGasPaymaster hook contract.
#[dusk_forge::contract]
mod igp {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::metadata;
    use hyperlane_dusk_types::DomainGasConfig;
    use hyperlane_dusk_types::GasPaymentRecord;

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Scale factor for token exchange rates (1e10).
    const TOKEN_EXCHANGE_RATE_SCALE: u128 = 10_000_000_000;

    /// InterchainGasPaymaster contract state.
    pub struct InterchainGasPaymaster {
        /// Contract owner.
        owner: Option<ContractId>,
        /// Beneficiary that receives gas payments.
        beneficiary: ContractId,
        /// Per-domain gas configuration (oracle data + overhead).
        domain_gas_configs: BTreeMap<u32, DomainGasConfig>,
        /// Total gas payments recorded (accounting).
        total_gas_payments: u64,
        /// Stored gas payment records (for off-chain indexing).
        gas_payments: Vec<GasPaymentRecord>,
    }

    impl InterchainGasPaymaster {
        /// Creates a new empty IGP state.
        pub const fn new() -> Self {
            Self {
                owner: None,
                beneficiary: ZERO_CONTRACT,
                domain_gas_configs: BTreeMap::new(),
                total_gas_payments: 0,
                gas_payments: Vec::new(),
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the IGP hook.
        ///
        /// Must be called once after deployment. Optionally accepts initial
        /// gas configurations for known domains.
        #[contract(emits = [
            (events::Initialized::TOPIC, events::Initialized),
            (events::BeneficiarySet::TOPIC, events::BeneficiarySet),
            (events::DomainGasConfigSet::TOPIC, events::DomainGasConfigSet)
        ])]
        pub fn init(
            &mut self,
            owner: ContractId,
            beneficiary: ContractId,
            initial_configs: Vec<(u32, DomainGasConfig)>,
        ) {
            assert!(self.owner.is_none(), "IGP: already initialized");
            assert!(
                beneficiary != ZERO_CONTRACT,
                "IGP: beneficiary cannot be zero"
            );
            self.owner = Some(owner);
            self.beneficiary = beneficiary;
            for (domain, config) in initial_configs {
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
                    owner: owner.to_bytes(),
                    mailbox: ZERO_CONTRACT.to_bytes(),
                    local_domain: 0,
                },
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
        /// Calculates the gas payment for the message's destination,
        /// records the payment, and emits a `GasPayment` event.
        #[contract(emits = [(events::GasPayment::TOPIC, events::GasPayment)])]
        pub fn post_dispatch(&mut self, hook_metadata: Vec<u8>, encoded_message: Vec<u8>) {
            let destination = message::destination(&encoded_message);
            let gas_limit = metadata::gas_limit(&hook_metadata);
            let payment = self.quote_gas_payment(destination, gas_limit);
            let message_id = message::id(&encoded_message);

            self.total_gas_payments = self.total_gas_payments.saturating_add(payment);
            self.gas_payments.push(GasPaymentRecord {
                message_id,
                destination,
                gas_limit,
                payment,
                block_height: abi::block_height(),
            });

            abi::emit(
                events::GasPayment::TOPIC,
                events::GasPayment {
                    message_id,
                    gas_limit,
                    payment,
                },
            );
        }

        /// Returns the fee required for this hook.
        pub fn quote_dispatch(
            &self,
            hook_metadata: Vec<u8>,
            encoded_message: Vec<u8>,
        ) -> u64 {
            let destination = message::destination(&encoded_message);
            let gas_limit = metadata::gas_limit(&hook_metadata);
            self.quote_gas_payment(destination, gas_limit)
        }

        /// Returns the hook type identifier.
        #[allow(clippy::unused_self)]
        pub fn hook_type(&self) -> u8 {
            4 // HookType::Igp
        }

        // =================================================================
        // Gas Payment Calculation
        // =================================================================

        /// Calculate the gas payment for a destination domain and gas limit.
        ///
        /// Formula: `(adjusted_gas * gas_price * exchange_rate) / 1e10`
        /// where `adjusted_gas = gas_limit + gas_overhead`.
        pub fn quote_gas_payment(&self, destination: u32, gas_limit: u64) -> u64 {
            let config = self.domain_gas_configs.get(&destination);

            match config {
                Some(config) => {
                    let adjusted_gas = u128::from(gas_limit) + u128::from(config.gas_overhead);
                    let cost = adjusted_gas
                        .checked_mul(u128::from(config.gas_price))
                        .expect("IGP: gas price overflow")
                        .checked_mul(u128::from(config.token_exchange_rate))
                        .expect("IGP: exchange rate overflow")
                        / TOKEN_EXCHANGE_RATE_SCALE;
                    u64::try_from(cost).expect("IGP: fee exceeds u64")
                }
                None => 0,
            }
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

        /// Returns the beneficiary contract ID.
        pub fn beneficiary(&self) -> ContractId {
            self.beneficiary
        }

        /// Returns the owner contract ID.
        pub fn owner(&self) -> Option<ContractId> {
            self.owner
        }

        /// Returns the total gas payments recorded.
        pub fn total_gas_payments(&self) -> u64 {
            self.total_gas_payments
        }

        /// Returns the number of gas payment records stored.
        pub fn gas_payment_count(&self) -> u32 {
            self.gas_payments.len() as u32
        }

        /// Returns the Nth gas payment record.
        ///
        /// Panics if the index is out of range.
        pub fn gas_payment_at(&self, index: u32) -> GasPaymentRecord {
            self.gas_payments[index as usize]
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Set the gas configuration for a single domain. Owner only.
        #[contract(emits = [(events::DomainGasConfigSet::TOPIC, events::DomainGasConfigSet)])]
        pub fn set_domain_gas_config(
            &mut self,
            domain: u32,
            config: DomainGasConfig,
        ) {
            self.only_owner();
            self.domain_gas_configs.insert(domain, config);
            abi::emit(
                events::DomainGasConfigSet::TOPIC,
                events::DomainGasConfigSet { domain, config },
            );
        }

        /// Set gas configurations for multiple domains. Owner only.
        #[contract(emits = [(events::DomainGasConfigSet::TOPIC, events::DomainGasConfigSet)])]
        pub fn set_domain_gas_configs(
            &mut self,
            configs: Vec<(u32, DomainGasConfig)>,
        ) {
            self.only_owner();
            for (domain, config) in configs {
                self.domain_gas_configs.insert(domain, config);
                abi::emit(
                    events::DomainGasConfigSet::TOPIC,
                    events::DomainGasConfigSet { domain, config },
                );
            }
        }

        /// Set the beneficiary. Owner only.
        #[contract(emits = [(events::BeneficiarySet::TOPIC, events::BeneficiarySet)])]
        pub fn set_beneficiary(&mut self, beneficiary: ContractId) {
            self.only_owner();
            assert!(
                beneficiary != ZERO_CONTRACT,
                "IGP: beneficiary cannot be zero"
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
            let previous_owner = self.owner.expect("IGP: no owner set");
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
            let caller = abi::caller().expect("IGP: cannot determine caller");
            let owner = self.owner.expect("IGP: no owner set");
            assert!(caller == owner, "IGP: caller is not the owner");
        }
    }
}
