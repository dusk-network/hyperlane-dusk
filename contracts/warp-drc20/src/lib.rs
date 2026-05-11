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
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane WarpDrc20 synthetic token contract.
#[dusk_forge::contract]
mod warp_drc20 {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::cmp::Ordering;

    use bytecheck::CheckBytes;
    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};
    use dusk_core::signatures::bls::PublicKey as AccountPublicKey;
    use dusk_core::transfer::TRANSFER_CONTRACT;
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

    /// A DRC20 account — either an external BLS key or a contract.
    ///
    /// This is layout-compatible with the DRC20 reference `Account` type
    /// so that standard DRC20 callers work seamlessly.
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

    /// Convert a DRC20 account into an indexable H256.
    fn account_id(account: Account) -> H256 {
        match account {
            Account::External(pk) => message::keccak256(&pk.to_bytes()),
            Account::Contract(id) => id.to_bytes(),
        }
    }

    /// Resolve the caller as an Account.
    fn sender_account() -> Account {
        if abi::callstack().len() == 1 {
            Account::External(
                abi::public_sender().expect("WarpDrc20: shielded transactions not supported"),
            )
        } else {
            Account::Contract(abi::caller().expect("WarpDrc20: missing caller"))
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
        /// Contract owner: keccak256(bls_public_key.to_bytes()).
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
    }

    impl WarpDrc20 {
        /// Creates a new empty state.
        pub const fn new() -> Self {
            Self {
                balances: BTreeMap::new(),
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
            }
        }

        // =================================================================
        // Initialization
        // =================================================================

        /// Initialize the warp route.
        #[contract(emits = [
            (events::Initialized::TOPIC, events::Initialized),
            (events::RemoteRouterEnrolled::TOPIC, events::RemoteRouterEnrolled)
        ])]
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
        #[contract(emits = [(events::AccountRegistered::TOPIC, events::AccountRegistered)])]
        pub fn register_account(&mut self) {
            let pk = abi::public_sender()
                .expect("WarpDrc20: register_account requires Moonlight TX");
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
        pub fn balance_of(&self, account: Account) -> u64 {
            self.balances.get(&account).copied().unwrap_or(0)
        }

        /// Transfer tokens from the caller to a recipient.
        #[contract(emits = [(events::Drc20Transfer::TOPIC, events::Drc20Transfer)])]
        pub fn transfer(&mut self, to: Account, value: u64) {
            let from = sender_account();
            self.do_transfer(from, to, value);
        }

        // =================================================================
        // Warp Route: transfer_remote (send)
        // =================================================================

        /// Send tokens to a remote chain.
        ///
        /// Burns `amount` from the caller and dispatches a Hyperlane message
        /// to the enrolled router on the destination domain.
        #[contract(emits = [
            (events::SentTransferRemote::TOPIC, events::SentTransferRemote),
            (events::Drc20Transfer::TOPIC, events::Drc20Transfer)
        ])]
        pub fn transfer_remote(
            &mut self,
            destination: u32,
            recipient: H256,
            amount: u64,
        ) -> MessageId {
            assert!(amount > 0, "WarpDrc20: amount must be > 0");
            let sender = sender_account();

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
        #[contract(emits = [
            (events::ReceivedTransferRemote::TOPIC, events::ReceivedTransferRemote),
            (events::Drc20Transfer::TOPIC, events::Drc20Transfer)
        ])]
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
                enrolled.is_some() && *enrolled.unwrap() == sender,
                "WarpDrc20: sender is not enrolled router for origin"
            );

            // Decode token message
            let msg =
                token_message::decode(&body).expect("WarpDrc20: invalid token message");

            // Resolve the recipient: if a BLS key is registered for this H256,
            // mint to the External account; otherwise mint to Contract account.
            let recipient_account =
                if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
                    Account::External(*pk)
                } else {
                    Account::Contract(ContractId::from_bytes(msg.recipient))
                };
            self.mint(recipient_account, msg.amount);

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
        #[contract(emits = [(events::RemoteRouterEnrolled::TOPIC, events::RemoteRouterEnrolled)])]
        pub fn enroll_remote_router(&mut self, domain: u32, router: H256) {
            self.only_owner();
            self.enrolled_routers.insert(domain, router);
            abi::emit(
                events::RemoteRouterEnrolled::TOPIC,
                events::RemoteRouterEnrolled { domain, router },
            );
        }

        /// Set the hook override. Owner only.
        #[contract(emits = [(events::HookSet::TOPIC, events::HookSet)])]
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
        #[contract(emits = [(events::IsmSet::TOPIC, events::IsmSet)])]
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
        #[contract(emits = [(events::OwnershipTransferred::TOPIC, events::OwnershipTransferred)])]
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
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

        /// Transfer tokens between accounts.
        fn do_transfer(&mut self, from: Account, to: Account, value: u64) {
            let from_balance = self.balances.get(&from).copied().unwrap_or(0);
            assert!(
                from_balance >= value,
                "WarpDrc20: insufficient balance"
            );
            *self.balances.entry(from).or_insert(0) -= value;
            let to_balance = self.balances.entry(to).or_insert(0);
            *to_balance = to_balance
                .checked_add(value)
                .expect("WarpDrc20: balance overflow");
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
            let balance = self.balances.entry(account).or_insert(0);
            *balance = balance
                .checked_add(amount)
                .expect("WarpDrc20: balance overflow");
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
            *self.balances.entry(account).or_insert(0) -= amount;
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
            let caller = abi::caller().expect("WarpDrc20: cannot determine caller");
            let sender_h = if caller == TRANSFER_CONTRACT {
                let sender = abi::public_sender().expect("WarpDrc20: no Moonlight sender");
                message::keccak256(&sender.to_bytes())
            } else {
                caller.to_bytes()
            };
            assert!(sender_h == owner, "WarpDrc20: caller is not the owner");
        }
    }
}
