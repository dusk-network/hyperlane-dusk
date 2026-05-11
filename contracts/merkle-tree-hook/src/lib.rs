// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane MerkleTreeHook contract for Dusk.

//! Post-dispatch hook that maintains an incremental Merkle tree of message IDs.
//!
//! This is the Dusk equivalent of `MerkleTreeHook.sol`. Every dispatched
//! message's ID is inserted into a depth-32 keccak256 Merkle tree. Validators
//! then attest to the tree root, enabling message verification on the
//! destination chain.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane MerkleTreeHook contract.
#[dusk_forge::contract]
mod merkle_tree_hook {
    extern crate alloc;

    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::merkle::IncrementalMerkle;
    use hyperlane_dusk_types::message;
    use hyperlane_dusk_types::H256;

    /// Zero contract ID.
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// MerkleTreeHook contract state.
    pub struct MerkleTreeHook {
        /// The incremental Merkle tree.
        tree: IncrementalMerkle,
        /// The Mailbox contract that is allowed to trigger post_dispatch.
        mailbox: ContractId,
    }

    impl MerkleTreeHook {
        /// Creates a new empty MerkleTreeHook state.
        pub const fn new() -> Self {
            Self {
                tree: IncrementalMerkle::new(),
                mailbox: ZERO_CONTRACT,
            }
        }

        /// Initialize with the Mailbox contract ID.
        pub fn init(&mut self, mailbox: ContractId) {
            assert!(
                self.mailbox == ZERO_CONTRACT,
                "MerkleTreeHook: already initialized"
            );
            self.mailbox = mailbox;
        }

        // =================================================================
        // IPostDispatchHook interface
        // =================================================================

        /// Called by the Mailbox after a message is dispatched.
        ///
        /// Inserts the message ID into the Merkle tree and emits an event.
        pub fn post_dispatch(&mut self, _metadata: Vec<u8>, encoded_message: Vec<u8>) {
            // Verify caller is the Mailbox.
            let caller = abi::caller().expect("MerkleTreeHook: no caller");
            assert!(
                caller == self.mailbox,
                "MerkleTreeHook: caller is not mailbox"
            );

            let id = message::id(&encoded_message);
            let index = self.tree.count;

            self.tree.insert(id);

            abi::emit(
                events::InsertedIntoTree::TOPIC,
                events::InsertedIntoTree {
                    message_id: id,
                    index,
                },
            );
        }

        /// Returns the cost of using this hook (always 0).
        #[allow(clippy::unused_self)]
        pub fn quote_dispatch(&self, _metadata: Vec<u8>, _message: Vec<u8>) -> u64 {
            0
        }

        /// Returns the hook type identifier.
        #[allow(clippy::unused_self)]
        pub fn hook_type(&self) -> u8 {
            3 // HookType::MerkleTree
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the current Merkle root.
        pub fn root(&self) -> H256 {
            self.tree.root()
        }

        /// Returns the number of leaves inserted.
        pub fn count(&self) -> u32 {
            self.tree.count
        }

        /// Returns the full incremental Merkle tree state (branch + count).
        pub fn tree(&self) -> IncrementalMerkle {
            self.tree.clone()
        }

        /// Returns the latest checkpoint (root, index).
        ///
        /// Returns `(root, index)` where index is `count - 1`.
        /// Panics if the tree is empty.
        pub fn latest_checkpoint(&self) -> (H256, u32) {
            self.tree
                .latest_checkpoint()
                .expect("MerkleTreeHook: tree is empty")
        }

        /// Returns the Mailbox contract ID.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }
    }
}
