// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Fee-on-transfer DRC20 with sender callbacks, for reentrancy tests.

//! Test-only DRC20 token that takes a fee on every transfer and calls back
//! into a sending contract before it moves the tokens.
//!
//! Real tokens do both. The fee is a share of each transfer, rounded down,
//! so a small transfer pays none, and it is burned. A contract that calls
//! `register_send_hook` gets `tokens_to_send(from, to, amount)` before every
//! `transfer` or `transfer_from` that moves its tokens, as ERC-777's
//! `tokensToSend` hook does, and can call other contracts from there. The
//! VM tests use it to re-enter the collateral route's `transfer_remote`
//! while the route's own `transfer_from` is still running.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::large_types_passed_by_value)] // Contract ABI takes archived calls by value.

/// Test-only fee-on-transfer DRC20 with sender callbacks.
#[dusk_forge::contract]
mod callback_fee_token {
    extern crate alloc;

    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId};
    use dusk_core::transfer::TRANSFER_CONTRACT;

    use hyperlane_dusk_types::drc20::{
        self, Account, Allowance, ApproveCall, BalanceOf, TransferCall, TransferFromCall,
    };

    /// Basis points in a whole.
    const BPS: u128 = 10_000;

    /// Token state.
    pub struct CallbackFeeToken {
        /// Balance of every account that holds tokens.
        balances: BTreeMap<Account, u64>,
        /// Remaining allowance per (owner, spender).
        allowances: BTreeMap<(Account, Account), u64>,
        /// Fee on each transfer, in basis points of the amount, rounded
        /// down. The fee is burned.
        fee_bps: u16,
        /// Contracts called before tokens leave them.
        send_hooks: BTreeSet<ContractId>,
        /// Tokens in existence.
        total_supply: u64,
    }

    impl CallbackFeeToken {
        /// Creates an empty token.
        pub const fn new() -> Self {
            Self {
                balances: BTreeMap::new(),
                allowances: BTreeMap::new(),
                fee_bps: 0,
                send_hooks: BTreeSet::new(),
                total_supply: 0,
            }
        }

        /// Sets the transfer fee and mints the initial balances.
        pub fn init(&mut self, fee_bps: u16, balances: Vec<(Account, u64)>) {
            assert!(
                u128::from(fee_bps) <= BPS,
                "CallbackFeeToken: fee above 100%"
            );
            self.fee_bps = fee_bps;
            for (account, amount) in balances {
                self.credit(account, amount);
                self.total_supply = self
                    .total_supply
                    .checked_add(amount)
                    .expect("CallbackFeeToken: supply overflow");
            }
        }

        /// Returns an account's balance.
        pub fn balance_of(&self, query: BalanceOf) -> u64 {
            self.balances.get(&query.account).copied().unwrap_or(0)
        }

        /// Returns what `spender` may still move of `owner`'s tokens.
        pub fn allowance(&self, query: Allowance) -> u64 {
            self.allowances
                .get(&(query.owner, query.spender))
                .copied()
                .unwrap_or(0)
        }

        /// Returns the tokens in existence.
        pub fn total_supply(&self) -> u64 {
            self.total_supply
        }

        /// Lets `spender` move up to `amount` of the caller's tokens.
        pub fn approve(&mut self, call: ApproveCall) {
            let owner = drc20::sender_account();
            self.allowances.insert((owner, call.spender), call.amount);
        }

        /// Moves `amount` of the caller's tokens to `to`, less the fee.
        pub fn transfer(&mut self, call: TransferCall) {
            let from = drc20::sender_account();
            self.notify_sender(from, call.to, call.amount);
            self.move_tokens(from, call.to, call.amount);
        }

        /// Moves `amount` of `owner`'s tokens to `to`, less the fee, out of
        /// the caller's allowance. The allowance is spent before the
        /// sender's hook runs.
        pub fn transfer_from(&mut self, call: TransferFromCall) {
            let key = (call.owner, drc20::sender_account());
            let allowance = self.allowances.get(&key).copied().unwrap_or(0);
            assert!(
                allowance >= call.amount,
                "CallbackFeeToken: insufficient allowance"
            );
            self.allowances.insert(key, allowance - call.amount);
            self.notify_sender(call.owner, call.to, call.amount);
            self.move_tokens(call.owner, call.to, call.amount);
        }

        /// Calls the calling contract's `tokens_to_send` before any of its
        /// tokens move.
        pub fn register_send_hook(&mut self) {
            let caller = abi::caller().expect("CallbackFeeToken: no caller");
            assert!(
                caller != TRANSFER_CONTRACT,
                "CallbackFeeToken: only contracts can register a hook"
            );
            self.send_hooks.insert(caller);
        }

        /// Calls `from`'s hook, if it registered one.
        fn notify_sender(&self, from: Account, to: Account, amount: u64) {
            if let Account::Contract(contract) = from {
                if self.send_hooks.contains(&contract) {
                    let (): () = abi::call(contract, "tokens_to_send", &(from, to, amount))
                        .expect("CallbackFeeToken: send hook failed");
                }
            }
        }

        /// Debits `amount` from `from`, credits `to` with it less the fee,
        /// and burns the fee. Reads the balances afresh, after any hook.
        fn move_tokens(&mut self, from: Account, to: Account, amount: u64) {
            let balance = self.balances.get(&from).copied().unwrap_or(0);
            assert!(balance >= amount, "CallbackFeeToken: insufficient balance");
            self.set_balance(from, balance - amount);
            let fee = u64::try_from(u128::from(amount) * u128::from(self.fee_bps) / BPS)
                .expect("CallbackFeeToken: fee is at most the amount");
            self.credit(to, amount - fee);
            self.total_supply -= fee;
        }

        fn credit(&mut self, account: Account, amount: u64) {
            let balance = self.balances.get(&account).copied().unwrap_or(0);
            let balance = balance
                .checked_add(amount)
                .expect("CallbackFeeToken: balance overflow");
            self.set_balance(account, balance);
        }

        fn set_balance(&mut self, account: Account, balance: u64) {
            if balance == 0 {
                self.balances.remove(&account);
            } else {
                self.balances.insert(account, balance);
            }
        }
    }
}
