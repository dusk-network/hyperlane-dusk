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
//!   to the recipient contract's `handle` function. Unlike Solidity's,
//!   it takes no calls from contracts: each delivery is a transaction.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::module_name_repetitions)]

/// Hyperlane Mailbox contract.
#[dusk_forge::contract(events = [
    events::DefaultHookSet,
    events::DefaultIsmSet,
    events::Dispatch,
    events::DispatchFeeFunded,
    events::DispatchFeePaid,
    events::DispatchFeeWithdrawn,
    events::DispatchId,
    events::Initialized,
    events::OwnershipRenounced,
    events::OwnershipTransferred,
    events::Process,
    events::ProcessId,
    events::RequiredHookSet,
])]
mod mailbox {
    extern crate alloc;

    use alloc::collections::btree_map::Entry;
    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::string::String;
    use alloc::vec::Vec;

    use dusk_bytes::Serializable;
    use dusk_core::abi::{self, ContractError, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::signatures::bls::PublicKey as AccountPublicKey;
    use dusk_core::transfer::{
        ContractToAccount, ContractToContract, ReceiveFromContract, TRANSFER_CONTRACT,
    };

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::storage::{BlockHeights, ChunkedLog};
    use hyperlane_dusk_types::{MessageId, H256, VERSION};

    // =====================================================================
    // Constants
    // =====================================================================

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Gas allowed for a recipient's `interchain_security_module` getter.
    const RECIPIENT_ISM_LOOKUP_GAS: u64 = 1_000_000;

    /// Size of a wasm32 memory page.
    const WASM_PAGE_BYTES: u64 = 64 * 1024;

    /// Memory `dispatch` and credit for a new payer never grow this contract
    /// past: 3 GiB of the 4 GiB a wasm32 contract can address. Memory never
    /// shrinks, so the last GiB stays free for `process`, which keeps
    /// working, and for withdrawals and top-ups, which add no entry.
    const DISPATCH_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;

    /// The most B-tree nodes adding a key to a map allocates: one per level
    /// of the tree when every level splits, and a new root. Every node but
    /// the root holds at least 5 keys, so a map of n keys has at most
    /// log6((n + 1) / 2) + 1 levels, 12 for any n below 2^32, and an
    /// insertion allocates at most 13 nodes.
    const MAX_NODES_PER_INSERT: u64 = 13;

    /// The most memory a new payer's credit can grow this contract by. A
    /// node of `fee_credits` takes under 600 bytes, so dlmalloc, which grows
    /// memory in whole pages when no free block fits, grows it by at most a
    /// page for each.
    const NEW_CREDIT_MEMORY_BOUND: u64 = MAX_NODES_PER_INSERT * WASM_PAGE_BYTES;

    /// Bytes dlmalloc adds to a request before growing memory for it: the
    /// chunk header and alignment, and the segment footer it keeps at the top
    /// (at most 63 bytes on wasm32 in dlmalloc 0.2).
    const ALLOCATOR_OVERHEAD: u64 = 64;

    /// The hook function the transfer contract calls with a hook's payment.
    const RECEIVE_PAYMENT: &str = "receive_payment";

    /// The least `fund_dispatch` takes for a payer with no credit: 0.01 DUSK.
    /// Each payer with credit is an entry in this contract's memory for as
    /// long as the credit lasts, so a new one costs at least this much.
    /// Credit is withdrawable, but a withdrawal decodes a key for about 57M
    /// gas, 0.057 DUSK at 1 LUX per gas, so dust credit isn't worth taking
    /// back.
    const MIN_NEW_PAYER_CREDIT: u64 = 10_000_000;

    // =====================================================================
    // State
    // =====================================================================

    /// Mailbox contract state.
    pub struct Mailbox {
        /// The domain ID of this chain.
        local_domain: u32,
        /// Monotonically increasing nonce for outbound messages.
        nonce: u32,
        /// Prevents hook callbacks from recursively entering dispatch.
        dispatching: bool,
        /// The ID of the most recently dispatched message.
        latest_dispatched_id: MessageId,
        /// The default Interchain Security Module.
        default_ism: ContractId,
        /// The default post-dispatch hook.
        default_hook: ContractId,
        /// The required post-dispatch hook (called on ALL dispatches).
        required_hook: ContractId,
        /// The first 16 bytes of every delivered message's ID (see
        /// [`delivery_key`]).
        delivered: BTreeSet<DeliveryKey>,
        /// Owner identity (Moonlight account hash or contract ID).
        owner: Option<H256>,
        /// Native DUSK fee credits keyed by encoded message sender identity.
        fee_credits: BTreeMap<H256, u64>,
        /// Message IDs indexed by nonce, for agents. The message itself is
        /// only in the `Dispatch` event: storing every message would let
        /// anyone fill this contract's memory cheaply.
        dispatched_ids: ChunkedLog<MessageId>,
        /// The block height of each dispatch, indexed by nonce.
        dispatched_heights: BlockHeights,
    }

    /// What `delivered` stores of a message ID: its first 16 bytes.
    type DeliveryKey = [u8; 16];

    /// The key a delivered message is recorded under.
    ///
    /// The same message always has the same ID, so a replay always finds its
    /// key. A different message is refused as already delivered only if its
    /// ID starts with the same 16 bytes as a delivered one's: by accident with
    /// probability about n^2 / 2^129 over n deliveries. Doing that to someone
    /// else's message takes a second preimage for 128 bits of keccak256; two
    /// messages of one's own can collide after about 2^64 hashes, which only
    /// blocks one's own message. See docs/storage.md.
    fn delivery_key(id: &MessageId) -> DeliveryKey {
        let mut key = [0u8; 16];
        key.copy_from_slice(&id[..16]);
        key
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
                dispatching: false,
                latest_dispatched_id: [0u8; 32],
                default_ism: ZERO_CONTRACT,
                default_hook: ZERO_CONTRACT,
                required_hook: ZERO_CONTRACT,
                delivered: BTreeSet::new(),
                owner: None,
                fee_credits: BTreeMap::new(),
                dispatched_ids: ChunkedLog::new(),
                dispatched_heights: BlockHeights::new(),
            }
        }

        /// Initialize the Mailbox with domain and configuration.
        ///
        /// Must be called once during deployment. Panics if already
        /// initialized. The configuration sentinel remains set after ownership
        /// is renounced, so renunciation cannot reopen initialization even if a
        /// future runtime permits calling `init` after deployment.
        pub fn init(
            &mut self,
            local_domain: u32,
            owner: H256,
            default_ism: ContractId,
            default_hook: ContractId,
            required_hook: ContractId,
        ) {
            assert!(
                self.owner.is_none() && self.default_ism == ZERO_CONTRACT,
                "Mailbox: already initialized"
            );
            assert!(owner != [0u8; 32], "Mailbox: owner cannot be zero");
            assert!(
                default_ism != ZERO_CONTRACT,
                "Mailbox: default ISM cannot be zero"
            );
            assert!(
                default_hook != ZERO_CONTRACT,
                "Mailbox: default hook cannot be zero"
            );
            assert!(
                required_hook != ZERO_CONTRACT,
                "Mailbox: required hook cannot be zero"
            );
            assert!(
                default_hook != required_hook,
                "Mailbox: default and required hooks must differ"
            );
            self.local_domain = local_domain;
            self.owner = Some(owner);
            self.default_ism = default_ism;
            self.default_hook = default_hook;
            self.required_hook = required_hook;
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_MAILBOX,
                    owner,
                    mailbox: ZERO_CONTRACT.to_bytes(),
                    local_domain,
                },
            );
            abi::emit(
                events::DefaultIsmSet::TOPIC,
                events::DefaultIsmSet {
                    module: default_ism.to_bytes(),
                },
            );
            abi::emit(
                events::DefaultHookSet::TOPIC,
                events::DefaultHookSet {
                    hook: default_hook.to_bytes(),
                },
            );
            abi::emit(
                events::RequiredHookSet::TOPIC,
                events::RequiredHookSet {
                    hook: required_hook.to_bytes(),
                },
            );
        }

        // =================================================================
        // Core: dispatch
        // =================================================================

        /// Deposit native DUSK into the dispatch-fee credit for `payer`.
        ///
        /// Anyone may fund an identity, but only dispatches whose encoded
        /// sender equals `payer` can consume the credit.
        ///
        /// A payer with no credit takes at least [`MIN_NEW_PAYER_CREDIT`],
        /// 0.01 DUSK; one with credit takes any amount. Credit that reaches
        /// zero, spent by dispatches or withdrawn, leaves no entry behind, so
        /// its payer needs the minimum again. A payer with no credit is also
        /// refused once its entry could take the memory past 3 GiB, where
        /// dispatch closes too (see [`Self::add_credit`]).
        pub fn fund_dispatch(&mut self, payer: H256, amount: u64) {
            assert!(payer != [0u8; 32], "Mailbox: payer cannot be zero");
            assert!(amount > 0, "Mailbox: funding amount is zero");
            assert!(
                amount >= MIN_NEW_PAYER_CREDIT || self.fee_credits.contains_key(&payer),
                "Mailbox: a payer with no credit takes at least 0.01 DUSK"
            );
            let _: () = abi::call(TRANSFER_CONTRACT, "deposit", &amount)
                .expect("Mailbox: fee deposit failed");
            self.add_credit(payer, amount);
            abi::emit(
                events::DispatchFeeFunded::TOPIC,
                events::DispatchFeeFunded { payer, amount },
            );
        }

        /// Withdraw native DUSK from the caller's dispatch-fee credit.
        ///
        /// The effective caller is always the payer: a direct Moonlight call
        /// withdraws that account's credit, while an inter-contract call
        /// withdraws the calling contract's credit. The payer may direct the
        /// funds to any explicit Moonlight account.
        ///
        /// `recipient` is decoded to its canonical form first, and the
        /// payout and the event use that key: the one its owner signs with,
        /// whatever limbs the caller sends (see [`canonical_recipient`]).
        /// Decoding costs about 57M gas.
        pub fn withdraw_dispatch_credit(&mut self, recipient: AccountPublicKey, amount: u64) {
            assert!(amount > 0, "Mailbox: withdrawal amount is zero");
            let recipient = canonical_recipient(&recipient);

            let payer = Self::resolve_sender();
            let credit = self.fee_credits.get(&payer).copied().unwrap_or(0);
            assert!(credit >= amount, "Mailbox: insufficient fee credit");

            let remaining = credit - amount;
            if remaining == 0 {
                self.fee_credits.remove(&payer);
            } else {
                self.fee_credits.insert(payer, remaining);
            }

            let recipient_hash = message::keccak256(&recipient.to_bytes());
            let transfer = ContractToAccount {
                account: recipient,
                value: amount,
            };
            let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_account", &transfer)
                .expect("Mailbox: dispatch credit withdrawal failed");

            abi::emit(
                events::DispatchFeeWithdrawn::TOPIC,
                events::DispatchFeeWithdrawn {
                    payer,
                    recipient: recipient_hash,
                    amount,
                },
            );
        }

        /// Accept native DUSK sent by a contract and credit that exact contract.
        ///
        /// The transfer contract supplies the declared source; requiring an
        /// authentic nested transfer callback prevents a root caller from
        /// forging it. Warp routes use this callback to contribute each user's
        /// dispatch quote immediately before dispatch, which spends it, so
        /// it takes any amount. A contract with no credit is refused once its
        /// entry could take the memory past 3 GiB, as in `fund_dispatch`;
        /// its only use is a dispatch, which closes there.
        pub fn receive_dispatch_funding(&mut self, transfer: ReceiveFromContract) {
            assert!(
                caller::authentic_transfer_callback(transfer.contract, transfer.contract),
                "Mailbox: unauthenticated dispatch funding"
            );
            assert!(
                transfer.contract != TRANSFER_CONTRACT,
                "Mailbox: invalid dispatch funding source"
            );
            assert!(
                transfer.data.is_empty(),
                "Mailbox: unexpected dispatch funding data"
            );
            assert!(transfer.value > 0, "Mailbox: funding amount is zero");
            let payer = transfer.contract.to_bytes();
            self.add_credit(payer, transfer.value);
            abi::emit(
                events::DispatchFeeFunded::TOPIC,
                events::DispatchFeeFunded {
                    payer,
                    amount: transfer.value,
                },
            );
        }

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
            assert!(
                hook != self.required_hook,
                "Mailbox: selected hook cannot equal required hook"
            );
            assert!(!self.dispatching, "Mailbox: dispatch reentrancy");
            self.dispatching = true;

            // Determine the sender: the contract that called us.
            let sender = Self::resolve_sender();

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

            // Quote and reserve value before mutating dispatch state. Any
            // later failed hook/payment interaction reverts the whole call.
            let required_fee = Self::quote_hook(
                self.required_hook,
                metadata.clone(),
                encoded.clone(),
                "Mailbox: required hook quote failed",
            );
            let hook_fee = Self::quote_hook(
                hook,
                metadata.clone(),
                encoded.clone(),
                "Mailbox: hook quote failed",
            );
            let total_fee = required_fee
                .checked_add(hook_fee)
                .expect("Mailbox: fee overflow");
            if total_fee > 0 {
                let credit = self.fee_credits.get(&sender).copied().unwrap_or(0);
                assert!(credit >= total_fee, "Mailbox: insufficient fee credit");
                let remaining = credit - total_fee;
                if remaining == 0 {
                    self.fee_credits.remove(&sender);
                } else {
                    self.fee_credits.insert(sender, remaining);
                }
            }

            // Refuse the message unless everything the rest of this call
            // can allocate fits under the limit, so dispatch never takes the
            // memory reserved for `process`.
            let block_height = abi::block_height();
            let growth = self.dispatch_memory_growth(block_height, &metadata, &encoded);
            assert!(
                Self::memory_bytes_used() + growth <= DISPATCH_MEMORY_LIMIT,
                "Mailbox: storage full, dispatch is closed"
            );

            // Effects
            self.latest_dispatched_id = id;
            self.dispatched_ids.push(id);
            self.dispatched_heights.push(block_height);
            self.nonce = self.nonce.checked_add(1).expect("Mailbox: nonce overflow");

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
            if total_fee > 0 {
                abi::emit(
                    events::DispatchFeePaid::TOPIC,
                    events::DispatchFeePaid {
                        payer: sender,
                        message_id: id,
                        amount: total_fee,
                    },
                );
            }

            // Interactions: call hooks
            // Required hook first
            Self::call_hook_with_payment(
                self.required_hook,
                metadata.clone(),
                encoded.clone(),
                required_fee,
                "Mailbox: required hook post_dispatch failed",
                "Mailbox: required hook payment failed",
            );

            // Default/custom hook
            Self::call_hook_with_payment(
                hook,
                metadata,
                encoded,
                hook_fee,
                "Mailbox: hook post_dispatch failed",
                "Mailbox: hook payment failed",
            );

            self.dispatching = false;
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
        ///
        /// Only a transaction's own call or a query can deliver: a contract
        /// can't call this, so every delivery costs a transaction and adds
        /// at most one entry to the delivered set. The relayer's simulation
        /// is a query, so it still runs.
        pub fn process(&mut self, metadata: Vec<u8>, encoded_message: Vec<u8>) {
            // The VM tests also build a copy without this check, with
            // `--cfg hyperlane_dusk_ungated_test` (`make mailbox-ungated`),
            // to show that the delivery reservation below refuses a replay
            // on its own (HIGH-6). That copy is never deployed.
            #[cfg(not(hyperlane_dusk_ungated_test))]
            assert!(
                caller::is_transaction_or_query(),
                "Mailbox: process must be a transaction's own call"
            );

            // Decode and validate the message.
            let msg = message::decode(&encoded_message).expect("Mailbox: invalid message encoding");

            assert!(msg.version == VERSION, "Mailbox: bad version");
            assert!(
                msg.destination == self.local_domain,
                "Mailbox: unexpected destination"
            );

            // Mark as delivered BEFORE external calls (checks-effects-interactions).
            let id = message::id(&encoded_message);
            assert!(
                self.delivered.insert(delivery_key(&id)),
                "Mailbox: already delivered"
            );

            // Emit events. Agents index deliveries from these.
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

            // Recipient ISM resolution is itself an external call. A call
            // back into `process` is refused at the check above; delivery is
            // reserved before this lookup, verification and handling as
            // well, and failed calls roll it all back.
            let recipient_id = ContractId::from_bytes(msg.recipient);
            let ism = self.resolve_recipient_ism(recipient_id);

            // Verify via ISM
            let verified: bool = abi::call(ism, "verify", &(metadata, encoded_message))
                .expect("Mailbox: ISM call failed");
            assert!(verified, "Mailbox: ISM verification failed");

            // Deliver to recipient
            let _: () = abi::call(recipient_id, "handle", &(msg.origin, msg.sender, msg.body))
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
        ///
        /// Only the first 16 bytes of each delivered ID are stored, so this
        /// is also true for a message whose ID shares them with a delivered
        /// one (see [`delivery_key`]).
        pub fn delivered(&self, id: MessageId) -> bool {
            self.delivered.contains(&delivery_key(&id))
        }

        /// Returns the ID of the message dispatched at `nonce`. The message
        /// itself is in that block's `Dispatch` event.
        ///
        /// Panics if the nonce is out of range.
        pub fn dispatched_id(&self, nonce: u32) -> MessageId {
            self.dispatched_ids
                .get(nonce)
                .expect("Mailbox: nonce out of range")
        }

        /// Returns the size of this contract's memory in bytes. It only
        /// grows; `dispatch` and credit for a new payer never take it past
        /// [`DISPATCH_MEMORY_LIMIT`].
        #[allow(clippy::unused_self)] // Contract queries are instance methods in the Dusk ABI.
        pub fn memory_bytes(&self) -> u64 {
            Self::memory_bytes_used()
        }

        /// Returns the block height at which the message at `nonce` was dispatched.
        ///
        /// Panics if the nonce is out of range.
        pub fn dispatched_block_height(&self, nonce: u32) -> u64 {
            self.dispatched_heights
                .get(nonce)
                .expect("Mailbox: nonce out of range")
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

        /// Returns the native DUSK dispatch-fee credit for `payer`.
        pub fn fee_credit(&self, payer: H256) -> u64 {
            self.fee_credits.get(&payer).copied().unwrap_or(0)
        }

        /// Returns the owner identity.
        pub fn owner(&self) -> Option<H256> {
            self.owner
        }

        /// Returns the deployment compatibility version expected by tooling.
        ///
        /// Version 6 takes `process` only as a transaction's own call or a
        /// query, funds a payer with no credit from 0.01 DUSK up, and adds
        /// no payer once its entry could take the memory past 3 GiB.
        /// Version 5 keeps only the first 16 bytes of delivered message IDs,
        /// drops the processed-message log (`processed_count`,
        /// `processed_at_index`, `processed_block_height_at_index`) and
        /// `delivered_at`, stores dispatched IDs in chunks and their block
        /// heights once per block, pays withdrawals to the decoded recipient
        /// key, and adds `quote_dispatch_for_sender`. Version 4 stored
        /// message IDs instead of messages and bounded the recipient ISM
        /// lookup.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            6
        }

        /// Compute a quote for dispatching a message from the caller.
        ///
        /// Returns the total fee required (required_hook quote + hook quote).
        /// The sender is resolved as `dispatch` resolves it, so this works
        /// only inside a transaction; a read-only query has no caller. Use
        /// [`Self::quote_dispatch_for_sender`] there.
        pub fn quote_dispatch(
            &self,
            destination: u32,
            recipient: H256,
            body: Vec<u8>,
            metadata: Vec<u8>,
            hook: ContractId,
        ) -> u64 {
            self.quote_dispatch_for_sender(
                Self::resolve_sender(),
                destination,
                recipient,
                body,
                metadata,
                hook,
            )
        }

        /// Compute a dispatch quote for the immediate calling contract.
        ///
        /// Unlike [`Self::quote_dispatch`], this is safe to call through a
        /// top-level read-only route query: the route itself is the immediate
        /// caller even when no Moonlight transaction origin exists.
        pub fn quote_dispatch_for_contract(
            &self,
            destination: u32,
            recipient: H256,
            body: Vec<u8>,
            metadata: Vec<u8>,
            hook: ContractId,
        ) -> u64 {
            let contract = abi::caller().expect("Mailbox: contract quote caller unavailable");
            assert!(
                contract != TRANSFER_CONTRACT,
                "Mailbox: contract quote requires contract caller"
            );
            self.quote_dispatch_for_sender(
                contract.to_bytes(),
                destination,
                recipient,
                body,
                metadata,
                hook,
            )
        }

        /// Compute a dispatch quote for an explicit `sender`.
        ///
        /// Returns what [`Self::dispatch`] would charge `sender` for this
        /// message right now: the required hook's quote plus the selected
        /// hook's (zero selects the default hook). A read-only query can call
        /// it, since it doesn't resolve a caller. Quotes depend on the
        /// current nonce and hook configuration, which can change before a
        /// dispatch.
        pub fn quote_dispatch_for_sender(
            &self,
            sender: H256,
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
            assert!(
                hook != self.required_hook,
                "Mailbox: selected hook cannot equal required hook"
            );

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

            let hook_fee: u64 = abi::call(hook, "quote_dispatch", &(metadata, encoded))
                .expect("Mailbox: hook quote failed");

            required_fee
                .checked_add(hook_fee)
                .expect("Mailbox: fee overflow")
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
            assert!(
                hook != self.required_hook,
                "Mailbox: default and required hooks must differ"
            );
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
            assert!(
                hook != self.default_hook,
                "Mailbox: default and required hooks must differ"
            );
            self.required_hook = hook;
            abi::emit(
                events::RequiredHookSet::TOPIC,
                events::RequiredHookSet {
                    hook: hook.to_bytes(),
                },
            );
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            assert!(new_owner != [0u8; 32], "Mailbox: new owner cannot be zero");
            let previous_owner = self.owner.expect("Mailbox: no owner set");
            self.owner = Some(new_owner);
            abi::emit(
                events::OwnershipTransferred::TOPIC,
                events::OwnershipTransferred {
                    previous_owner,
                    new_owner,
                },
            );
        }

        /// Renounce ownership. Owner only.
        pub fn renounce_ownership(&mut self) {
            self.only_owner();
            let previous_owner = self.owner.expect("Mailbox: no owner set");
            self.owner = None;
            abi::emit(
                events::OwnershipRenounced::TOPIC,
                events::OwnershipRenounced { previous_owner },
            );
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
        fn resolve_sender() -> H256 {
            caller::effective_caller()
        }

        /// Quote one post-dispatch hook.
        fn quote_hook(
            hook: ContractId,
            metadata: Vec<u8>,
            encoded_message: Vec<u8>,
            error: &str,
        ) -> u64 {
            abi::call(hook, "quote_dispatch", &(metadata, encoded_message)).expect(error)
        }

        /// Invoke a hook, then transfer its exact quoted DUSK payment through
        /// the transfer contract. Value-bearing hooks authenticate and record
        /// the resulting `receive_payment` callback.
        fn call_hook_with_payment(
            hook: ContractId,
            metadata: Vec<u8>,
            encoded_message: Vec<u8>,
            fee: u64,
            post_dispatch_error: &str,
            payment_error: &str,
        ) {
            let _: () = abi::call(hook, "post_dispatch", &(metadata, encoded_message))
                .expect(post_dispatch_error);
            if fee == 0 {
                return;
            }

            let transfer = ContractToContract {
                contract: hook,
                value: fee,
                fn_name: String::from(RECEIVE_PAYMENT),
                data: Vec::new(),
            };
            let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_contract", &transfer)
                .expect(payment_error);
        }

        /// Current size of the contract's linear memory.
        fn memory_bytes_used() -> u64 {
            core::arch::wasm32::memory_size(0) as u64 * WASM_PAGE_BYTES
        }

        /// Add `amount` to `payer`'s dispatch credit. A top-up changes its
        /// entry in place. A payer with no credit is a new entry in this
        /// contract's memory, refused unless what it can allocate fits under
        /// [`DISPATCH_MEMORY_LIMIT`], checked before it is allocated, so the
        /// last GiB stays for deliveries. Credit only pays for dispatches,
        /// which close at the same limit.
        fn add_credit(&mut self, payer: H256, amount: u64) {
            match self.fee_credits.entry(payer) {
                Entry::Occupied(mut credit) => {
                    let total = credit
                        .get()
                        .checked_add(amount)
                        .expect("Mailbox: fee credit overflow");
                    credit.insert(total);
                }
                Entry::Vacant(credit) => {
                    assert!(
                        Self::memory_bytes_used() + NEW_CREDIT_MEMORY_BOUND
                            <= DISPATCH_MEMORY_LIMIT,
                        "Mailbox: storage full, dispatch is closing to new payers"
                    );
                    credit.insert(amount);
                }
            }
        }

        /// The most the rest of `dispatch` can grow this contract's memory,
        /// once the message is encoded and quoted: the log chunks that store
        /// it may start, and the `Dispatch` event and the hook calls take
        /// copies of the message and metadata.
        fn dispatch_memory_growth(
            &self,
            block_height: u64,
            metadata: &[u8],
            encoded: &[u8],
        ) -> u64 {
            let [ids_chunk, ids_table] = self.dispatched_ids.push_allocations();
            let [heights_chunk, heights_table] =
                self.dispatched_heights.push_allocations(block_height);
            [
                ids_chunk,
                ids_table,
                heights_chunk,
                heights_table,
                // The `Dispatch` event's copy of the message.
                encoded.len(),
                // The required hook's copies; the selected hook gets the
                // originals.
                metadata.len(),
                encoded.len(),
                // The function name of a payment to each hook.
                RECEIVE_PAYMENT.len(),
                RECEIVE_PAYMENT.len(),
            ]
            .into_iter()
            .map(memory_growth)
            .sum()
        }

        /// Resolve the ISM for a recipient.
        ///
        /// The lookup runs with a fixed gas allowance. A failed inter-contract
        /// call is charged its whole allowance, so an unbounded lookup on a
        /// recipient without `interchain_security_module` would burn most of
        /// the transaction's gas. A recipient that doesn't export the getter,
        /// or whose getter panics, gets the default ISM, as in Solidity. One
        /// that runs out of gas does not: falling back then would let the
        /// submitter's gas limit choose the ISM.
        fn resolve_recipient_ism(&self, recipient: ContractId) -> ContractId {
            let result: Result<ContractId, ContractError> = abi::call_with_limit(
                recipient,
                "interchain_security_module",
                &(),
                RECIPIENT_ISM_LOOKUP_GAS,
            );
            match result {
                Ok(ism) if ism != ZERO_CONTRACT => ism,
                Err(ContractError::OutOfGas) => {
                    panic!("Mailbox: recipient ISM lookup ran out of gas")
                }
                _ => self.default_ism,
            }
        }

        /// Panics if the caller is not the owner.
        fn only_owner(&self) {
            let owner = self.owner.expect("Mailbox: no owner set");
            assert!(
                caller::effective_caller() == owner,
                "Mailbox: caller is not the owner"
            );
        }
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

    /// Decode a withdrawal recipient to the key its owner signs with, as the
    /// warp routes decode a sponsored claim's key.
    ///
    /// The archive check accepts any limbs in a public key. Its compressed
    /// encoding (`to_bytes`), which the event's recipient hash covers,
    /// depends only on x reduced mod p, the infinity flag and the sign of y,
    /// but the transfer contract pays the raw limbs (`to_raw_bytes`). A twin
    /// of the recipient's key would be debited and named in the event, and
    /// paid to an account nobody can spend from. Decoding the compressed
    /// bytes gives the one canonical key with that encoding.
    ///
    /// # Panics
    ///
    /// Panics if the encoding isn't a valid public key, including the
    /// identity, which nobody can sign for.
    fn canonical_recipient(recipient: &AccountPublicKey) -> AccountPublicKey {
        AccountPublicKey::from_bytes(&recipient.to_bytes())
            .ok()
            // `from_bytes` checks that the point is on the curve and in G2,
            // so `is_valid` would only add the identity check, at the cost
            // of a second subgroup check. The default key is the identity.
            .filter(|pk| *pk != AccountPublicKey::default())
            .expect("Mailbox: invalid withdrawal recipient")
    }
}
