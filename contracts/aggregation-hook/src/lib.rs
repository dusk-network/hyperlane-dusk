// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane static aggregation hook contract for Dusk.

//! Post-dispatch hook that invokes a fixed set of child hooks and forwards each
//! child's exact quoted native DUSK payment.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]

/// Hyperlane static aggregation hook contract.
#[dusk_forge::contract(events = [events::Initialized])]
mod aggregation_hook {
    extern crate alloc;

    use alloc::string::String;
    use alloc::vec::Vec;
    use core::mem;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::{ContractToContract, ReceiveFromContract, TRANSFER_CONTRACT};

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::events;

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Static aggregation hook state.
    pub struct AggregationHook {
        /// Mailbox authorized to invoke and fund the aggregation.
        mailbox: ContractId,
        /// Fixed child hook list.
        hooks: Vec<ContractId>,
        /// Child payments prepared by `post_dispatch` and awaiting the exact
        /// authenticated Mailbox transfer.
        pending_payments: Vec<(ContractId, u64)>,
    }

    impl AggregationHook {
        /// Creates an empty aggregation hook.
        pub const fn new() -> Self {
            Self {
                mailbox: ZERO_CONTRACT,
                hooks: Vec::new(),
                pending_payments: Vec::new(),
            }
        }

        /// Initialize with the Mailbox and a fixed, non-empty child hook list.
        pub fn init(&mut self, mailbox: ContractId, hooks: Vec<ContractId>) {
            assert!(
                self.mailbox == ZERO_CONTRACT,
                "AggregationHook: already initialized"
            );
            assert!(mailbox != ZERO_CONTRACT, "AggregationHook: mailbox is zero");
            assert!(!hooks.is_empty(), "AggregationHook: hooks are empty");
            assert!(
                hooks.iter().all(|hook| *hook != ZERO_CONTRACT),
                "AggregationHook: child hook is zero"
            );
            self.mailbox = mailbox;
            self.hooks = hooks;
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_AGGREGATION_HOOK,
                    owner: ZERO_CONTRACT.to_bytes(),
                    mailbox: mailbox.to_bytes(),
                    local_domain: 0,
                },
            );
        }

        /// Prepare every child hook and the exact native payments to forward.
        pub fn post_dispatch(&mut self, metadata: Vec<u8>, encoded_message: Vec<u8>) {
            assert!(
                abi::caller() == Some(self.mailbox),
                "AggregationHook: caller is not mailbox"
            );
            assert!(
                self.pending_payments.is_empty(),
                "AggregationHook: payment already pending"
            );

            for hook in &self.hooks {
                let fee: u64 = abi::call(
                    *hook,
                    "quote_dispatch",
                    &(metadata.clone(), encoded_message.clone()),
                )
                .expect("AggregationHook: child quote failed");
                let _: () = abi::call(
                    *hook,
                    "post_dispatch",
                    &(metadata.clone(), encoded_message.clone()),
                )
                .expect("AggregationHook: child post_dispatch failed");
                if fee > 0 {
                    self.pending_payments.push((*hook, fee));
                }
            }
        }

        /// Authenticate the Mailbox payment and forward each child's exact fee.
        pub fn receive_payment(&mut self, transfer: ReceiveFromContract) {
            assert!(
                caller::authentic_transfer_callback(transfer.contract, self.mailbox),
                "AggregationHook: unauthenticated payment"
            );
            assert!(
                transfer.data.is_empty(),
                "AggregationHook: unexpected payment data"
            );

            let payments = mem::take(&mut self.pending_payments);
            assert!(!payments.is_empty(), "AggregationHook: no payment pending");
            let expected = payments
                .iter()
                .try_fold(0u64, |total, (_, fee)| total.checked_add(*fee));
            assert!(
                expected == Some(transfer.value),
                "AggregationHook: incorrect payment"
            );

            for (hook, fee) in payments {
                let child_transfer = ContractToContract {
                    contract: hook,
                    value: fee,
                    fn_name: String::from("receive_payment"),
                    data: Vec::new(),
                };
                let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_contract", &child_transfer)
                    .expect("AggregationHook: child payment failed");
            }
        }

        /// Return the sum of all child hook quotes.
        pub fn quote_dispatch(&self, metadata: Vec<u8>, encoded_message: Vec<u8>) -> u64 {
            self.hooks.iter().fold(0u64, |total, hook| {
                let fee: u64 = abi::call(
                    *hook,
                    "quote_dispatch",
                    &(metadata.clone(), encoded_message.clone()),
                )
                .expect("AggregationHook: child quote failed");
                total
                    .checked_add(fee)
                    .expect("AggregationHook: fee overflow")
            })
        }

        /// Return the Hyperlane aggregation hook type identifier.
        #[allow(clippy::unused_self)]
        pub fn hook_type(&self) -> u8 {
            2
        }

        /// Returns the persisted state layout version expected by deployment tooling.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            1
        }

        /// Return the configured Mailbox.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }

        /// Return the fixed child hook list.
        pub fn hooks(&self) -> Vec<ContractId> {
            self.hooks.clone()
        }
    }
}
