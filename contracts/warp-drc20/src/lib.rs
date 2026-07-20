// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane WarpDrc20 (Synthetic) token contract for Dusk.

//! A synthetic DRC20 token that bridges foreign tokens to Dusk via Hyperlane.
//!
//! This contract IS a DRC20 token that mints tokens when receiving cross-chain
//! transfers (via `handle`) and burns tokens when sending cross-chain
//! transfers (via `transfer_remote`).
//!
//! Equivalent to `HypERC20` in the Solidity Hyperlane contracts.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::large_types_passed_by_value)] // Contract ABI takes archived calls by value.
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane WarpDrc20 synthetic token contract.
#[dusk_forge::contract(events = [
    events::AccountRegistered,
    events::Drc20Approval,
    events::Drc20Transfer,
    events::HookSet,
    events::Initialized,
    events::IsmSet,
    events::OwnershipTransferred,
    events::PendingTransferClaimed,
    events::ReceivedTransferRemote,
    events::RemoteRouterEnrolled,
    events::SentTransferRemote,
])]
mod warp_drc20 {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::signatures::bls::PublicKey as AccountPublicKey;

    use dusk_bytes::Serializable;

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::drc20::{
        self, Account, Allowance, ApproveCall, BalanceOf, TransferCall, TransferFromCall,
    };
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::token_message;
    use hyperlane_dusk_types::{MessageId, H256};

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Convert a DRC20 account into an indexable H256.
    fn account_id(account: Account) -> H256 {
        match account {
            Account::External(pk) => message::keccak256(&pk.to_bytes()),
            Account::Contract(id) => id.to_bytes(),
        }
    }

    // =====================================================================
    // State
    // =====================================================================

    /// WarpDrc20 contract state.
    pub struct WarpDrc20 {
        // -- DRC20 token state --
        /// Token balances per account.
        balances: BTreeMap<Account, u64>,
        /// Allowances keyed by token owner and approved spender.
        allowances: BTreeMap<Account, BTreeMap<Account, u64>>,
        /// Total token supply.
        supply: u64,
        /// Token name.
        name: String,
        /// Token symbol.
        symbol: String,
        /// Token decimals.
        decimals: u8,

        // -- Router state --
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
        /// so that inbound transfers can look up the full key and mint
        /// to their External account.
        registered_accounts: BTreeMap<H256, AccountPublicKey>,
        /// Inbound synthetic balances waiting for an authenticated recipient.
        ///
        /// An unregistered H256 is ambiguous on Dusk: it can be either a
        /// hashed Moonlight public key or a contract ID. Keep the amount
        /// unminted until one of those recipient types proves ownership.
        pending_transfers: BTreeMap<H256, u64>,
        /// Aggregate not-yet-minted liability reserved for pending claims.
        pending_total: u64,
    }

    impl WarpDrc20 {
        /// Creates a new empty state.
        pub const fn new() -> Self {
            Self {
                balances: BTreeMap::new(),
                allowances: BTreeMap::new(),
                supply: 0,
                name: String::new(),
                symbol: String::new(),
                decimals: 0,
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

        /// Initialize the warp route.
        pub fn init(
            &mut self,
            mailbox: ContractId,
            owner: H256,
            name: String,
            symbol: String,
            decimals: u8,
            enrolled_routers: Vec<(u32, H256)>,
        ) {
            assert!(self.owner.is_none(), "WarpDrc20: already initialized");
            assert!(owner != [0u8; 32], "WarpDrc20: owner cannot be zero");
            assert!(
                mailbox != ZERO_CONTRACT,
                "WarpDrc20: mailbox cannot be zero"
            );
            self.mailbox = mailbox;
            self.owner = Some(owner);
            self.name = name;
            self.symbol = symbol;
            self.decimals = decimals;
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
                    contract_type: events::CONTRACT_WARP_DRC20,
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
        /// can resolve the H256 recipient to a full account key and mint
        /// to an External account.
        ///
        /// Reads the sender from `abi::public_sender()` (Moonlight TX).
        /// Stores `keccak256(pk.to_bytes()) → pk`.
        pub fn register_account(&mut self) {
            let pk =
                abi::public_sender().expect("WarpDrc20: register_account requires Moonlight TX");
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

        /// Claim pending synthetic tokens for the calling Moonlight account.
        ///
        /// The H256 recipient is derived from the authenticated public sender,
        /// so an arbitrary account cannot claim another recipient's balance.
        pub fn claim_pending(&mut self) {
            let pk = abi::public_sender().expect("WarpDrc20: claim_pending requires Moonlight TX");
            let h = message::keccak256(&pk.to_bytes());
            self.claim_pending_to(h, Account::External(pk));
        }

        /// Claim pending synthetic tokens for the calling contract.
        ///
        /// The pending-recipient key is the immediate caller's `ContractId`
        /// bytes. The Moonlight transfer contract is rejected as a root
        /// caller so it cannot be confused with the intended recipient.
        pub fn claim_pending_contract(&mut self) {
            let contract = abi::caller().expect("WarpDrc20: contract caller unavailable");
            assert!(
                contract != dusk_core::transfer::TRANSFER_CONTRACT,
                "WarpDrc20: claim_pending_contract requires contract caller"
            );
            self.claim_pending_to(contract.to_bytes(), Account::Contract(contract));
        }

        /// Returns the pending, not-yet-minted balance for an H256 recipient.
        pub fn pending_balance(&self, h: H256) -> u64 {
            self.pending_transfers.get(&h).copied().unwrap_or(0)
        }

        /// Returns the aggregate synthetic liability reserved for claims.
        pub fn pending_total(&self) -> u64 {
            self.pending_total
        }

        /// Storage/escrow ABI version for deployment compatibility checks.
        #[allow(clippy::unused_self)] // Contract queries are instance methods in the Dusk ABI.
        pub fn state_version(&self) -> u32 {
            2
        }

        // =================================================================
        // DRC20 interface
        // =================================================================

        /// Returns the token name.
        pub fn name(&self) -> String {
            self.name.clone()
        }

        /// Returns the token symbol.
        pub fn symbol(&self) -> String {
            self.symbol.clone()
        }

        /// Returns the number of decimals.
        pub fn decimals(&self) -> u8 {
            self.decimals
        }

        /// Returns the total token supply.
        pub fn total_supply(&self) -> u64 {
            self.supply
        }

        /// Returns the balance of an account.
        pub fn balance_of(&self, args: BalanceOf) -> u64 {
            self.balances.get(&args.account).copied().unwrap_or(0)
        }

        /// Returns the allowance granted by an owner to a spender.
        pub fn allowance(&self, args: Allowance) -> u64 {
            self.allowances
                .get(&args.owner)
                .and_then(|allowances| allowances.get(&args.spender).copied())
                .unwrap_or(0)
        }

        /// Transfer tokens from the caller to a recipient.
        pub fn transfer(&mut self, args: TransferCall) {
            self.do_transfer(drc20::sender_account(), args.to, args.value);
        }

        /// Approve a spender to transfer tokens on behalf of the caller.
        pub fn approve(&mut self, args: ApproveCall) {
            assert!(
                account_id(args.spender) != [0u8; 32],
                "WarpDrc20: spender cannot be zero"
            );
            let owner = drc20::sender_account();
            self.set_allowance(owner, args.spender, args.value);
            abi::emit(
                events::Drc20Approval::TOPIC,
                events::Drc20Approval {
                    owner: account_id(owner),
                    spender: account_id(args.spender),
                    amount: args.value,
                },
            );
        }

        /// Transfer tokens from an owner using the caller's allowance.
        pub fn transfer_from(&mut self, args: TransferFromCall) {
            let spender = drc20::sender_account();
            let current = self.allowance(Allowance {
                owner: args.owner,
                spender,
            });
            assert!(current >= args.value, "WarpDrc20: allowance too low");
            if args.value > 0 {
                self.set_allowance(args.owner, spender, current - args.value);
            }
            self.do_transfer(args.owner, args.to, args.value);
        }

        // =================================================================
        // Warp Route: transfer_remote (send)
        // =================================================================

        /// Send tokens to a remote chain.
        ///
        /// Burns `amount` from the caller and dispatches a Hyperlane message
        /// to the enrolled router on the destination domain.
        pub fn transfer_remote(
            &mut self,
            destination: u32,
            recipient: H256,
            amount: u64,
        ) -> MessageId {
            assert!(amount > 0, "WarpDrc20: amount must be > 0");
            let sender = drc20::sender_account();

            // Burn tokens from sender
            self.burn(sender, amount);

            // Look up enrolled router
            let router = self
                .enrolled_routers
                .get(&destination)
                .expect("WarpDrc20: no router enrolled for destination");

            // Encode token message body
            let body = token_message::encode(recipient, amount);

            // Dispatch via Mailbox
            let message_id: MessageId = abi::call(
                self.mailbox,
                "dispatch",
                &(destination, *router, body, Vec::<u8>::new(), self.hook),
            )
            .expect("WarpDrc20: dispatch failed");

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
        // Warp Route: handle (receive)
        // =================================================================

        /// Handle an incoming cross-chain message.
        ///
        /// Called by the Mailbox when a message is delivered. Mints tokens
        /// to the recipient specified in the token message body.
        pub fn handle(&mut self, origin: u32, sender: H256, body: Vec<u8>) {
            // Verify caller is the Mailbox
            let caller = abi::caller().expect("WarpDrc20: cannot determine caller");
            assert!(
                caller == self.mailbox,
                "WarpDrc20: caller is not the Mailbox"
            );

            // Verify sender is an enrolled router
            let enrolled = self.enrolled_routers.get(&origin);
            assert!(
                matches!(enrolled, Some(enrolled_sender) if *enrolled_sender == sender),
                "WarpDrc20: sender is not enrolled router for origin"
            );

            // Decode token message
            let msg = token_message::decode(&body).expect("WarpDrc20: invalid token message");
            assert!(msg.amount > 0, "WarpDrc20: amount must be > 0");

            // A registered BLS hash is unambiguously an external account.
            // An unregistered H256 could also be a contract ID, so do not
            // fabricate an account type. Keep the value unminted until the
            // external account or contract proves that it owns the key.
            if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
                self.ensure_mint_capacity(msg.amount);
                self.mint(Account::External(*pk), msg.amount);
            } else {
                self.ensure_mint_capacity(msg.amount);
                let pending = self.pending_transfers.entry(msg.recipient).or_insert(0);
                *pending = pending
                    .checked_add(msg.amount)
                    .expect("WarpDrc20: pending overflow");
                self.pending_total = self
                    .pending_total
                    .checked_add(msg.amount)
                    .expect("WarpDrc20: pending total overflow");
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
        ///
        /// Called by the Mailbox to determine which ISM to use when
        /// processing messages addressed to this contract.
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

        /// Returns the owner (keccak256 of the BLS public key).
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
            .expect("WarpDrc20: dispatch credit withdrawal failed");
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            assert!(
                new_owner != [0u8; 32],
                "WarpDrc20: new owner cannot be zero"
            );
            let previous_owner = self.owner.expect("WarpDrc20: no owner set");
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

        /// Mint and clear a pending balance to an authenticated account.
        fn claim_pending_to(&mut self, recipient: H256, account: Account) {
            let amount = self.pending_transfers.remove(&recipient).unwrap_or(0);
            assert!(amount > 0, "WarpDrc20: no pending transfers");
            self.pending_total = self
                .pending_total
                .checked_sub(amount)
                .expect("WarpDrc20: pending total underflow");
            self.mint(account, amount);
            abi::emit(
                events::PendingTransferClaimed::TOPIC,
                events::PendingTransferClaimed { recipient, amount },
            );
        }

        /// Ensure a direct mint or new pending liability cannot consume the
        /// supply capacity already promised to pending recipients.
        fn ensure_mint_capacity(&self, amount: u64) {
            self.supply
                .checked_add(self.pending_total)
                .and_then(|reserved| reserved.checked_add(amount))
                .expect("WarpDrc20: insufficient supply capacity");
        }

        /// Transfer tokens between accounts.
        fn do_transfer(&mut self, from: Account, to: Account, value: u64) {
            let from_balance = self.balances.get(&from).copied().unwrap_or(0);
            assert!(from_balance >= value, "WarpDrc20: insufficient balance");
            if value > 0 && from != to {
                let remaining = from_balance - value;
                if remaining == 0 {
                    self.balances.remove(&from);
                } else {
                    self.balances.insert(from, remaining);
                }
                let to_balance = self.balances.get(&to).copied().unwrap_or(0);
                self.balances.insert(
                    to,
                    to_balance
                        .checked_add(value)
                        .expect("WarpDrc20: balance overflow"),
                );
            }
            abi::emit(
                events::Drc20Transfer::TOPIC,
                events::Drc20Transfer {
                    from: account_id(from),
                    to: account_id(to),
                    amount: value,
                },
            );
        }

        /// Mint tokens to an account.
        fn mint(&mut self, account: Account, amount: u64) {
            if amount > 0 {
                let balance = self.balances.get(&account).copied().unwrap_or(0);
                self.balances.insert(
                    account,
                    balance
                        .checked_add(amount)
                        .expect("WarpDrc20: balance overflow"),
                );
            }
            self.supply = self
                .supply
                .checked_add(amount)
                .expect("WarpDrc20: supply overflow");
            abi::emit(
                events::Drc20Transfer::TOPIC,
                events::Drc20Transfer {
                    from: ZERO_CONTRACT.to_bytes(),
                    to: account_id(account),
                    amount,
                },
            );
        }

        /// Burn tokens from an account.
        fn burn(&mut self, account: Account, amount: u64) {
            let balance = self.balances.get(&account).copied().unwrap_or(0);
            assert!(balance >= amount, "WarpDrc20: insufficient balance to burn");
            if amount > 0 {
                let remaining = balance - amount;
                if remaining == 0 {
                    self.balances.remove(&account);
                } else {
                    self.balances.insert(account, remaining);
                }
            }
            self.supply = self
                .supply
                .checked_sub(amount)
                .expect("WarpDrc20: supply underflow");
            abi::emit(
                events::Drc20Transfer::TOPIC,
                events::Drc20Transfer {
                    from: account_id(account),
                    to: ZERO_CONTRACT.to_bytes(),
                    amount,
                },
            );
        }

        /// Panics if the resolved admin sender is not the owner.
        fn only_owner(&self) {
            let owner = self.owner.expect("WarpDrc20: no owner set");
            assert!(
                caller::effective_caller() == owner,
                "WarpDrc20: caller is not the owner"
            );
        }

        /// Set or clear a sparse allowance entry.
        fn set_allowance(&mut self, owner: Account, spender: Account, value: u64) {
            if value == 0 {
                let remove_owner = if let Some(spenders) = self.allowances.get_mut(&owner) {
                    spenders.remove(&spender);
                    spenders.is_empty()
                } else {
                    false
                };
                if remove_owner {
                    self.allowances.remove(&owner);
                }
            } else {
                self.allowances
                    .entry(owner)
                    .or_default()
                    .insert(spender, value);
            }
        }
    }
}
