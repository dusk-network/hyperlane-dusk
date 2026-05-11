// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane WarpDrc20Collateral contract for Dusk.

//! A collateral warp route that locks/unlocks an existing DRC20 token
//! for cross-chain transfers via Hyperlane.
//!
//! When sending tokens to a remote chain, this contract locks the DRC20
//! tokens (via `transfer_from`). When receiving tokens from a remote chain,
//! it unlocks them (via `transfer`) to the recipient.
//!
//! Equivalent to `HypERC20Collateral` in the Solidity Hyperlane contracts.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane WarpDrc20Collateral contract.
#[dusk_forge::contract]
mod warp_drc20_collateral {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;
    use core::cmp::Ordering;

    use bytecheck::CheckBytes;
    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::signatures::bls::PublicKey as AccountPublicKey;
    use rkyv::{Archive, Deserialize, Serialize};

    use dusk_bytes::Serializable;

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::token_message;
    use hyperlane_dusk_types::{H256, MessageId};

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    // =====================================================================
    // Account type (DRC20-compatible)
    // =====================================================================

    /// A DRC20 account — must be rkyv-layout-compatible with the DRC20
    /// reference implementation's `Account` type.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
    #[archive_attr(derive(CheckBytes))]
    pub enum Account {
        /// An externally owned account.
        External(AccountPublicKey),
        /// A contract account.
        Contract(ContractId),
    }

    impl PartialOrd for Account {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for Account {
        fn cmp(&self, other: &Self) -> Ordering {
            match (self, other) {
                (Account::External(a), Account::External(b)) => {
                    a.to_raw_bytes().cmp(&b.to_raw_bytes())
                }
                (Account::Contract(a), Account::Contract(b)) => a.cmp(b),
                (Account::External(_), Account::Contract(_)) => Ordering::Less,
                (Account::Contract(_), Account::External(_)) => Ordering::Greater,
            }
        }
    }

    /// Resolve the caller as an Account.
    fn sender_account() -> Account {
        if abi::callstack().len() == 1 {
            Account::External(
                abi::public_sender()
                    .expect("WarpCollateral: shielded transactions not supported"),
            )
        } else {
            Account::Contract(abi::caller().expect("WarpCollateral: missing caller"))
        }
    }

    // =====================================================================
    // State
    // =====================================================================

    /// WarpDrc20Collateral contract state.
    pub struct WarpDrc20Collateral {
        /// The DRC20 token contract to lock/unlock.
        wrapped_token: ContractId,
        /// Mailbox contract ID.
        mailbox: ContractId,
        /// Post-dispatch hook override (zero = use Mailbox default).
        hook: ContractId,
        /// ISM override (zero = use Mailbox default).
        ism: ContractId,
        /// Contract owner.
        owner: Option<ContractId>,
        /// Enrolled remote routers per domain.
        enrolled_routers: BTreeMap<u32, H256>,
        /// Registry of external accounts: keccak256(pk.to_bytes()) → pk.
        ///
        /// External Dusk accounts use 96-byte BLS public keys that don't
        /// fit in a 32-byte H256. Users must call `register_account` once
        /// so that inbound transfers can resolve to External accounts.
        registered_accounts: BTreeMap<H256, AccountPublicKey>,
    }

    impl WarpDrc20Collateral {
        /// Creates a new empty state.
        pub const fn new() -> Self {
            Self {
                wrapped_token: ZERO_CONTRACT,
                mailbox: ZERO_CONTRACT,
                hook: ZERO_CONTRACT,
                ism: ZERO_CONTRACT,
                owner: None,
                enrolled_routers: BTreeMap::new(),
                registered_accounts: BTreeMap::new(),
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the collateral warp route.
        #[contract(no_event)]
        pub fn init(
            &mut self,
            wrapped_token: ContractId,
            mailbox: ContractId,
            owner: ContractId,
            enrolled_routers: Vec<(u32, H256)>,
        ) {
            assert!(
                self.owner.is_none(),
                "WarpCollateral: already initialized"
            );
            assert!(
                wrapped_token != ZERO_CONTRACT,
                "WarpCollateral: wrapped token cannot be zero"
            );
            self.wrapped_token = wrapped_token;
            self.mailbox = mailbox;
            self.owner = Some(owner);
            for (domain, router) in enrolled_routers {
                self.enrolled_routers.insert(domain, router);
            }
        }

        // =================================================================
        // Account registration
        // =================================================================

        /// Register the caller's BLS public key so that inbound transfers
        /// can resolve the H256 recipient to an External DRC20 account.
        ///
        /// Reads the sender from `abi::public_sender()` (Moonlight TX).
        /// Stores `keccak256(pk.to_bytes()) → pk`.
        #[contract(no_event)]
        pub fn register_account(&mut self) {
            let pk = abi::public_sender()
                .expect("WarpCollateral: register_account requires Moonlight TX");
            let h = message::keccak256(&pk.to_bytes());
            self.registered_accounts.insert(h, pk);
        }

        /// Check whether an H256 has a registered account.
        pub fn is_registered(&self, h: H256) -> bool {
            self.registered_accounts.contains_key(&h)
        }

        // =================================================================
        // Warp Route: transfer_remote (send — lock tokens)
        // =================================================================

        /// Send tokens to a remote chain by locking them in this contract.
        ///
        /// The caller must have approved this contract to spend `amount`
        /// of the wrapped DRC20 token via `approve()`.
        #[contract(emits = [(events::SentTransferRemote::TOPIC, events::SentTransferRemote)])]
        pub fn transfer_remote(
            &mut self,
            destination: u32,
            recipient: H256,
            amount: u64,
        ) -> MessageId {
            let sender = sender_account();
            let self_account = Account::Contract(abi::self_id());

            // Lock tokens: transfer_from(sender → this contract)
            let _: () = abi::call(
                self.wrapped_token,
                "transfer_from",
                &(sender, self_account, amount),
            )
            .expect("WarpCollateral: transfer_from failed");

            // Look up enrolled router
            let router = self
                .enrolled_routers
                .get(&destination)
                .expect("WarpCollateral: no router enrolled for destination");

            // Encode token message body
            let body = token_message::encode(recipient, amount);

            // Dispatch via Mailbox
            let message_id: MessageId = abi::call(
                self.mailbox,
                "dispatch",
                &(destination, *router, body, Vec::<u8>::new(), self.hook),
            )
            .expect("WarpCollateral: dispatch failed");

            abi::emit(
                events::SentTransferRemote::TOPIC,
                events::SentTransferRemote {
                    destination,
                    recipient,
                    amount,
                },
            );

            message_id
        }

        // =================================================================
        // Warp Route: handle (receive — unlock tokens)
        // =================================================================

        /// Handle an incoming cross-chain message.
        ///
        /// Called by the Mailbox when a message is delivered. Unlocks
        /// wrapped DRC20 tokens to the recipient.
        #[contract(emits = [(events::ReceivedTransferRemote::TOPIC, events::ReceivedTransferRemote)])]
        pub fn handle(&mut self, origin: u32, sender: H256, body: Vec<u8>) {
            // Verify caller is the Mailbox
            let caller = abi::caller().expect("WarpCollateral: cannot determine caller");
            assert!(
                caller == self.mailbox,
                "WarpCollateral: caller is not the Mailbox"
            );

            // Verify sender is an enrolled router
            let enrolled = self.enrolled_routers.get(&origin);
            assert!(
                enrolled.is_some() && *enrolled.unwrap() == sender,
                "WarpCollateral: sender is not enrolled router for origin"
            );

            // Decode token message
            let msg = token_message::decode(&body)
                .expect("WarpCollateral: invalid token message");

            // Resolve the recipient: if a BLS key is registered for this H256,
            // unlock to the External account; otherwise treat as Contract.
            let recipient_account =
                if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
                    Account::External(*pk)
                } else {
                    Account::Contract(ContractId::from_bytes(msg.recipient))
                };

            let _: () = abi::call(
                self.wrapped_token,
                "transfer",
                &(recipient_account, msg.amount),
            )
            .expect("WarpCollateral: transfer failed");

            abi::emit(
                events::ReceivedTransferRemote::TOPIC,
                events::ReceivedTransferRemote {
                    origin,
                    recipient: msg.recipient,
                    amount: msg.amount,
                },
            );
        }

        /// Returns the ISM override for this contract.
        pub fn interchain_security_module(&self) -> ContractId {
            self.ism
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the wrapped DRC20 token contract ID.
        pub fn wrapped_token(&self) -> ContractId {
            self.wrapped_token
        }

        /// Returns the Mailbox contract ID.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }

        /// Returns the hook override.
        pub fn hook(&self) -> ContractId {
            self.hook
        }

        /// Returns the owner.
        pub fn owner(&self) -> Option<ContractId> {
            self.owner
        }

        /// Returns the enrolled router for a domain.
        pub fn enrolled_router(&self, domain: u32) -> H256 {
            self.enrolled_routers
                .get(&domain)
                .copied()
                .unwrap_or([0u8; 32])
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Enroll a remote router for a domain. Owner only.
        #[contract(no_event)]
        pub fn enroll_remote_router(&mut self, domain: u32, router: H256) {
            self.only_owner();
            self.enrolled_routers.insert(domain, router);
        }

        /// Set the hook override. Owner only.
        #[contract(no_event)]
        pub fn set_hook(&mut self, hook: ContractId) {
            self.only_owner();
            self.hook = hook;
        }

        /// Set the ISM override. Owner only.
        #[contract(no_event)]
        pub fn set_ism(&mut self, ism: ContractId) {
            self.only_owner();
            self.ism = ism;
        }

        /// Transfer ownership. Owner only.
        #[contract(no_event)]
        pub fn transfer_ownership(&mut self, new_owner: ContractId) {
            self.only_owner();
            self.owner = Some(new_owner);
        }

        // =================================================================
        // Internal helpers
        // =================================================================

        /// Panics if the caller is not the owner.
        fn only_owner(&self) {
            let caller =
                abi::caller().expect("WarpCollateral: cannot determine caller");
            let owner = self.owner.expect("WarpCollateral: no owner set");
            assert!(
                caller == owner,
                "WarpCollateral: caller is not the owner"
            );
        }
    }
}
