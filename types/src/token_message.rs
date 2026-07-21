// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane TokenMessage encoding for Dusk warp routes.

//! Encodes and decodes the Hyperlane `TokenMessage` format used as the
//! message body in warp route transfers.
//!
//! Layout (EVM-compatible):
//! ```text
//! [0..32]   recipient (H256 — 32 bytes)
//! [32..64]  amount (uint256 — 32 bytes, big-endian; only bottom 8 bytes used)
//! [64..]    metadata (variable length, optional)
//! ```
//!
//! This matches the Solidity `TokenMessage` library exactly. The amount
//! is encoded as a 32-byte uint256 for wire compatibility with EVM, but
//! Dusk uses `u64` internally (values above `u64::MAX` are unsupported).

use alloc::vec::Vec;

use crate::H256;

/// Minimum encoded length: 32 (recipient) + 32 (amount as uint256).
const MIN_LEN: usize = 64;

/// Encode a token message body (EVM-compatible uint256 amount).
#[must_use]
pub fn encode(recipient: H256, amount: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(MIN_LEN);
    buf.extend_from_slice(&recipient);
    // Pad u64 to 32 bytes (uint256 big-endian): 24 zero bytes + 8 amount bytes
    let mut amount_bytes = [0u8; 32];
    amount_bytes[24..32].copy_from_slice(&amount.to_be_bytes());
    buf.extend_from_slice(&amount_bytes);
    buf
}

/// Encode a token message body with metadata.
#[must_use]
pub fn encode_with_metadata(recipient: H256, amount: u64, metadata: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(MIN_LEN + metadata.len());
    buf.extend_from_slice(&recipient);
    let mut amount_bytes = [0u8; 32];
    amount_bytes[24..32].copy_from_slice(&amount.to_be_bytes());
    buf.extend_from_slice(&amount_bytes);
    buf.extend_from_slice(metadata);
    buf
}

/// Decoded token message fields.
pub struct TokenMessage<'a> {
    /// The recipient address (H256).
    pub recipient: H256,
    /// The transfer amount.
    pub amount: u64,
    /// Optional trailing metadata.
    pub metadata: &'a [u8],
}

/// Decode a token message body.
///
/// Returns `None` if the body is too short or the uint256 amount does not fit
/// in Dusk's `u64` token representation.
#[must_use]
pub fn decode(body: &[u8]) -> Option<TokenMessage<'_>> {
    if body.len() < MIN_LEN {
        return None;
    }

    let mut recipient = [0u8; 32];
    recipient.copy_from_slice(&body[..32]);

    // Reject rather than truncate remote uint256 amounts above u64::MAX.
    if body[32..56].iter().any(|byte| *byte != 0) {
        return None;
    }

    // Read last 8 bytes of the 32-byte uint256 amount field as u64.
    let amount = u64::from_be_bytes(body[56..64].try_into().ok()?);
    let metadata = &body[64..];

    Some(TokenMessage {
        recipient,
        amount,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let recipient = [0xAB; 32];
        let amount = 1_000_000u64;
        let encoded = encode(recipient, amount);

        assert_eq!(encoded.len(), 64);

        let msg = decode(&encoded).expect("decode should succeed");
        assert_eq!(msg.recipient, recipient);
        assert_eq!(msg.amount, amount);
        assert!(msg.metadata.is_empty());
    }

    #[test]
    fn roundtrip_with_metadata() {
        let recipient = [0xCD; 32];
        let amount = 42u64;
        let meta = b"extra data";
        let encoded = encode_with_metadata(recipient, amount, meta);

        assert_eq!(encoded.len(), 64 + meta.len());

        let msg = decode(&encoded).expect("decode should succeed");
        assert_eq!(msg.recipient, recipient);
        assert_eq!(msg.amount, amount);
        assert_eq!(msg.metadata, meta);
    }

    #[test]
    fn decode_too_short() {
        assert!(decode(&[0u8; 63]).is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn decode_rejects_amount_above_u64() {
        let mut encoded = encode([0; 32], 1);
        encoded[55] = 1;

        assert!(decode(&encoded).is_none());
    }

    #[test]
    fn amount_uint256_encoding() {
        let encoded = encode([0; 32], 0x0102030405060708u64);
        // First 24 bytes of amount field should be zero (uint256 padding)
        assert_eq!(&encoded[32..56], &[0u8; 24]);
        // Last 8 bytes should be the u64 in big-endian
        assert_eq!(
            &encoded[56..64],
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );
    }

    #[test]
    fn evm_compatible_format() {
        // Verify that the encoding matches EVM's abi.encodePacked(bytes32, uint256)
        let recipient = [0xAA; 32];
        let amount = 1_000_000_000_000_000_000u64; // 1e18
        let encoded = encode(recipient, amount);

        assert_eq!(encoded.len(), 64); // 32 + 32 = 64, same as EVM
        assert_eq!(&encoded[..32], &recipient);

        // First 24 bytes of amount field are zero padding
        assert_eq!(&encoded[32..56], &[0u8; 24]);
        // Last 8 bytes are the u64 amount
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.amount, amount);
    }
}
