// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane WarpDrc20 (Synthetic) token contract for Dusk.

//! A synthetic DRC20 token that bridges foreign tokens to Dusk via Hyperlane.
//!
//! This contract IS a DRC20 token that mints tokens when receiving cross-chain
//! transfers (via `handle`) and burns tokens when sending cross-chain
//! transfers (via `transfer_remote`). Tokens for an account the route hasn't
//! minted to before wait in escrow until they are claimed, which also
//! registers the account for later transfers.
//!
//! Two limits keep holders from filling its memory: an owner holds at most
//! 64 allowances, and transfers and approvals that would add an entry stop
//! once the memory would pass 3 GiB, leaving the last GiB to the bridge's
//! mints, escrow and claims.
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
    use dusk_core::transfer::{ContractToContract, TRANSFER_CONTRACT};

    use dusk_bytes::Serializable;

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::drc20::{
        self, account_id, Account, AccountId, Allowance, ApproveCall, BalanceOf, TransferCall,
        TransferFromCall,
    };
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::token_message;
    use hyperlane_dusk_types::{MessageId, H256};

    /// Zero contract ID used as "no contract set".
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Most allowances one owner holds at once. Approving another spender
    /// reverts until the owner sets one of them to zero, which removes it.
    const MAX_ALLOWANCES_PER_OWNER: usize = 64;

    /// Size of a wasm32 memory page.
    const WASM_PAGE_BYTES: u64 = 64 * 1024;

    /// Memory a transfer to a new holder or an approval of a new spender
    /// never grows this contract past: 3 GiB of the 4 GiB a wasm32 contract
    /// can address. Memory never shrinks, so the last GiB stays for the
    /// bridge: mints, escrow, and the registrations and mints of claims,
    /// which come only from transfers someone paid for on another chain.
    const HOLDER_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;

    /// The most B-tree nodes adding a key to a map allocates: one per level
    /// of the tree when every level splits, and a new root. Every node but
    /// the root holds at least 5 keys, so a map of n keys has at most
    /// log6((n + 1) / 2) + 1 levels, 12 for any n below 2^32, and an
    /// insertion allocates at most 13 nodes.
    const MAX_NODES_PER_INSERT: u64 = 13;

    /// The most memory adding a balance can grow this contract by. A node
    /// of these maps takes under 600 bytes, so dlmalloc, which grows memory
    /// in whole pages when no free block fits, grows it by at most a page
    /// for each.
    const NEW_BALANCE_MEMORY_BOUND: u64 = MAX_NODES_PER_INSERT * WASM_PAGE_BYTES;

    /// The most memory adding an allowance can grow this contract by: the
    /// owners' map, and the first node of the owner's own map of spenders.
    const NEW_ALLOWANCE_MEMORY_BOUND: u64 = (MAX_NODES_PER_INSERT + 1) * WASM_PAGE_BYTES;

    // =====================================================================
    // State
    // =====================================================================

    /// WarpDrc20 contract state.
    pub struct WarpDrc20 {
        // -- DRC20 token state --
        /// Token balances, keyed by the holder's [`account_id`]. A zero
        /// balance has no entry.
        balances: BTreeMap<AccountId, u64>,
        /// Allowances, keyed by the owner's and then the spender's
        /// [`account_id`]. A zero allowance has no entry, an owner with
        /// none has no map, and an owner holds at most
        /// [`MAX_ALLOWANCES_PER_OWNER`].
        allowances: BTreeMap<AccountId, BTreeMap<AccountId, u64>>,
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
        /// fit in a 32-byte H256. An account is registered when its escrow
        /// is claimed with `claim_pending` or `claim_pending_for`, so that
        /// later inbound transfers can look up the full key and mint to its
        /// External account. Nothing else registers one, so every entry
        /// stands for a transfer someone sent it. Every stored key is
        /// canonical (see [`canonical_account`]).
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
                assert!(router != [0u8; 32], "WarpDrc20: router cannot be zero");
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

        /// Check whether an H256 has a registered account.
        pub fn is_registered(&self, h: H256) -> bool {
            self.registered_accounts.contains_key(&h)
        }

        /// Register the transaction's sender and mint it the tokens escrowed
        /// for its hash.
        ///
        /// The H256 recipient is derived from the authenticated public sender,
        /// so an arbitrary account cannot claim another recipient's balance.
        /// The sender is `abi::public_sender()`, which the node decodes from
        /// the transaction's compressed encoding, so it is already
        /// canonical. Registering it means later transfers to it are minted
        /// directly, as after [`Self::claim_pending_for`]. A call with
        /// nothing in escrow reverts and registers nothing.
        pub fn claim_pending(&mut self) {
            let pk = abi::public_sender().expect("WarpDrc20: claim_pending requires Moonlight TX");
            let h = self.register(pk);
            self.claim_pending_to(h, Account::moonlight(&pk));
        }

        /// Register `account` and mint it the tokens escrowed for
        /// `keccak256(account)`.
        ///
        /// Anyone can call this and pay its gas, so a recipient that holds no
        /// DUSK can still receive. `account` is decoded to its canonical form
        /// first, and the escrow lookup, the registration and the mint all
        /// use that key: the one its owner signs with, whatever limbs the
        /// caller sends. A key that doesn't decode to a valid public key
        /// reverts, and so does a call with nothing in escrow, so this can't
        /// register a key nobody has sent tokens to.
        pub fn claim_pending_for(&mut self, account: AccountPublicKey) {
            let account = canonical_account(&account);
            let h = self.register(account);
            self.claim_pending_to(h, Account::moonlight(&account));
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

        /// Deployment compatibility version. Version 8 drops
        /// `register_account`, holds an owner to 64 allowances, stops
        /// transfers to new holders and approvals of new spenders at 3 GiB
        /// of memory, refuses transfers to the zero account, and adds
        /// `memory_bytes` and `allowance_count`. Version 7 stores balances and allowances under 32-byte
        /// account IDs instead of the principals; the ABI is unchanged.
        /// Version 6: `claim_pending_for` mints to the decoded key, and
        /// `claim_pending` registers the sender. Version 5 added
        /// `claim_pending_for`.
        #[allow(clippy::unused_self)] // Contract queries are instance methods in the Dusk ABI.
        pub fn state_version(&self) -> u32 {
            8
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
            self.balance(&account_id(&args.account))
        }

        /// Returns the allowance granted by an owner to a spender.
        pub fn allowance(&self, args: Allowance) -> u64 {
            self.allowance_of(&account_id(&args.owner), &account_id(&args.spender))
        }

        /// Returns how many spenders `owner` has a live allowance for. Once
        /// it has 64, approving another spender reverts until one of them is
        /// set to zero.
        pub fn allowance_count(&self, owner: Account) -> u32 {
            let count = self
                .allowances
                .get(&account_id(&owner))
                .map_or(0, BTreeMap::len);
            u32::try_from(count).unwrap_or(u32::MAX)
        }

        /// Returns the size of this contract's memory in bytes. It only
        /// grows; transfers to new holders and approvals of new spenders stop
        /// before they could take it past 3 GiB, and mints, escrow and
        /// claims use the rest.
        #[allow(clippy::unused_self)] // Contract queries are instance methods in the Dusk ABI.
        pub fn memory_bytes(&self) -> u64 {
            memory_bytes_used()
        }

        /// Transfer tokens from the caller to a recipient.
        ///
        /// A transfer to an account with no balance adds a holder, and
        /// reverts once that could take this contract's memory past 3 GiB;
        /// transfers to holders keep working. A transfer to the zero account
        /// reverts.
        pub fn transfer(&mut self, args: TransferCall) {
            let from = drc20::sender_account();
            let from_id = account_id(&from);
            self.do_transfer(from, from_id, args.to, args.amount);
        }

        /// Approve a spender to transfer tokens on behalf of the caller.
        ///
        /// An owner holds at most 64 allowances: approving another spender
        /// reverts until the owner approves zero for one of them, which
        /// removes it. A new spender also reverts once its allowance could
        /// take this contract's memory past 3 GiB. Changing an existing
        /// allowance always works.
        pub fn approve(&mut self, args: ApproveCall) {
            assert!(!args.spender.is_zero(), "WarpDrc20: spender cannot be zero");
            let owner = drc20::sender_account();
            self.set_allowance(account_id(&owner), account_id(&args.spender), args.amount);
            abi::emit(
                events::Drc20Approval::TOPIC,
                events::Drc20Approval {
                    owner,
                    spender: args.spender,
                    amount: args.amount,
                },
            );
        }

        /// Transfer tokens from an owner using the caller's allowance.
        pub fn transfer_from(&mut self, args: TransferFromCall) {
            let owner_id = account_id(&args.owner);
            let spender_id = account_id(&drc20::sender_account());
            let current = self.allowance_of(&owner_id, &spender_id);
            assert!(current >= args.amount, "WarpDrc20: allowance too low");
            if args.amount > 0 {
                self.set_allowance(owner_id, spender_id, current - args.amount);
            }
            self.do_transfer(args.owner, owner_id, args.to, args.amount);
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
            assert!(
                recipient != [0u8; 32],
                "WarpDrc20: recipient cannot be zero"
            );
            // Look up enrolled router
            let router = *self
                .enrolled_routers
                .get(&destination)
                .expect("WarpDrc20: no router enrolled for destination");

            // Encode token message body
            let body = token_message::encode(recipient, amount);
            let credit_before = self.dispatch_credit();
            let dispatch_fee = self.quote_dispatch(destination, router, body.clone());
            self.collect_and_forward_dispatch_fee(dispatch_fee);

            // Burn tokens from sender only after the fee contribution is
            // authenticated. Any later failure rolls the whole call back.
            self.burn(drc20::sender_account(), amount);

            // Dispatch via Mailbox
            let message_id: MessageId = abi::call(
                self.mailbox,
                "dispatch",
                &(destination, router, body, Vec::<u8>::new(), self.hook),
            )
            .expect("WarpDrc20: dispatch failed");
            assert_eq!(
                self.dispatch_credit(),
                credit_before,
                "WarpDrc20: dispatch must consume only caller-funded credit"
            );

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

        /// Quote the native-DUSK fee that must accompany `transfer_remote`.
        pub fn quote_transfer_remote(&self, destination: u32, recipient: H256, amount: u64) -> u64 {
            assert!(amount > 0, "WarpDrc20: amount must be > 0");
            assert!(
                recipient != [0u8; 32],
                "WarpDrc20: recipient cannot be zero"
            );
            let router = self
                .enrolled_routers
                .get(&destination)
                .expect("WarpDrc20: no router enrolled for destination");
            self.quote_dispatch(
                destination,
                *router,
                token_message::encode(recipient, amount),
            )
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
            assert!(
                msg.recipient != [0u8; 32],
                "WarpDrc20: recipient cannot be zero"
            );

            // A registered BLS hash is unambiguously an external account.
            // An unregistered H256 could also be a contract ID, so do not
            // fabricate an account type. Keep the value unminted until the
            // external account or contract proves that it owns the key.
            if let Some(pk) = self.registered_accounts.get(&msg.recipient) {
                self.ensure_mint_capacity(msg.amount);
                self.mint(Account::moonlight(pk), msg.amount);
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
            assert!(router != [0u8; 32], "WarpDrc20: router cannot be zero");
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
        /// withdrawn native DUSK to the explicit Moonlight recipient. It
        /// decodes the recipient's key first and pays the account that signs
        /// with it, so a forged twin of the key can't divert the payout; the
        /// decoding costs about 57M gas.
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

        /// Map `keccak256(pk)` to `pk` and return the hash. `pk` must be
        /// canonical: the transaction's public sender, or a key from
        /// [`canonical_account`].
        fn register(&mut self, pk: AccountPublicKey) -> H256 {
            let h = message::keccak256(&pk.to_bytes());
            self.registered_accounts.insert(h, pk);
            abi::emit(
                events::AccountRegistered::TOPIC,
                events::AccountRegistered { account_hash: h },
            );
            h
        }

        /// Query the route's current Mailbox credit.
        fn dispatch_credit(&self) -> u64 {
            abi::call(self.mailbox, "fee_credit", &(abi::self_id().to_bytes(),))
                .expect("WarpDrc20: fee credit query failed")
        }

        /// Quote dispatch with this route as the encoded sender.
        fn quote_dispatch(&self, destination: u32, router: H256, body: Vec<u8>) -> u64 {
            abi::call(
                self.mailbox,
                "quote_dispatch_for_contract",
                &(destination, router, body, Vec::<u8>::new(), self.hook),
            )
            .expect("WarpDrc20: dispatch quote failed")
        }

        /// Claim the caller's exact Moonlight deposit and forward it to Mailbox.
        fn collect_and_forward_dispatch_fee(&self, fee: u64) {
            if fee == 0 {
                return;
            }
            let _: () = abi::call(TRANSFER_CONTRACT, "deposit", &fee)
                .expect("WarpDrc20: dispatch fee deposit failed");
            self.forward_dispatch_fee(fee);
        }

        /// Transfer collected DUSK to Mailbox, which credits this route.
        fn forward_dispatch_fee(&self, fee: u64) {
            let transfer = ContractToContract {
                contract: self.mailbox,
                value: fee,
                fn_name: String::from("receive_dispatch_funding"),
                data: Vec::new(),
            };
            let _: () = abi::call(TRANSFER_CONTRACT, "contract_to_contract", &transfer)
                .expect("WarpDrc20: dispatch fee forwarding failed");
        }

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

        /// Returns the balance stored under `id`.
        fn balance(&self, id: &AccountId) -> u64 {
            self.balances.get(id).copied().unwrap_or(0)
        }

        /// Stores `balance` under `id`, or removes the entry when it is zero.
        fn set_balance(&mut self, id: AccountId, balance: u64) {
            if balance == 0 {
                self.balances.remove(&id);
            } else {
                self.balances.insert(id, balance);
            }
        }

        /// Returns the allowance stored for an owner and spender ID.
        fn allowance_of(&self, owner: &AccountId, spender: &AccountId) -> u64 {
            self.allowances
                .get(owner)
                .and_then(|allowances| allowances.get(spender).copied())
                .unwrap_or(0)
        }

        /// Transfer tokens between accounts, for `transfer` and
        /// `transfer_from`. `from_id` is `from`'s account ID.
        fn do_transfer(&mut self, from: Account, from_id: AccountId, to: Account, value: u64) {
            // Nobody can spend from the zero account, and its transfer event
            // would read as a burn while the supply stays the same.
            assert!(!to.is_zero(), "WarpDrc20: recipient cannot be zero");
            // No call ever resolves to a Phoenix principal, so tokens sent to
            // one could never be spent again.
            assert!(
                !matches!(to, Account::Phoenix(_)),
                "WarpDrc20: Phoenix recipients are not supported"
            );
            let from_balance = self.balance(&from_id);
            assert!(from_balance >= value, "WarpDrc20: insufficient balance");
            if value > 0 && from != to {
                let to_id = account_id(&to);
                let to_balance = self.balance(&to_id);
                // A new holder is an entry anyone can add for a token unit.
                // Checked before anything is stored or allocated.
                if to_balance == 0 {
                    assert!(
                        memory_bytes_used() + NEW_BALANCE_MEMORY_BOUND <= HOLDER_MEMORY_LIMIT,
                        "WarpDrc20: storage full, transfers to new holders are closed"
                    );
                }
                let to_balance = to_balance
                    .checked_add(value)
                    .expect("WarpDrc20: balance overflow");
                self.set_balance(from_id, from_balance - value);
                self.set_balance(to_id, to_balance);
            }
            abi::emit(
                events::Drc20Transfer::TOPIC,
                events::Drc20Transfer {
                    from,
                    to,
                    amount: value,
                },
            );
        }

        /// Mint tokens to an account.
        fn mint(&mut self, account: Account, amount: u64) {
            if amount > 0 {
                let id = account_id(&account);
                let balance = self
                    .balance(&id)
                    .checked_add(amount)
                    .expect("WarpDrc20: balance overflow");
                self.set_balance(id, balance);
            }
            self.supply = self
                .supply
                .checked_add(amount)
                .expect("WarpDrc20: supply overflow");
            abi::emit(
                events::Drc20Transfer::TOPIC,
                events::Drc20Transfer {
                    from: Account::Contract(ZERO_CONTRACT),
                    to: account,
                    amount,
                },
            );
        }

        /// Burn tokens from an account.
        fn burn(&mut self, account: Account, amount: u64) {
            let id = account_id(&account);
            let balance = self.balance(&id);
            assert!(balance >= amount, "WarpDrc20: insufficient balance to burn");
            if amount > 0 {
                self.set_balance(id, balance - amount);
            }
            self.supply = self
                .supply
                .checked_sub(amount)
                .expect("WarpDrc20: supply underflow");
            abi::emit(
                events::Drc20Transfer::TOPIC,
                events::Drc20Transfer {
                    from: account,
                    to: Account::Contract(ZERO_CONTRACT),
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

        /// Set or clear a sparse allowance entry. A new spender must fit
        /// under the owner's limit and the memory limit; only `approve`
        /// adds one, since `transfer_from` only lowers an allowance.
        fn set_allowance(&mut self, owner: AccountId, spender: AccountId, value: u64) {
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
                let spenders = self.allowances.get(&owner);
                if !spenders.is_some_and(|spenders| spenders.contains_key(&spender)) {
                    assert!(
                        spenders.map_or(0, BTreeMap::len) < MAX_ALLOWANCES_PER_OWNER,
                        "WarpDrc20: an owner holds at most 64 allowances"
                    );
                    assert!(
                        memory_bytes_used() + NEW_ALLOWANCE_MEMORY_BOUND <= HOLDER_MEMORY_LIMIT,
                        "WarpDrc20: storage full, new allowances are closed"
                    );
                }
                self.allowances
                    .entry(owner)
                    .or_default()
                    .insert(spender, value);
            }
        }
    }

    /// Current size of the contract's linear memory.
    fn memory_bytes_used() -> u64 {
        core::arch::wasm32::memory_size(0) as u64 * WASM_PAGE_BYTES
    }

    /// Decode a caller-supplied account key to the key its owner signs with.
    ///
    /// The archive check accepts any limbs in a public key. A recipient hash
    /// covers only the compressed encoding (`to_bytes`): x reduced mod p, the
    /// infinity flag and the sign of y. A Moonlight principal holds the raw
    /// limbs (`to_raw_bytes`). So a forged key can share a recipient's hash
    /// and still be minted to a principal nobody can sign for. Decoding the
    /// compressed bytes gives the one canonical key with that hash.
    ///
    /// # Panics
    ///
    /// Panics if the encoding isn't a valid public key, including the
    /// identity, which nobody can sign for.
    fn canonical_account(account: &AccountPublicKey) -> AccountPublicKey {
        AccountPublicKey::from_bytes(&account.to_bytes())
            .ok()
            // `from_bytes` checks that the point is on the curve and in G2,
            // so `is_valid` would only add the identity check, at the cost
            // of a second subgroup check (about 22M gas). The default key is
            // the identity.
            .filter(|pk| *pk != AccountPublicKey::default())
            .expect("WarpDrc20: invalid account public key")
    }
}
