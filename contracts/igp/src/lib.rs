// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane InterchainGasPaymaster hook contract for Dusk.

//! Post-dispatch hook that calculates interchain gas costs.
//!
//! This is the Dusk equivalent of `InterchainGasPaymaster.sol`. It stores
//! per-domain gas oracle data and computes the DUSK cost of relaying
//! a message to a remote chain based on the gas limit, remote gas price,
//! and token exchange rate. A message pays when it is dispatched, and anyone
//! can pay for more gas for it later with `pay_for_gas`.

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
    use hyperlane_dusk_types::storage::ChunkedLog;
    use hyperlane_dusk_types::DomainGasConfig;
    use hyperlane_dusk_types::GasPaymentRecord;
    use hyperlane_dusk_types::{MessageId, H256};

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Scale factor for token exchange rates (1e10).
    const TOKEN_EXCHANGE_RATE_SCALE: u128 = 10_000_000_000;
    /// Maximum destination gas limit accepted by this u64-priced route.
    const MAX_GAS_LIMIT: u64 = 1_000_000_000;
    /// Most records one `gas_payments` query returns.
    const MAX_PAGE_SIZE: u32 = 256;

    /// Size of a wasm32 memory page.
    const WASM_PAGE_BYTES: u64 = 64 * 1024;
    /// Memory `pay_for_gas` never grows this contract past: 3 GiB of the
    /// 4 GiB a wasm32 contract can address. Memory never shrinks, so the
    /// last GiB stays for the payments dispatches make: top-ups, which cost
    /// a transaction each but no fee beyond the gas they buy, can't fill the
    /// log both write to and make dispatches through the IGP revert.
    const TOP_UP_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;
    /// Bytes dlmalloc adds to a request before growing memory for it: the
    /// chunk header and alignment, and the segment footer it keeps at the top
    /// (at most 63 bytes on wasm32 in dlmalloc 0.2), as in the Mailbox.
    const ALLOCATOR_OVERHEAD: u64 = 64;

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
        /// Gas payment records in payment order, for agents: dispatches'
        /// payments and top-ups alike. A record takes 64 bytes, 1024 to a
        /// chunk.
        gas_payments: ChunkedLog<GasPaymentRecord>,
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
                gas_payments: ChunkedLog::new(),
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
            // Credit the full destination gas purchased by the quote, including
            // overhead, and retain it independently of later oracle updates.
            let gas_limit = gas_limit
                .checked_add(self.domain_gas_configs[&destination].gas_overhead)
                .expect("IGP: adjusted gas exceeds u64");
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
            let record = self
                .pending_payment
                .take()
                .expect("IGP: no payment pending");
            assert!(transfer.value == record.payment, "IGP: incorrect payment");
            self.record_payment(record);
        }

        /// Pay for `gas_amount` more gas on `destination` for the message
        /// `message_id`, as `payForGas` does on EVM: anyone may pay for any
        /// message, and the relayer adds up all the payments for a message
        /// ID and destination. A message whose dispatch paid for too little
        /// gas, or whose delivery got more expensive, is relayed once enough
        /// is paid for it.
        ///
        /// The payment is the native DUSK the calling Moonlight transaction
        /// deposits, so this must be the transaction's own call. `deposit` is
        /// that deposit: the most the payer pays. The IGP takes all of it,
        /// keeps what [`Self::quote_pay_for_gas`] charges at this moment and
        /// sends the rest back to the account that signed the transaction,
        /// so a deposit with some margin survives a price update between
        /// quoting and inclusion. The gas bought is `gas_amount`, with no
        /// overhead added: the message's dispatch paid for that.
        ///
        /// As on EVM, the message isn't looked up: a payment for an ID the
        /// Mailbox never dispatched, or for another destination than the
        /// message's, is recorded like any other and only costs its payer.
        ///
        /// Records the payment, with its destination, and emits the same
        /// `GasPayment` event a dispatch's payment does. Refuses once the
        /// contract's memory would pass 3 GiB, leaving the rest to the
        /// payments dispatches make.
        pub fn pay_for_gas(
            &mut self,
            message_id: MessageId,
            destination: u32,
            gas_amount: u64,
            deposit: u64,
        ) {
            // Only the transaction's own call can take its deposit, and the
            // refund then goes back to the account the deposit came from.
            assert!(
                abi::callstack().len() == 1 && abi::caller() == Some(TRANSFER_CONTRACT),
                "IGP: payer must call directly"
            );
            let payer = abi::public_sender().expect("IGP: payer must be Moonlight");
            let payment = self.quote_pay_for_gas(destination, gas_amount);
            assert!(
                deposit >= payment,
                "IGP: insufficient interchain gas payment"
            );
            assert!(
                memory_bytes_used() + self.top_up_memory_growth() <= TOP_UP_MEMORY_LIMIT,
                "IGP: storage full, top-ups are closed"
            );

            // The transfer contract moves the transaction's deposit to this
            // contract once, and only if it is exactly `deposit`. Nothing is
            // recorded unless it did.
            let _: () =
                abi::call(TRANSFER_CONTRACT, "deposit", &deposit).expect("IGP: deposit failed");
            self.record_payment(GasPaymentRecord {
                message_id,
                destination,
                gas_limit: gas_amount,
                payment,
                block_height: 0,
            });

            let refund = deposit - payment;
            if refund > 0 {
                let transfer = ContractToAccount {
                    account: payer,
                    value: refund,
                };
                let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_account", &transfer)
                    .expect("IGP: refund failed");
            }
        }

        /// What [`Self::pay_for_gas`] charges for `gas_amount` gas on
        /// `destination`, in LUX:
        /// `gas_amount * gas_price * exchange_rate / 1e10`, with no overhead.
        /// [`Self::quote_gas_payment`] adds the overhead, as a dispatch does.
        pub fn quote_pay_for_gas(&self, destination: u32, gas_amount: u64) -> u64 {
            assert!(gas_amount > 0, "IGP: gas amount cannot be zero");
            assert!(
                gas_amount <= MAX_GAS_LIMIT,
                "IGP: gas amount exceeds supported maximum"
            );
            let config = self
                .domain_gas_configs
                .get(&destination)
                .expect("IGP: destination is not configured");
            // `validate_domain_gas_config` showed that the largest dispatch
            // quote, which adds the overhead to MAX_GAS_LIMIT, fits.
            let cost = u128::from(gas_amount)
                .checked_mul(u128::from(config.gas_price))
                .expect("IGP: gas price overflow")
                .checked_mul(u128::from(config.token_exchange_rate))
                .expect("IGP: exchange rate overflow")
                / TOKEN_EXCHANGE_RATE_SCALE;
            let payment = u64::try_from(cost).expect("IGP: fee exceeds u64");
            assert!(payment > 0, "IGP: payment rounds to zero");
            payment
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
        ///
        /// Version 3 stores payment records in chunks. Version 4 adds
        /// `pay_for_gas` and `quote_pay_for_gas`; the state, the other calls
        /// and the events are unchanged.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            4
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
            let adjusted_gas = u128::from(
                gas_limit
                    .checked_add(config.gas_overhead)
                    .expect("IGP: adjusted gas exceeds u64"),
            );
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
            self.gas_payments.len()
        }

        /// Returns the Nth gas payment record.
        ///
        /// Panics if the index is out of range.
        pub fn gas_payment_at(&self, index: u32) -> GasPaymentRecord {
            self.gas_payments
                .get(index)
                .expect("IGP: gas payment index out of bounds")
        }

        /// Returns a bounded consecutive page of gas-payment records.
        pub fn gas_payments(&self, start: u32, limit: u32) -> Vec<GasPaymentRecord> {
            assert!(
                start <= self.gas_payments.len(),
                "IGP: gas payment index out of bounds"
            );
            assert!(limit <= MAX_PAGE_SIZE, "IGP: page too large");
            self.gas_payments.page(start, limit)
        }

        /// Returns collected native DUSK not yet claimed by the beneficiary.
        pub fn claimable_fees(&self) -> u64 {
            self.claimable_fees
        }

        /// Transfer all claimable native DUSK to the beneficiary account.
        pub fn claim(&mut self) {
            assert!(
                abi::callstack().len() == 1,
                "IGP: beneficiary must call directly"
            );
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

        /// Account for a payment whose DUSK this contract holds, append its
        /// record at the current height and emit its `GasPayment` event:
        /// alike for a dispatch's payment and a top-up, so agents read both
        /// the same way.
        fn record_payment(&mut self, mut record: GasPaymentRecord) {
            self.total_gas_payments = self
                .total_gas_payments
                .checked_add(record.payment)
                .expect("IGP: total gas payment overflow");
            self.claimable_fees = self
                .claimable_fees
                .checked_add(record.payment)
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

        /// The most a top-up can still grow this contract's memory: the
        /// chunk of records its record may start and the larger chunk table
        /// that may need, each in whole pages with dlmalloc's overhead.
        /// Nothing else it does allocates.
        fn top_up_memory_growth(&self) -> u64 {
            self.gas_payments
                .push_allocations()
                .into_iter()
                .map(memory_growth)
                .sum()
        }

        fn validate_domain_gas_config(config: DomainGasConfig) {
            assert!(
                config.gas_overhead.checked_add(MAX_GAS_LIMIT).is_some(),
                "IGP: configured adjusted gas exceeds u64"
            );
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

    /// Current size of the contract's linear memory.
    fn memory_bytes_used() -> u64 {
        core::arch::wasm32::memory_size(0) as u64 * WASM_PAGE_BYTES
    }

    /// The most an allocation of `bytes` can grow memory by. When no free
    /// block fits, dlmalloc asks for whole pages: enough for the request and
    /// its own overhead.
    fn memory_growth(bytes: usize) -> u64 {
        if bytes == 0 {
            return 0;
        }
        (bytes as u64 + ALLOCATOR_OVERHEAD).next_multiple_of(WASM_PAGE_BYTES)
    }
}
