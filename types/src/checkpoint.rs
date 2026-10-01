// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane checkpoint digest computation for Dusk.

//! Checkpoint digest computation matching `CheckpointLib.sol`.
//!
//! Validators sign checkpoints using Ethereum's `eth_sign` personal message
//! scheme. This module provides the digest computation so it can be verified
//! both on-chain (by the `MultisigISM`) and off-chain (for testing).

use alloc::vec::Vec;

use crate::message::keccak256;

/// Ethereum Signed Message prefix for 32-byte messages.
///
/// This matches `ECDSA.toEthSignedMessageHash()` in `OpenZeppelin`.
const ETH_SIGNED_MESSAGE_PREFIX: &[u8] = b"\x19Ethereum Signed Message:\n32";

/// Compute the domain hash for a given origin and merkle tree hook.
///
/// `domain_hash = keccak256(origin || merkle_tree_hook || "HYPERLANE")`
///
/// This matches `CheckpointLib.domainHash()` in Solidity.
#[must_use]
pub fn domain_hash(origin: u32, merkle_tree_hook: &[u8; 32]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(4 + 32 + 9);
    preimage.extend_from_slice(&origin.to_be_bytes());
    preimage.extend_from_slice(merkle_tree_hook);
    preimage.extend_from_slice(b"HYPERLANE");
    keccak256(&preimage)
}

/// Compute the checkpoint digest that validators sign.
///
/// 1. `domain_hash = keccak256(origin || merkle_tree_hook || "HYPERLANE")`
/// 2. `checkpoint_hash = keccak256(domain_hash || root || index || message_id)`
/// 3. `digest = keccak256("\x19Ethereum Signed Message:\n32" || checkpoint_hash)`
///
/// This matches `CheckpointLib.digest()` + `ECDSA.toEthSignedMessageHash()`.
#[must_use]
pub fn checkpoint_digest(
    origin: u32,
    merkle_tree_hook: &[u8; 32],
    root: &[u8; 32],
    index: u32,
    message_id: &[u8; 32],
) -> [u8; 32] {
    let dh = domain_hash(origin, merkle_tree_hook);

    let mut checkpoint_preimage = Vec::with_capacity(32 + 32 + 4 + 32);
    checkpoint_preimage.extend_from_slice(&dh);
    checkpoint_preimage.extend_from_slice(root);
    checkpoint_preimage.extend_from_slice(&index.to_be_bytes());
    checkpoint_preimage.extend_from_slice(message_id);
    let checkpoint_hash = keccak256(&checkpoint_preimage);

    eth_signed_message_hash(&checkpoint_hash)
}

/// Compute the Ethereum signed message hash.
///
/// `keccak256("\x19Ethereum Signed Message:\n32" || hash)`
///
/// This matches `ECDSA.toEthSignedMessageHash()` in `OpenZeppelin`.
#[must_use]
pub fn eth_signed_message_hash(hash: &[u8; 32]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(28 + 32);
    preimage.extend_from_slice(ETH_SIGNED_MESSAGE_PREFIX);
    preimage.extend_from_slice(hash);
    keccak256(&preimage)
}

/// Compute the validator announcement domain hash.
///
/// `keccak256(local_domain || mailbox || "HYPERLANE_ANNOUNCEMENT")`
///
/// This matches `ValidatorAnnounce._domainHash()` in Solidity.
#[must_use]
pub fn announcement_domain_hash(local_domain: u32, mailbox: &[u8; 32]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(4 + 32 + 22);
    preimage.extend_from_slice(&local_domain.to_be_bytes());
    preimage.extend_from_slice(mailbox);
    preimage.extend_from_slice(b"HYPERLANE_ANNOUNCEMENT");
    keccak256(&preimage)
}

/// Compute the announcement digest.
///
/// 1. `domain_hash = keccak256(local_domain || mailbox || "HYPERLANE_ANNOUNCEMENT")`
/// 2. `inner = keccak256(domain_hash || storage_location)`
/// 3. `digest = keccak256("\x19Ethereum Signed Message:\n32" || inner)`
#[must_use]
pub fn announcement_digest(
    local_domain: u32,
    mailbox: &[u8; 32],
    storage_location: &str,
) -> [u8; 32] {
    let dh = announcement_domain_hash(local_domain, mailbox);

    let mut inner_preimage = Vec::with_capacity(32 + storage_location.len());
    inner_preimage.extend_from_slice(&dh);
    inner_preimage.extend_from_slice(storage_location.as_bytes());
    let inner = keccak256(&inner_preimage);

    eth_signed_message_hash(&inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_hash() {
        // Test that domain_hash matches the Solidity computation.
        // domain_hash = keccak256(abi.encodePacked(origin, merkleTreeHook, "HYPERLANE"))
        let origin: u32 = 1;
        let hook = [0xAA; 32];
        let dh = domain_hash(origin, &hook);

        // Manually compute what Solidity would produce:
        // abi.encodePacked(uint32(1), bytes32(0xAA..AA), "HYPERLANE")
        let mut expected_preimage = Vec::new();
        expected_preimage.extend_from_slice(&1u32.to_be_bytes());
        expected_preimage.extend_from_slice(&[0xAA; 32]);
        expected_preimage.extend_from_slice(b"HYPERLANE");
        let expected = keccak256(&expected_preimage);

        assert_eq!(dh, expected);
    }

    #[test]
    fn test_eth_signed_message_hash() {
        // keccak256("\x19Ethereum Signed Message:\n32" || hash)
        let hash = keccak256(b"test");
        let signed = eth_signed_message_hash(&hash);

        // The result should be different from the input.
        assert_ne!(signed, hash);

        // Recomputing should give the same result.
        assert_eq!(signed, eth_signed_message_hash(&hash));
    }

    #[test]
    fn test_checkpoint_digest_deterministic() {
        let origin = 11155111u32; // Sepolia
        let hook = [0x11; 32];
        let root = [0x22; 32];
        let index = 42u32;
        let message_id = [0x33; 32];

        let digest1 = checkpoint_digest(origin, &hook, &root, index, &message_id);
        let digest2 = checkpoint_digest(origin, &hook, &root, index, &message_id);
        assert_eq!(digest1, digest2);

        // Different parameters should produce different digests.
        let digest3 = checkpoint_digest(origin + 1, &hook, &root, index, &message_id);
        assert_ne!(digest1, digest3);
    }

    #[test]
    fn test_announcement_digest_deterministic() {
        let domain = 1u32;
        let mailbox = [0x44; 32];
        let location = "s3://hyperlane-testnet/us-east-1/validator-0";

        let d1 = announcement_digest(domain, &mailbox, location);
        let d2 = announcement_digest(domain, &mailbox, location);
        assert_eq!(d1, d2);

        let d3 = announcement_digest(domain, &mailbox, "s3://other-bucket");
        assert_ne!(d1, d3);
    }

    #[test]
    fn test_message_encoding_matches_solidity() {
        // Verify our encoding produces the same bytes as Solidity's
        // abi.encodePacked(version, nonce, origin, sender, destination, recipient, body).
        //
        // version = 3 (uint8)
        // nonce = 0 (uint32)
        // origin = 1 (uint32)
        // sender = 0x00..01 (bytes32)
        // destination = 2 (uint32)
        // recipient = 0x00..02 (bytes32)
        // body = 0xDEAD (bytes)

        use crate::message;

        let mut sender = [0u8; 32];
        sender[31] = 1;
        let mut recipient = [0u8; 32];
        recipient[31] = 2;

        let encoded = message::encode(3, 0, 1, sender, 2, recipient, &[0xDE, 0xAD]);

        // Total length = 1 + 4 + 4 + 32 + 4 + 32 + 2 = 79
        assert_eq!(encoded.len(), 79);

        // Check version (offset 0, 1 byte)
        assert_eq!(encoded[0], 3);

        // Check nonce (offset 1, 4 bytes big-endian)
        assert_eq!(&encoded[1..5], &[0, 0, 0, 0]);

        // Check origin (offset 5, 4 bytes big-endian)
        assert_eq!(&encoded[5..9], &[0, 0, 0, 1]);

        // Check sender (offset 9, 32 bytes)
        assert_eq!(&encoded[9..41], &sender);

        // Check destination (offset 41, 4 bytes big-endian)
        assert_eq!(&encoded[41..45], &[0, 0, 0, 2]);

        // Check recipient (offset 45, 32 bytes)
        assert_eq!(&encoded[45..77], &recipient);

        // Check body (offset 77)
        assert_eq!(&encoded[77..], &[0xDE, 0xAD]);
    }
}
