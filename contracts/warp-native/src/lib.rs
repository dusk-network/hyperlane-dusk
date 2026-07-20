// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane WarpNative contract for Dusk.

//! A warp route for bridging native DUSK tokens via Hyperlane.
//!
//! When sending DUSK to a remote chain, the caller deposits DUSK into this
//! contract via the Moonlight TX `deposit` field. When receiving DUSK from
//! a remote chain, the contract sends DUSK to the recipient via the
//! `contract_to_account` transfer contract call.
//!
//! Equivalent to `HypNative` in the Solidity Hyperlane contracts.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane WarpNative contract.
#[dusk_forge::contract(events = [
    events::AccountRegistered,
    events::HookSet,
    events::Initialized,
    events::IsmSet,
    events::OwnershipTransferred,
    events::PendingTransferClaimed,
    events::ReceivedTransferRemote,
    events::RemoteRouterEnrolled,
    events::SentTransferRemote,
])]
mod warp_native {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    use dusk_bytes::Serializable;
    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::signatures::bls::PublicKey as AccountPublicKey;
    use dusk_core::transfer::{ContractToAccount, TRANSFER_CONTRACT};

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::token_message;
    use hyperlane_dusk_types::{MessageId, H256};

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    // =====================================================================
    // State
    // =====================================================================

    /// WarpNative contract state.
    pub struct WarpNative {
        /// Mailbox contract ID.
        mailbox: ContractId,
        /// Post-dispatch hook override (zero = use Mailbox default).
        hook: ContractId,
        /// ISM override (zero = use Mailbox default).
        ism: ContractId,
        /// Owner identity (Moonlight account hash or contract ID).
        owner: Option<H256>,
        /// Enrolled remote routers per domain.
        enrolled_routers: BTreeMap<u32, H256>,
        /// Registry of external accounts: keccak256(pk) → pk.
        ///
        /// External Dusk accounts use 96-byte BLS public keys that don't
        /// fit in a 32-byte H256. Users must call `register_account` once
        /// so that inbound transfers can look up the full key.
        registered_accounts: BTreeMap<H256, AccountPublicKey>,
        /// Pending transfers for recipients who haven't registered yet.
        /// The recipient can call `claim_pending` after registering to
        /// receive their DUSK.
        pending_transfers: BTreeMap<H256, u64>,
    }

    impl WarpNative {
        /// Creates a new empty state.
        pub const fn new() -> Self {
            Self {
                mailbox: ZERO_CONTRACT,
                hook: ZERO_CONTRACT,
                ism: ZERO_CONTRACT,
                owner: None,
                enrolled_routers: BTreeMap::new(),
                registered_accounts: BTreeMap::new(),
                pending_transfers: BTreeMap::new(),
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the native DUSK warp route.
        pub fn init(
            &mut self,
            mailbox: ContractId,
            owner: H256,
            enrolled_routers: Vec<(u32, H256)>,
        ) {
            assert!(self.owner.is_none(), "WarpNative: already initialized");
            assert!(owner != [0u8; 32], "WarpNative: owner cannot be zero");
            assert!(
                mailbox != ZERO_CONTRACT,
                "WarpNative: mailbox cannot be zero"
            );
            self.mailbox = mailbox;
            self.owner = Some(owner);
            for (domain, router) in enrolled_routers {
                self.enrolled_routers.insert(domain, router);
                abi::emit(
                    events::RemoteRouterEnrolled::TOPIC,
                    events::RemoteRouterEnrolled { domain, router },
                );
            }
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_WARP_NATIVE,
                    owner,
                    mailbox: mailbox.to_bytes(),
                    local_domain: 0,
                },
            );
        }

        // =================================================================
        // Account registration
        // =================================================================

        /// Register the caller's BLS public key so that inbound transfers
        /// can resolve the H256 recipient to a full account key.
        ///
        /// Reads the sender from `abi::public_sender()` (Moonlight TX).
        /// Stores `keccak256(pk.to_bytes()) → pk`.
        pub fn register_account(&mut self) {
            let pk =
                abi::public_sender().expect("WarpNative: register_account requires Moonlight TX");
            let h = message::keccak256(&pk.to_bytes());
            self.registered_accounts.insert(h, pk);
            abi::emit(
                events::AccountRegistered::TOPIC,
                events::AccountRegistered { account_hash: h },
            );
        }

        /// Check whether an H256 has a registered account.
        pub fn is_registered(&self, h: H256) -> bool {
            self.registered_accounts.contains_key(&h)
        }

        /// Claim pending DUSK that arrived before the caller registered.
        ///
        /// The caller must have previously called `register_account`.
        /// Transfers any escrowed DUSK to the caller's account.
        pub fn claim_pending(&mut self) {
            let pk = abi::public_sender().expect("WarpNative: claim_pending requires Moonlight TX");
            let h = message::keccak256(&pk.to_bytes());

            let amount = self.pending_transfers.remove(&h).unwrap_or(0);
            assert!(amount > 0, "WarpNative: no pending transfers");

            let transfer = ContractToAccount {
                account: pk,
                value: amount,
            };
            let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_account", &transfer)
                .expect("WarpNative: contract_to_account failed");
            abi::emit(
                events::PendingTransferClaimed::TOPIC,
                events::PendingTransferClaimed {
                    recipient: h,
                    amount,
                },
            );
        }

        /// Returns the pending (escrowed) balance for an H256 recipient.
        pub fn pending_balance(&self, h: H256) -> u64 {
            self.pending_transfers.get(&h).copied().unwrap_or(0)
        }

        // =================================================================
        // Warp Route: transfer_remote (send — lock DUSK)
        // =================================================================

        /// Send native DUSK to a remote chain.
        ///
        /// The caller must include a Moonlight TX with `deposit >= amount`
        /// to fund this contract. The amount is recorded in the Hyperlane
        /// message and the remote router mints equivalent tokens.
        pub fn transfer_remote(
            &mut self,
            destination: u32,
            recipient: H256,
            amount: u64,
        ) -> MessageId {
            assert!(amount > 0, "WarpNative: amount must be > 0");

            // Claim the DUSK deposit from the Moonlight TX.
            // The transfer contract validates that the TX deposit field
            // matches `amount` exactly and that the target is this contract.
            let _: () = abi::call(TRANSFER_CONTRACT, "deposit", &amount)
                .expect("WarpNative: deposit failed — TX deposit must equal amount");

            // Look up enrolled router
            let router = self
                .enrolled_routers
                .get(&destination)
                .expect("WarpNative: no router enrolled for destination");

            // Encode token message body
            let body = token_message::encode(recipient, amount);

            // Dispatch via Mailbox
            let message_id: MessageId = abi::call(
                self.mailbox,
                "dispatch",
                &(destination, *router, body, Vec::<u8>::new(), self.hook),
            )
            .expect("WarpNative: dispatch failed");

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
        // Warp Route: handle (receive — unlock DUSK)
        // =================================================================

        /// Handle an incoming cross-chain message.
        ///
        /// Called by the Mailbox when a message is delivered. Sends DUSK
        /// from this contract's balance to the recipient's registered
        /// account via the transfer contract.
        pub fn handle(&mut self, origin: u32, sender: H256, body: Vec<u8>) {
            // Verify caller is the Mailbox
            let caller = abi::caller().expect("WarpNative: cannot determine caller");
            assert!(
                caller == self.mailbox,
                "WarpNative: caller is not the Mailbox"
            );

            // Verify sender is an enrolled router
            let enrolled = self.enrolled_routers.get(&origin);
            assert!(
                matches!(enrolled, Some(enrolled_sender) if *enrolled_sender == sender),
                "WarpNative: sender is not enrolled router for origin"
            );

            // Decode token message
            let msg = token_message::decode(&body).expect("WarpNative: invalid token message");
            assert!(msg.amount > 0, "WarpNative: amount must be > 0");

            // Try to send DUSK to the recipient. If they're registered,
            // transfer directly. Otherwise, hold in escrow.
            if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
                let transfer = ContractToAccount {
                    account: *pk,
                    value: msg.amount,
                };
                let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_account", &transfer)
                    .expect("WarpNative: contract_to_account failed");
            } else {
                // Recipient not registered — hold funds in escrow.
                // They can call `claim_pending` after registering.
                let pending = self.pending_transfers.entry(msg.recipient).or_insert(0);
                *pending = pending
                    .checked_add(msg.amount)
                    .expect("WarpNative: pending overflow");
            }

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

        /// Returns the Mailbox contract ID.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }

        /// Returns the hook override.
        pub fn hook(&self) -> ContractId {
            self.hook
        }

        /// Returns the owner identity.
        pub fn owner(&self) -> Option<H256> {
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
        pub fn enroll_remote_router(&mut self, domain: u32, router: H256) {
            self.only_owner();
            self.enrolled_routers.insert(domain, router);
            abi::emit(
                events::RemoteRouterEnrolled::TOPIC,
                events::RemoteRouterEnrolled { domain, router },
            );
        }

        /// Set the hook override. Owner only.
        pub fn set_hook(&mut self, hook: ContractId) {
            self.only_owner();
            self.hook = hook;
            abi::emit(
                events::HookSet::TOPIC,
                events::HookSet {
                    hook: hook.to_bytes(),
                },
            );
        }

        /// Set the ISM override. Owner only.
        pub fn set_ism(&mut self, ism: ContractId) {
            self.only_owner();
            self.ism = ism;
            abi::emit(
                events::IsmSet::TOPIC,
                events::IsmSet {
                    ism: ism.to_bytes(),
                },
            );
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            assert!(
                new_owner != [0u8; 32],
                "WarpNative: new owner cannot be zero"
            );
            let previous_owner = self.owner.expect("WarpNative: no owner set");
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
            let owner = self.owner.expect("WarpNative: no owner set");
            assert!(
                caller::effective_caller() == owner,
                "WarpNative: caller is not the owner"
            );
        }
    }
}
