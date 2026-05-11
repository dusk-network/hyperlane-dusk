// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane ValidatorAnnounce contract for Dusk.

//! Registry for validator storage locations.
//!
//! Validators use this contract to announce where their signed checkpoints
//! can be found (e.g., an S3 bucket URL). The relayer queries this contract
//! to discover validator checkpoint storage.
//!
//! This is the Dusk equivalent of `ValidatorAnnounce.sol`.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane ValidatorAnnounce contract.
#[dusk_forge::contract]
mod validator_announce {
    extern crate alloc;

    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message::keccak256;
    use hyperlane_dusk_types::EthAddress;

    /// Zero contract ID.
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Ethereum Signed Message prefix for 32-byte messages.
    const ETH_SIGNED_MESSAGE_PREFIX: &[u8] = b"\x19Ethereum Signed Message:\n32";

    /// ValidatorAnnounce contract state.
    pub struct ValidatorAnnounce {
        /// The local domain ID.
        local_domain: u32,
        /// The Mailbox contract ID.
        mailbox: ContractId,
        /// Set of validators that have announced.
        validators: Vec<EthAddress>,
        /// Storage locations per validator.
        storage_locations: BTreeMap<[u8; 20], Vec<String>>,
        /// Replay protection: tracks already-announced (validator, location) pairs.
        replay_protection: BTreeMap<[u8; 32], bool>,
    }

    impl ValidatorAnnounce {
        /// Creates a new empty ValidatorAnnounce state.
        pub const fn new() -> Self {
            Self {
                local_domain: 0,
                mailbox: ZERO_CONTRACT,
                validators: Vec::new(),
                storage_locations: BTreeMap::new(),
                replay_protection: BTreeMap::new(),
            }
        }

        /// Initialize with domain and mailbox.
        #[contract(emits = [(events::Initialized::TOPIC, events::Initialized)])]
        pub fn init(&mut self, local_domain: u32, mailbox: ContractId) {
            assert!(
                self.mailbox == ZERO_CONTRACT,
                "ValidatorAnnounce: already initialized"
            );
            self.local_domain = local_domain;
            self.mailbox = mailbox;
            abi::emit(
                events::Initialized::TOPIC,
                events::Initialized {
                    contract_type: events::CONTRACT_VALIDATOR_ANNOUNCE,
                    owner: ZERO_CONTRACT.to_bytes(),
                    mailbox: mailbox.to_bytes(),
                    local_domain,
                },
            );
        }

        // =================================================================
        // Core
        // =================================================================

        /// Announce a validator's storage location.
        ///
        /// The signature must be a valid ECDSA signature by the validator
        /// over the announcement digest.
        #[contract(emits = [(events::ValidatorAnnouncement::TOPIC, events::ValidatorAnnouncement)])]
        pub fn announce(
            &mut self,
            validator: EthAddress,
            storage_location: String,
            signature: Vec<u8>,
        ) -> bool {
            // Replay protection
            let replay_id = Self::compute_replay_id(&validator, &storage_location);
            assert!(
                !self.replay_protection.contains_key(&replay_id),
                "ValidatorAnnounce: replay"
            );
            self.replay_protection.insert(replay_id, true);

            // Verify signature
            let digest = self.announcement_digest(&storage_location);
            let signer = ecrecover_eth_address(&digest, &signature);
            assert!(
                signer == validator.0,
                "ValidatorAnnounce: invalid signature"
            );

            // Register validator if first announcement.
            if !self.storage_locations.contains_key(&validator.0) {
                self.validators.push(validator);
            }

            // Store location.
            self.storage_locations
                .entry(validator.0)
                .or_default()
                .push(storage_location.clone());

            abi::emit(
                events::ValidatorAnnouncement::TOPIC,
                events::ValidatorAnnouncement {
                    validator,
                    storage_location,
                },
            );

            true
        }

        // =================================================================
        // Queries
        // =================================================================

        /// Returns the storage locations for a set of validators.
        pub fn get_announced_storage_locations(
            &self,
            validators: Vec<EthAddress>,
        ) -> Vec<Vec<String>> {
            validators
                .iter()
                .map(|v| {
                    self.storage_locations
                        .get(&v.0)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect()
        }

        /// Returns all validators that have announced.
        pub fn get_announced_validators(&self) -> Vec<EthAddress> {
            self.validators.clone()
        }

        /// Returns the Mailbox contract ID.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }

        /// Returns the local domain ID.
        pub fn local_domain(&self) -> u32 {
            self.local_domain
        }

        // =================================================================
        // Internal helpers
        // =================================================================

        /// Compute the announcement digest.
        ///
        /// Matches the Solidity `getAnnouncementDigest`:
        /// 1. `domain_hash = keccak256(local_domain || mailbox || "HYPERLANE_ANNOUNCEMENT")`
        /// 2. `digest = keccak256("\x19Ethereum Signed Message:\n32" || keccak256(domain_hash || storage_location))`
        fn announcement_digest(&self, storage_location: &str) -> [u8; 32] {
            // Domain hash
            let mut domain_preimage = Vec::with_capacity(4 + 32 + 22);
            domain_preimage.extend_from_slice(&self.local_domain.to_be_bytes());
            domain_preimage.extend_from_slice(&self.mailbox.to_bytes());
            domain_preimage.extend_from_slice(b"HYPERLANE_ANNOUNCEMENT");
            let domain_hash = keccak256(&domain_preimage);

            // Inner hash
            let mut inner_preimage =
                Vec::with_capacity(32 + storage_location.len());
            inner_preimage.extend_from_slice(&domain_hash);
            inner_preimage.extend_from_slice(storage_location.as_bytes());
            let inner_hash = keccak256(&inner_preimage);

            // Ethereum signed message
            let mut eth_preimage = Vec::with_capacity(28 + 32);
            eth_preimage.extend_from_slice(ETH_SIGNED_MESSAGE_PREFIX);
            eth_preimage.extend_from_slice(&inner_hash);
            keccak256(&eth_preimage)
        }

        /// Compute replay ID for a (validator, location) pair.
        fn compute_replay_id(
            validator: &EthAddress,
            storage_location: &str,
        ) -> [u8; 32] {
            let mut preimage = Vec::with_capacity(20 + storage_location.len());
            preimage.extend_from_slice(&validator.0);
            preimage.extend_from_slice(storage_location.as_bytes());
            keccak256(&preimage)
        }
    }

    /// Recover the 20-byte Ethereum address from an ECDSA signature.
    fn ecrecover_eth_address(digest: &[u8; 32], sig: &[u8]) -> [u8; 20] {
        assert!(sig.len() == 65, "ValidatorAnnounce: bad sig length");

        let mut sig_arr = [0u8; 65];
        sig_arr.copy_from_slice(sig);

        let pubkey = abi::secp256k1_recover(*digest, sig_arr)
            .expect("ValidatorAnnounce: ecrecover failed");

        let hash = keccak256(&pubkey[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..32]);
        addr
    }
}
