// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane MessageIdMultisigISM contract for Dusk.

//! Interchain Security Module that verifies m-of-n ECDSA validator signatures.
//!
//! This is the Dusk equivalent of `MessageIdMultisigIsm.sol`. It verifies
//! that a threshold of registered validators have signed a checkpoint
//! corresponding to the message being delivered.
//!
//! ## Metadata format
//!
//! The metadata passed to `verify` must be:
//! ```text
//! [  0: 32] origin merkle tree hook address (bytes32)
//! [ 32: 64] signed checkpoint root (bytes32)
//! [ 64: 68] signed checkpoint index (uint32)
//! [ 68:...] validator signatures (65 bytes each: r(32) || s(32) || v(1))
//! ```
//!
//! ## Digest computation
//!
//! The signed digest is computed as:
//! 1. `domain_hash = keccak256(origin || merkle_tree_hook || "HYPERLANE")`
//! 2. `checkpoint_hash = keccak256(domain_hash || root || index || message_id)`
//! 3. `digest = keccak256("\x19Ethereum Signed Message:\n32" || checkpoint_hash)`

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_possible_truncation)]

/// Hyperlane MessageIdMultisigISM contract.
#[dusk_forge::contract(events = [
    events::Initialized,
    events::OwnershipTransferred,
    events::ValidatorsAndThresholdSet,
])]
mod ism_multisig {
    extern crate alloc;

    use alloc::vec::Vec;

    use dusk_core::abi;

    use hyperlane_dusk_types::caller;
    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message::{self, keccak256};
    use hyperlane_dusk_types::{EthAddress, H256};

    /// Structural upper bound implied by the u8 threshold ABI.
    const MAX_VALIDATORS: usize = u8::MAX as usize;

    // Metadata offsets matching MessageIdMultisigIsmMetadata.sol
    const MERKLE_TREE_HOOK_OFFSET: usize = 0;
    const ROOT_OFFSET: usize = 32;
    const INDEX_OFFSET: usize = 64;
    const SIGNATURES_OFFSET: usize = 68;
    const SIGNATURE_LENGTH: usize = 65;

    /// Ethereum Signed Message prefix for 32-byte messages.
    const ETH_SIGNED_MESSAGE_PREFIX: &[u8] = b"\x19Ethereum Signed Message:\n32";

    /// MultisigISM contract state.
    pub struct MultisigIsm {
        /// Registered validators (sorted by Ethereum address).
        validators: Vec<EthAddress>,
        /// Number of required signatures.
        threshold: u8,
        /// Contract owner.
        owner: Option<H256>,
    }

    impl MultisigIsm {
        /// Creates a new empty MultisigISM state.
        pub const fn new() -> Self {
            Self {
                validators: Vec::new(),
                threshold: 0,
                owner: None,
            }
        }

        /// Initialize the ISM with validators and threshold.
        ///
        /// Validators must be sorted by address (ascending). The threshold
        /// must be > 0 and <= number of validators.
        pub fn init(&mut self, owner: H256, validators: Vec<EthAddress>, threshold: u8) {
            assert!(self.owner.is_none(), "MultisigISM: already initialized");
            assert!(owner != [0u8; 32], "MultisigISM: owner cannot be zero");
            assert!(!validators.is_empty(), "MultisigISM: no validators");
            assert!(
                validators.len() <= MAX_VALIDATORS,
                "MultisigISM: too many validators"
            );
            assert!(
                threshold > 0 && threshold as usize <= validators.len(),
                "MultisigISM: invalid threshold"
            );

            // Verify validators are sorted.
            for i in 1..validators.len() {
                assert!(
                    validators[i - 1].0 < validators[i].0,
                    "MultisigISM: validators not sorted"
                );
            }

            self.owner = Some(owner);
            self.validators = validators;
            self.threshold = threshold;
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_ISM_MULTISIG,
                    owner,
                    mailbox: [0u8; 32],
                    local_domain: 0,
                },
            );
            abi::emit(
                events::ValidatorsAndThresholdSet::TOPIC,
                events::ValidatorsAndThresholdSet {
                    validators: self.validators.clone(),
                    threshold,
                },
            );
        }

        // =================================================================
        // IInterchainSecurityModule interface
        // =================================================================

        /// Verify that a message has been signed by a threshold of validators.
        ///
        /// Returns `true` if verification succeeds.
        #[allow(clippy::unused_self)]
        pub fn verify(&self, metadata: Vec<u8>, encoded_message: Vec<u8>) -> bool {
            assert!(
                self.threshold > 0 && !self.validators.is_empty(),
                "MultisigISM: not initialized"
            );
            assert!(
                metadata.len() >= SIGNATURES_OFFSET,
                "MultisigISM: metadata too short"
            );
            assert!(
                (metadata.len() - SIGNATURES_OFFSET).is_multiple_of(SIGNATURE_LENGTH),
                "MultisigISM: metadata signature length mismatch"
            );

            let sig_count = (metadata.len() - SIGNATURES_OFFSET) / SIGNATURE_LENGTH;
            assert!(
                sig_count >= self.threshold as usize,
                "MultisigISM: not enough signatures"
            );

            // Parse metadata fields.
            let merkle_tree_hook = &metadata[MERKLE_TREE_HOOK_OFFSET..ROOT_OFFSET];
            let root = &metadata[ROOT_OFFSET..INDEX_OFFSET];
            let index = {
                let mut buf = [0u8; 4];
                buf.copy_from_slice(&metadata[INDEX_OFFSET..SIGNATURES_OFFSET]);
                u32::from_be_bytes(buf)
            };

            // Compute the message ID.
            let message_id = message::id(&encoded_message);
            let origin = message::origin(&encoded_message);

            // Compute the digest that validators signed.
            let digest = compute_digest(origin, merkle_tree_hook, root, index, &message_id);

            // Verify threshold signatures using sorted two-pointer matching.
            let mut validator_index = 0usize;
            let validator_count = self.validators.len();

            for i in 0..self.threshold as usize {
                let sig_start = SIGNATURES_OFFSET + i * SIGNATURE_LENGTH;
                let sig_end = sig_start + SIGNATURE_LENGTH;
                let sig_bytes: &[u8] = &metadata[sig_start..sig_end];

                // Recover the signer's Ethereum address.
                let signer = ecrecover_eth_address(&digest, sig_bytes);

                // Two-pointer: find signer in sorted validator list.
                while validator_index < validator_count
                    && self.validators[validator_index].0 != signer
                {
                    validator_index += 1;
                }

                assert!(
                    validator_index < validator_count,
                    "MultisigISM: insufficient valid signatures"
                );
                validator_index += 1;
            }

            true
        }

        /// Returns the ISM module type.
        #[allow(clippy::unused_self)]
        pub fn module_type(&self) -> u8 {
            5 // IsmType::MessageIdMultisig
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the registered validators.
        pub fn validators(&self) -> Vec<EthAddress> {
            self.validators.clone()
        }

        /// Returns the required threshold.
        pub fn threshold(&self) -> u8 {
            self.threshold
        }

        /// Returns one coherent validator configuration snapshot.
        ///
        /// Agents must prefer this over separate `validators` and `threshold`
        /// queries so an owner update cannot be observed half-applied across
        /// two independent RUES requests.
        pub fn validators_and_threshold(&self) -> (Vec<EthAddress>, u8) {
            (self.validators.clone(), self.threshold)
        }

        /// Returns the owner identity.
        pub fn owner(&self) -> Option<H256> {
            self.owner
        }

        // =================================================================
        // Admin
        // =================================================================

        /// Update validators and threshold. Owner only.
        pub fn set_validators_and_threshold(&mut self, validators: Vec<EthAddress>, threshold: u8) {
            self.only_owner();

            assert!(!validators.is_empty(), "MultisigISM: no validators");
            assert!(
                validators.len() <= MAX_VALIDATORS,
                "MultisigISM: too many validators"
            );
            assert!(
                threshold > 0 && threshold as usize <= validators.len(),
                "MultisigISM: invalid threshold"
            );

            for i in 1..validators.len() {
                assert!(
                    validators[i - 1].0 < validators[i].0,
                    "MultisigISM: validators not sorted"
                );
            }

            self.validators = validators;
            self.threshold = threshold;
            abi::emit(
                events::ValidatorsAndThresholdSet::TOPIC,
                events::ValidatorsAndThresholdSet {
                    validators: self.validators.clone(),
                    threshold,
                },
            );
        }

        /// Transfer ownership. Owner only.
        pub fn transfer_ownership(&mut self, new_owner: H256) {
            self.only_owner();
            assert!(
                new_owner != [0u8; 32],
                "MultisigISM: new owner cannot be zero"
            );
            let previous_owner = self.owner.expect("MultisigISM: no owner");
            self.owner = Some(new_owner);
            abi::emit(
                events::OwnershipTransferred::TOPIC,
                events::OwnershipTransferred {
                    previous_owner,
                    new_owner,
                },
            );
        }

        /// Panics if the caller is not the owner.
        fn only_owner(&self) {
            let owner = self.owner.expect("MultisigISM: no owner");
            assert!(
                caller::effective_caller() == owner,
                "MultisigISM: caller is not owner"
            );
        }
    }

    // =====================================================================
    // Crypto helpers
    // =====================================================================

    /// Compute the digest that validators sign.
    ///
    /// Matches `CheckpointLib.digest()` in Solidity:
    /// 1. `domain_hash = keccak256(origin || merkle_tree_hook || "HYPERLANE")`
    /// 2. `checkpoint_hash = keccak256(domain_hash || root || index || message_id)`
    /// 3. `digest = keccak256("\x19Ethereum Signed Message:\n32" || checkpoint_hash)`
    fn compute_digest(
        origin: u32,
        merkle_tree_hook: &[u8],
        root: &[u8],
        index: u32,
        message_id: &[u8; 32],
    ) -> [u8; 32] {
        // domain_hash = keccak256(origin || merkle_tree_hook || "HYPERLANE")
        let mut domain_preimage = Vec::with_capacity(4 + 32 + 9);
        domain_preimage.extend_from_slice(&origin.to_be_bytes());
        domain_preimage.extend_from_slice(merkle_tree_hook);
        domain_preimage.extend_from_slice(b"HYPERLANE");
        let domain_hash = keccak256(&domain_preimage);

        // checkpoint_hash = keccak256(domain_hash || root || index || message_id)
        let mut checkpoint_preimage = Vec::with_capacity(32 + 32 + 4 + 32);
        checkpoint_preimage.extend_from_slice(&domain_hash);
        checkpoint_preimage.extend_from_slice(root);
        checkpoint_preimage.extend_from_slice(&index.to_be_bytes());
        checkpoint_preimage.extend_from_slice(message_id);
        let checkpoint_hash = keccak256(&checkpoint_preimage);

        // Ethereum signed message hash
        let mut eth_preimage = Vec::with_capacity(28 + 32);
        eth_preimage.extend_from_slice(ETH_SIGNED_MESSAGE_PREFIX);
        eth_preimage.extend_from_slice(&checkpoint_hash);
        keccak256(&eth_preimage)
    }

    /// Recover the 20-byte Ethereum address from an ECDSA signature.
    ///
    /// Uses the Dusk VM's `secp256k1_recover` host function.
    /// Signature format: `r(32) || s(32) || v(1)` where v is 0, 1, 27, or 28.
    fn ecrecover_eth_address(digest: &[u8; 32], sig: &[u8]) -> [u8; 20] {
        assert!(sig.len() == SIGNATURE_LENGTH, "MultisigISM: bad sig length");

        let mut sig_arr = [0u8; 65];
        sig_arr.copy_from_slice(sig);

        // Recover the uncompressed public key (65 bytes: 0x04 || x || y).
        let pubkey =
            abi::secp256k1_recover(*digest, sig_arr).expect("MultisigISM: ecrecover failed");

        // Derive Ethereum address: keccak256(pubkey[1..])[12..32]
        let hash = keccak256(&pubkey[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..32]);
        addr
    }
}
