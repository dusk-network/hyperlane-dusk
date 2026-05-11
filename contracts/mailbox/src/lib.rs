// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane Mailbox contract for Dusk.

//! Central hub for dispatching and processing cross-chain messages.
//!
//! This contract is the Dusk equivalent of `Mailbox.sol` in the Hyperlane
//! Solidity contracts. It provides two core operations:
//!
//! - **dispatch**: Send a message to a destination chain. The message is
//!   formatted, an ID is emitted, and post-dispatch hooks are called.
//!
//! - **process**: Deliver a message from a remote chain. The message is
//!   verified by an Interchain Security Module (ISM) and then delivered
//!   to the recipient contract's `handle` function.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::module_name_repetitions)]

/// Hyperlane Mailbox contract.
#[dusk_forge::contract]
mod mailbox {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;

    use dusk_bytes::Serializable;
    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::transfer::TRANSFER_CONTRACT;

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::{DeliveryRecord, H256, MessageId, VERSION};

    // =====================================================================
    // Constants
    // =====================================================================

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    // =====================================================================
    // State
    // =====================================================================

    /// Mailbox contract state.
    pub struct Mailbox {
        /// The domain ID of this chain.
        local_domain: u32,
        /// Monotonically increasing nonce for outbound messages.
        nonce: u32,
        /// The ID of the most recently dispatched message.
        latest_dispatched_id: MessageId,
        /// The default Interchain Security Module.
        default_ism: ContractId,
        /// The default post-dispatch hook.
        default_hook: ContractId,
        /// The required post-dispatch hook (called on ALL dispatches).
        required_hook: ContractId,
        /// Map of message ID -> delivery record for processed messages.
        delivered: BTreeMap<MessageId, DeliveryRecord>,
        /// Contract owner (can update ISM/hooks).
        owner: Option<ContractId>,
        /// Encoded messages indexed by nonce (for off-chain agent indexing).
        dispatched_messages: Vec<Vec<u8>>,
        /// Dispatch block heights indexed by nonce (for off-chain agent indexing).
        dispatched_block_heights: Vec<u64>,
        /// Message IDs in delivery order (for off-chain agent indexing).
        processed_ids: Vec<MessageId>,
        /// Delivery block heights in delivery order (for off-chain agent indexing).
        processed_block_heights: Vec<u64>,
    }

    impl Mailbox {
        // =================================================================
        // Initialization
        // =================================================================

        /// Creates a new empty Mailbox state.
        pub const fn new() -> Self {
            Self {
                local_domain: 0,
                nonce: 0,
                latest_dispatched_id: [0u8; 32],
                default_ism: ZERO_CONTRACT,
                default_hook: ZERO_CONTRACT,
                required_hook: ZERO_CONTRACT,
                delivered: BTreeMap::new(),
                owner: None,
                dispatched_messages: Vec::new(),
                dispatched_block_heights: Vec::new(),
                processed_ids: Vec::new(),
                processed_block_heights: Vec::new(),
            }
        }

        /// Initialize the Mailbox with domain and configuration.
        ///
        /// Must be called once after deployment. Panics if already
        /// initialized (owner is set).
        pub fn init(
            &mut self,
            local_domain: u32,
            owner: ContractId,
            default_ism: ContractId,
            default_hook: ContractId,
            required_hook: ContractId,
        ) {
            assert!(self.owner.is_none(), "Mailbox: already initialized");
            self.local_domain = local_domain;
            self.owner = Some(owner);
            self.default_ism = default_ism;
            self.default_hook = default_hook;
            self.required_hook = required_hook;
        }

        // =================================================================
        // Core: dispatch
        // =================================================================

        /// Dispatch a message to a destination chain.
        ///
        /// Builds the Hyperlane message, increments the nonce, emits
        /// `Dispatch` and `DispatchId` events, and calls the required
        /// hook and default hook's `post_dispatch` methods.
        ///
        /// Returns the message ID.
        pub fn dispatch(
            &mut self,
            destination: u32,
            recipient: H256,
            body: Vec<u8>,
            metadata: Vec<u8>,
            hook: ContractId,
        ) -> MessageId {
            let hook = if hook == ZERO_CONTRACT {
                self.default_hook
            } else {
                hook
            };

            // Determine the sender: the contract that called us.
            let sender = self.resolve_sender();

            // Build the packed message.
            let encoded = message::encode(
                VERSION,
                self.nonce,
                self.local_domain,
                sender,
                destination,
                recipient,
                &body,
            );
            let id = message::id(&encoded);

            // Effects
            self.latest_dispatched_id = id;
            self.dispatched_messages.push(encoded.clone());
            self.dispatched_block_heights.push(abi::block_height());
            self.nonce += 1;

            // Emit events
            abi::emit(
                events::Dispatch::TOPIC,
                events::Dispatch {
                    sender,
                    destination,
                    recipient,
                    message: encoded.clone(),
                },
            );
            abi::emit(
                events::DispatchId::TOPIC,
                events::DispatchId { message_id: id },
            );

            // Interactions: call hooks
            // Required hook first
            let _: () = abi::call(
                self.required_hook,
                "post_dispatch",
                &(metadata.clone(), encoded.clone()),
            )
            .expect("Mailbox: required hook post_dispatch failed");

            // Default/custom hook
            let _: () = abi::call(hook, "post_dispatch", &(metadata, encoded))
                .expect("Mailbox: hook post_dispatch failed");

            id
        }

        /// Dispatch with default hook and empty metadata.
        pub fn dispatch_default(
            &mut self,
            destination: u32,
            recipient: H256,
            body: Vec<u8>,
        ) -> MessageId {
            self.dispatch(destination, recipient, body, Vec::new(), ZERO_CONTRACT)
        }

        // =================================================================
        // Core: process
        // =================================================================

        /// Process (deliver) a message from a remote chain.
        ///
        /// Verifies the message via the recipient's ISM (or the default
        /// ISM) and then calls `handle` on the recipient contract.
        pub fn process(&mut self, metadata: Vec<u8>, encoded_message: Vec<u8>) {
            // Decode and validate the message.
            let msg = message::decode(&encoded_message)
                .expect("Mailbox: invalid message encoding");

            assert!(
                msg.version == VERSION,
                "Mailbox: bad version"
            );
            assert!(
                msg.destination == self.local_domain,
                "Mailbox: unexpected destination"
            );

            let id = message::id(&encoded_message);
            assert!(
                !self.delivered.contains_key(&id),
                "Mailbox: already delivered"
            );

            // Resolve the recipient's ISM, falling back to default.
            let recipient_id = ContractId::from_bytes(msg.recipient);
            let ism = self.resolve_recipient_ism(recipient_id);

            // Track delivery order for off-chain indexing.
            let block_height = abi::block_height();
            self.processed_ids.push(id);
            self.processed_block_heights.push(block_height);

            // Mark as delivered BEFORE external calls (checks-effects-interactions).
            self.delivered.insert(
                id,
                DeliveryRecord {
                    block_height,
                },
            );

            // Emit events
            abi::emit(
                events::Process::TOPIC,
                events::Process {
                    origin: msg.origin,
                    sender: msg.sender,
                    recipient: msg.recipient,
                },
            );
            abi::emit(
                events::ProcessId::TOPIC,
                events::ProcessId { message_id: id },
            );

            // Verify via ISM
            let verified: bool = abi::call(
                ism,
                "verify",
                &(metadata, encoded_message),
            )
            .expect("Mailbox: ISM call failed");
            assert!(verified, "Mailbox: ISM verification failed");

            // Deliver to recipient
            let _: () = abi::call(
                recipient_id,
                "handle",
                &(msg.origin, msg.sender, msg.body),
            )
            .expect("Mailbox: recipient handle failed");
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the local domain ID.
        pub fn local_domain(&self) -> u32 {
            self.local_domain
        }

        /// Returns the current nonce.
        pub fn nonce(&self) -> u32 {
            self.nonce
        }

        /// Returns the ID of the most recently dispatched message.
        pub fn latest_dispatched_id(&self) -> MessageId {
            self.latest_dispatched_id
        }

        /// Returns whether a message has been delivered.
        pub fn delivered(&self, id: MessageId) -> bool {
            self.delivered.contains_key(&id)
        }

        /// Returns the block height at which a message was delivered.
        ///
        /// Returns 0 if the message hasn't been delivered.
        pub fn delivered_at(&self, id: MessageId) -> u64 {
            self.delivered
                .get(&id)
                .map_or(0, |d| d.block_height)
        }

        /// Returns the encoded dispatched message at the given nonce.
        ///
        /// Panics if the nonce is out of range.
        pub fn dispatched_message(&self, nonce: u32) -> Vec<u8> {
            self.dispatched_messages[nonce as usize].clone()
        }

        /// Returns the block height at which the message at `nonce` was dispatched.
        ///
        /// Panics if the nonce is out of range.
        pub fn dispatched_block_height(&self, nonce: u32) -> u64 {
            self.dispatched_block_heights[nonce as usize]
        }

        /// Returns the message ID of the N-th processed (delivered) message.
        ///
        /// Panics if the index is out of range.
        pub fn processed_at_index(&self, index: u32) -> MessageId {
            self.processed_ids[index as usize]
        }

        /// Returns the block height at which the N-th message was processed (delivered).
        ///
        /// Panics if the index is out of range.
        pub fn processed_block_height_at_index(&self, index: u32) -> u64 {
            self.processed_block_heights[index as usize]
        }

        /// Returns the number of messages that have been processed (delivered).
        pub fn processed_count(&self) -> u32 {
            self.processed_ids.len() as u32
        }

        /// Returns the default ISM contract ID.
        pub fn default_ism(&self) -> ContractId {
            self.default_ism
        }

        /// Returns the default hook contract ID.
        pub fn default_hook(&self) -> ContractId {
            self.default_hook
        }

        /// Returns the required hook contract ID.
        pub fn required_hook(&self) -> ContractId {
            self.required_hook
        }

        /// Returns the owner contract ID.
        pub fn owner(&self) -> Option<ContractId> {
            self.owner
        }

        /// Compute a quote for dispatching a message.
        ///
        /// Returns the total fee required (required_hook quote + hook quote).
        pub fn quote_dispatch(
            &self,
            destination: u32,
            recipient: H256,
            body: Vec<u8>,
            metadata: Vec<u8>,
            hook: ContractId,
        ) -> u64 {
            let hook = if hook == ZERO_CONTRACT {
                self.default_hook
            } else {
                hook
            };

            let sender = self.resolve_sender();
            let encoded = message::encode(
                VERSION,
                self.nonce,
                self.local_domain,
                sender,
                destination,
                recipient,
                &body,
            );

            let required_fee: u64 = abi::call(
                self.required_hook,
                "quote_dispatch",
                &(metadata.clone(), encoded.clone()),
            )
            .expect("Mailbox: required hook quote failed");

            let hook_fee: u64 = abi::call(
                hook,
                "quote_dispatch",
                &(metadata, encoded),
            )
            .expect("Mailbox: hook quote failed");

            required_fee + hook_fee
        }

        /// Resolve the ISM for a given recipient.
        ///
        /// Tries to call `interchain_security_module()` on the recipient.
        /// Falls back to `default_ism` if the call fails or returns zero.
        pub fn recipient_ism(&self, recipient: ContractId) -> ContractId {
            self.resolve_recipient_ism(recipient)
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Set the default ISM. Owner only.
        pub fn set_default_ism(&mut self, module: ContractId) {
            self.only_owner();
            assert!(module != ZERO_CONTRACT, "Mailbox: ISM cannot be zero");
            self.default_ism = module;
            abi::emit(
                events::DefaultIsmSet::TOPIC,
                events::DefaultIsmSet {
                    module: module.to_bytes(),
                },
            );
        }

        /// Set the default post-dispatch hook. Owner only.
        pub fn set_default_hook(&mut self, hook: ContractId) {
            self.only_owner();
            assert!(hook != ZERO_CONTRACT, "Mailbox: hook cannot be zero");
            self.default_hook = hook;
            abi::emit(
                events::DefaultHookSet::TOPIC,
                events::DefaultHookSet {
                    hook: hook.to_bytes(),
                },
            );
        }

        /// Set the required post-dispatch hook. Owner only.
        pub fn set_required_hook(&mut self, hook: ContractId) {
            self.only_owner();
            assert!(hook != ZERO_CONTRACT, "Mailbox: hook cannot be zero");
            self.required_hook = hook;
            abi::emit(
                events::RequiredHookSet::TOPIC,
                events::RequiredHookSet {
                    hook: hook.to_bytes(),
                },
            );
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: ContractId) {
            self.only_owner();
            self.owner = Some(new_owner);
        }

        /// Renounce ownership. Owner only.
        pub fn renounce_ownership(&mut self) {
            self.only_owner();
            self.owner = None;
        }

        // =================================================================
        // Internal helpers
        // =================================================================

        /// Resolve the sender address.
        ///
        /// - **Inter-contract call**: returns the calling contract's ID.
        /// - **Direct Moonlight TX**: the transfer contract is the immediate
        ///   caller; we look up the BLS public key of the transaction origin
        ///   via `abi::public_sender()` and keccak256-hash it to H256.
        fn resolve_sender(&self) -> H256 {
            match abi::caller() {
                Some(id) if id == TRANSFER_CONTRACT => {
                    // Direct Moonlight transaction — derive sender from
                    // the BLS public key of the account that signed the TX.
                    let pk = abi::public_sender()
                        .expect("Mailbox: shielded transactions not supported");
                    message::keccak256(&pk.to_bytes())
                }
                Some(id) => {
                    // Inter-contract call — sender is the calling contract.
                    id.to_bytes()
                }
                None => {
                    panic!("Mailbox: cannot determine sender")
                }
            }
        }

        /// Resolve the ISM for a recipient.
        fn resolve_recipient_ism(&self, recipient: ContractId) -> ContractId {
            // Try to query the recipient for its ISM.
            let result: Result<ContractId, _> =
                abi::call(recipient, "interchain_security_module", &());

            if let Ok(ism) = result {
                if ism != ZERO_CONTRACT {
                    return ism;
                }
            }

            // Fall back to default ISM.
            self.default_ism
        }

        /// Panics if the caller is not the owner.
        fn only_owner(&self) {
            let caller = abi::caller().expect("Mailbox: cannot determine caller");
            let owner = self.owner.expect("Mailbox: no owner set");
            assert!(caller == owner, "Mailbox: caller is not the owner");
        }
    }
}
