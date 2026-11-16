// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane ValidatorAnnounce contract for Dusk.

//! Registry for validator storage locations.
//!
//! Validators use this contract to announce where their signed checkpoints
//! can be found (e.g., an S3 bucket URL). The relayer looks up a validator's
//! storage locations here.
//!
//! This is the Dusk equivalent of `ValidatorAnnounce.sol`, with one
//! difference: the contract keeps the keccak256 hash of each location, not
//! the location. The location itself is in the `ValidatorAnnouncement` event
//! that announced it. An agent reads a validator's locations from those
//! events and checks each against
//! `get_announced_storage_location_hashes(validator)`, which lists the hashes
//! in announcement order.

#![no_std]
#![cfg(target_family = "wasm")]
#![deny(unused_extern_crates)]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::used_underscore_binding)]

/// Hyperlane ValidatorAnnounce contract.
#[dusk_forge::contract(events = [
    events::Initialized,
    events::ValidatorAnnouncement,
])]
mod validator_announce {
    extern crate alloc;

    use alloc::collections::btree_map::{BTreeMap, Entry};
    use alloc::string::String;
    use alloc::vec::Vec;

    use dusk_core::abi::{self, ContractId, CONTRACT_ID_BYTES};

    use hyperlane_dusk_types::events;
    use hyperlane_dusk_types::message::keccak256;
    use hyperlane_dusk_types::storage::ChunkedLog;
    use hyperlane_dusk_types::{signature, EthAddress, H256};

    /// Zero contract ID.
    const ZERO_CONTRACT: ContractId = ContractId::from_bytes([0u8; CONTRACT_ID_BYTES]);

    /// Ethereum Signed Message prefix for 32-byte messages.
    const ETH_SIGNED_MESSAGE_PREFIX: &[u8] = b"\x19Ethereum Signed Message:\n32";
    /// Longest storage location accepted, in bytes.
    const MAX_LOCATION_BYTES: usize = 1_024;
    /// Most locations one validator can announce.
    const MAX_LOCATIONS_PER_VALIDATOR: usize = 16;
    /// Most validators one `get_announced_validators` page returns.
    const MAX_VALIDATOR_PAGE: u32 = 256;

    /// ValidatorAnnounce contract state.
    pub struct ValidatorAnnounce {
        /// The local domain ID.
        local_domain: u32,
        /// The Mailbox contract ID.
        mailbox: ContractId,
        /// Validators in the order of their first announcement.
        validators: ChunkedLog<EthAddress>,
        /// `keccak256(location)` of each location a validator announced,
        /// oldest first, at most [`MAX_LOCATIONS_PER_VALIDATOR`]. A
        /// validator announces a location once, so this is also the replay
        /// protection.
        location_hashes: BTreeMap<[u8; 20], Vec<H256>>,
    }

    impl ValidatorAnnounce {
        /// Creates a new empty ValidatorAnnounce state.
        pub const fn new() -> Self {
            Self {
                local_domain: 0,
                mailbox: ZERO_CONTRACT,
                validators: ChunkedLog::new(),
                location_hashes: BTreeMap::new(),
            }
        }

        /// Initialize with domain and mailbox.
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
        /// over the announcement digest. The contract stores
        /// `keccak256(storage_location)`; the location itself is in the
        /// `ValidatorAnnouncement` event.
        pub fn announce(
            &mut self,
            validator: EthAddress,
            storage_location: String,
            signature: Vec<u8>,
        ) -> bool {
            assert!(
                !storage_location.is_empty(),
                "ValidatorAnnounce: storage location cannot be empty"
            );
            assert!(
                storage_location.len() <= MAX_LOCATION_BYTES,
                "ValidatorAnnounce: storage location too long"
            );
            // Replay protection
            let location_hash = keccak256(storage_location.as_bytes());
            let announced = self.location_hashes.get(&validator.0);
            assert!(
                !announced.is_some_and(|hashes| hashes.contains(&location_hash)),
                "ValidatorAnnounce: replay"
            );

            // Verify signature
            let digest = self.announcement_digest(&storage_location);
            let signer = ecrecover_eth_address(&digest, &signature);
            assert!(
                signer == validator.0,
                "ValidatorAnnounce: invalid signature"
            );

            // Store the location's hash, and the validator on its first
            // announcement. Each list grows one entry at a time: most
            // validators announce once.
            match self.location_hashes.entry(validator.0) {
                Entry::Vacant(entry) => {
                    entry.insert(alloc::vec![location_hash]);
                    self.validators.push(validator);
                }
                Entry::Occupied(entry) => {
                    let hashes = entry.into_mut();
                    assert!(
                        hashes.len() < MAX_LOCATIONS_PER_VALIDATOR,
                        "ValidatorAnnounce: location limit reached"
                    );
                    hashes.reserve_exact(1);
                    hashes.push(location_hash);
                }
            }

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

        /// Returns `keccak256` of each storage location `validator` has
        /// announced, oldest first: at most 16, and none if it never
        /// announced.
        ///
        /// The locations are in the `ValidatorAnnouncement` events that
        /// announced them. A location from an event is the validator's if its
        /// hash is in this list, and the last hash is its latest location.
        pub fn get_announced_storage_location_hashes(&self, validator: EthAddress) -> Vec<H256> {
            self.location_hashes
                .get(&validator.0)
                .cloned()
                .unwrap_or_default()
        }

        /// Returns a page of the validators that have announced, in the order
        /// of their first announcement: at most `limit` (1 to 256), starting
        /// at index `start`. A `start` at or past the count returns an empty
        /// page.
        pub fn get_announced_validators(&self, start: u32, limit: u32) -> Vec<EthAddress> {
            assert!(limit > 0, "ValidatorAnnounce: query limit is zero");
            assert!(
                limit <= MAX_VALIDATOR_PAGE,
                "ValidatorAnnounce: query batch too large"
            );
            self.validators.page(start, limit)
        }

        /// Returns the number of validators that have announced.
        pub fn announced_validator_count(&self) -> u32 {
            self.validators.len()
        }

        /// Returns the Mailbox contract ID.
        pub fn mailbox(&self) -> ContractId {
            self.mailbox
        }

        /// Returns the local domain ID.
        pub fn local_domain(&self) -> u32 {
            self.local_domain
        }

        /// Returns the persisted state layout version expected by deployment tooling.
        ///
        /// Version 2 stores location hashes instead of locations: it replaces
        /// `get_announced_storage_locations` and
        /// `get_announced_storage_locations_for_validator` with
        /// `get_announced_storage_location_hashes`, and pages of
        /// `get_announced_validators` hold up to 256 validators.
        #[allow(clippy::unused_self)]
        pub fn state_version(&self) -> u32 {
            2
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
            let mut inner_preimage = Vec::with_capacity(32 + storage_location.len());
            inner_preimage.extend_from_slice(&domain_hash);
            inner_preimage.extend_from_slice(storage_location.as_bytes());
            let inner_hash = keccak256(&inner_preimage);

            // Ethereum signed message
            let mut eth_preimage = Vec::with_capacity(28 + 32);
            eth_preimage.extend_from_slice(ETH_SIGNED_MESSAGE_PREFIX);
            eth_preimage.extend_from_slice(&inner_hash);
            keccak256(&eth_preimage)
        }
    }

    /// Recover the 20-byte Ethereum address from an ECDSA signature.
    fn ecrecover_eth_address(digest: &[u8; 32], sig: &[u8]) -> [u8; 20] {
        assert!(sig.len() == 65, "ValidatorAnnounce: bad sig length");

        let mut sig_arr = [0u8; 65];
        sig_arr.copy_from_slice(sig);
        // Accept exactly what OpenZeppelin's ECDSA.recover accepts: the host
        // query would also take v of 0/1 and high-s twins.
        assert!(
            signature::is_canonical(&sig_arr),
            "ValidatorAnnounce: non-canonical signature"
        );

        let pubkey =
            abi::secp256k1_recover(*digest, sig_arr).expect("ValidatorAnnounce: ecrecover failed");

        let hash = keccak256(&pubkey[1..]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..32]);
        addr
    }
}
