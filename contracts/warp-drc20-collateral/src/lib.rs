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
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane WarpDrc20Collateral contract.
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
mod warp_drc20_collateral {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::vec::Vec;
    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::signatures::bls::PublicKey as AccountPublicKey;
    use dusk_core::transfer::TRANSFER_CONTRACT;

    use dusk_bytes::Serializable;

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::drc20::{self, Account, BalanceOf, TransferCall, TransferFromCall};
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::token_message;
    use hyperlane_dusk_types::{MessageId, H256};

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

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
        /// Owner identity (Moonlight account hash or contract ID).
        owner: Option<H256>,
        /// Enrolled remote routers per domain.
        enrolled_routers: BTreeMap<u32, H256>,
        /// Registry of external accounts: keccak256(pk.to_bytes()) → pk.
        ///
        /// External Dusk accounts use 96-byte BLS public keys that don't
        /// fit in a 32-byte H256. Users must call `register_account` once
        /// so that inbound transfers can resolve to External accounts.
        registered_accounts: BTreeMap<H256, AccountPublicKey>,
        /// Pending transfers for recipients who haven't registered yet.
        ///
        /// The recipient can call `claim_pending` after registering to
        /// receive their wrapped DRC20 tokens.
        pending_transfers: BTreeMap<H256, u64>,
        /// Aggregate pending liability reserved against wrapped-token custody.
        pending_total: u64,
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
                pending_transfers: BTreeMap::new(),
                pending_total: 0,
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the collateral warp route.
        pub fn init(
            &mut self,
            wrapped_token: ContractId,
            mailbox: ContractId,
            owner: H256,
            enrolled_routers: Vec<(u32, H256)>,
        ) {
            assert!(self.owner.is_none(), "WarpCollateral: already initialized");
            assert!(owner != [0u8; 32], "WarpCollateral: owner cannot be zero");
            assert!(
                mailbox != ZERO_CONTRACT,
                "WarpCollateral: mailbox cannot be zero"
            );
            assert!(
                wrapped_token != ZERO_CONTRACT,
                "WarpCollateral: wrapped token cannot be zero"
            );
            self.wrapped_token = wrapped_token;
            self.mailbox = mailbox;
            self.owner = Some(owner);
            for (domain, router) in enrolled_routers {
                assert!(
                    router != [0u8; 32],
                    "WarpCollateral: router cannot be zero"
                );
                self.enrolled_routers.insert(domain, router);
                abi::emit(
                    events::RemoteRouterEnrolled::TOPIC,
                    events::RemoteRouterEnrolled { domain, router },
                );
            }
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_WARP_DRC20_COLLATERAL,
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
        /// can resolve the H256 recipient to an External DRC20 account.
        ///
        /// Reads the sender from `abi::public_sender()` (Moonlight TX).
        /// Stores `keccak256(pk.to_bytes()) → pk`.
        pub fn register_account(&mut self) {
            let pk = abi::public_sender()
                .expect("WarpCollateral: register_account requires Moonlight TX");
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

        /// Claim pending wrapped DRC20 tokens that arrived before the caller
        /// registered.
        ///
        /// The caller must have previously called `register_account`.
        pub fn claim_pending(&mut self) {
            let pk =
                abi::public_sender().expect("WarpCollateral: claim_pending requires Moonlight TX");
            let h = message::keccak256(&pk.to_bytes());
            self.claim_pending_to(h, Account::External(pk));
        }

        /// Claim pending wrapped DRC20 tokens for the calling contract.
        ///
        /// The pending-recipient key is the immediate caller's `ContractId`
        /// bytes. The Moonlight transfer contract is rejected as a root
        /// caller so it cannot be confused with the intended recipient.
        pub fn claim_pending_contract(&mut self) {
            let contract = abi::caller().expect("WarpCollateral: contract caller unavailable");
            assert!(
                contract != TRANSFER_CONTRACT,
                "WarpCollateral: claim_pending_contract requires contract caller"
            );
            self.claim_pending_to(contract.to_bytes(), Account::Contract(contract));
        }

        /// Returns the pending (escrowed) balance for an H256 recipient.
        pub fn pending_balance(&self, h: H256) -> u64 {
            self.pending_transfers.get(&h).copied().unwrap_or(0)
        }

        /// Returns the aggregate wrapped-token liability held in escrow.
        pub fn pending_total(&self) -> u64 {
            self.pending_total
        }

        /// Returns the persisted state layout version expected by deployment tooling.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            1
        }

        // =================================================================
        // Warp Route: transfer_remote (send — lock tokens)
        // =================================================================

        /// Send tokens to a remote chain by locking them in this contract.
        ///
        /// The caller must have approved this contract to spend `amount`
        /// of the wrapped DRC20 token via `approve()`.
        pub fn transfer_remote(
            &mut self,
            destination: u32,
            recipient: H256,
            amount: u64,
        ) -> MessageId {
            assert!(amount > 0, "WarpCollateral: amount must be > 0");
            let sender = drc20::sender_account();
            let self_account = Account::Contract(abi::self_id());

            // Lock tokens: transfer_from(sender → this contract)
            let _: () = abi::call(
                self.wrapped_token,
                "transfer_from",
                &TransferFromCall {
                    owner: sender,
                    to: self_account,
                    value: amount,
                },
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
                matches!(enrolled, Some(enrolled_sender) if *enrolled_sender == sender),
                "WarpCollateral: sender is not enrolled router for origin"
            );

            // Decode token message
            let msg = token_message::decode(&body).expect("WarpCollateral: invalid token message");
            assert!(msg.amount > 0, "WarpCollateral: amount must be > 0");

            // Pending claims reserve custody. A registered delivery must not
            // consume collateral already promised to an unregistered
            // recipient, and a new escrow may not finalize an unbacked claim.
            self.assert_unreserved_collateral(msg.amount);

            // If the recipient is registered, transfer immediately.
            // Otherwise, hold the wrapped tokens in this contract's DRC20
            // balance until the recipient registers and claims them.
            if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
                let _: () = abi::call(
                    self.wrapped_token,
                    "transfer",
                    &TransferCall {
                        to: Account::External(*pk),
                        value: msg.amount,
                    },
                )
                .expect("WarpCollateral: transfer failed");
            } else {
                let pending = self.pending_transfers.entry(msg.recipient).or_insert(0);
                *pending = pending
                    .checked_add(msg.amount)
                    .expect("WarpCollateral: pending overflow");
                self.pending_total = self
                    .pending_total
                    .checked_add(msg.amount)
                    .expect("WarpCollateral: total pending overflow");
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
            assert!(
                router != [0u8; 32],
                "WarpCollateral: router cannot be zero"
            );
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

        /// Withdraw this route's unused Mailbox dispatch-fee credit.
        ///
        /// Owner only. The Mailbox sees this route as the payer and sends the
        /// withdrawn native DUSK to the explicit Moonlight recipient.
        pub fn withdraw_dispatch_credit(&mut self, recipient: AccountPublicKey, amount: u64) {
            self.only_owner();
            let _: () = abi::call(
                self.mailbox,
                "withdraw_dispatch_credit",
                &(recipient, amount),
            )
            .expect("WarpCollateral: dispatch credit withdrawal failed");
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            assert!(
                new_owner != [0u8; 32],
                "WarpCollateral: new owner cannot be zero"
            );
            let previous_owner = self.owner.expect("WarpCollateral: no owner set");
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
            let owner = self.owner.expect("WarpCollateral: no owner set");
            assert!(
                caller::effective_caller() == owner,
                "WarpCollateral: caller is not the owner"
            );
        }

        /// Transfer and clear a pending balance to an authenticated account.
        fn claim_pending_to(&mut self, recipient: H256, account: Account) {
            let amount = self.pending_transfers.remove(&recipient).unwrap_or(0);
            assert!(amount > 0, "WarpCollateral: no pending transfers");
            self.pending_total = self
                .pending_total
                .checked_sub(amount)
                .expect("WarpCollateral: pending liability underflow");

            let _: () = abi::call(
                self.wrapped_token,
                "transfer",
                &TransferCall {
                    to: account,
                    value: amount,
                },
            )
            .expect("WarpCollateral: transfer failed");
            abi::emit(
                events::PendingTransferClaimed::TOPIC,
                events::PendingTransferClaimed { recipient, amount },
            );
        }

        /// Ensure `amount` can be paid without consuming existing escrow.
        fn assert_unreserved_collateral(&self, amount: u64) {
            let balance: u64 = abi::call(
                self.wrapped_token,
                "balance_of",
                &BalanceOf {
                    account: Account::Contract(abi::self_id()),
                },
            )
            .expect("WarpCollateral: balance query failed");
            let available = balance
                .checked_sub(self.pending_total)
                .expect("WarpCollateral: pending liability exceeds custody");
            assert!(
                available >= amount,
                "WarpCollateral: insufficient unreserved collateral"
            );
        }
    }
}
