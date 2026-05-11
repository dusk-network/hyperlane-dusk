// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane message encoding for Dusk.

//! Hyperlane message encoding and decoding.
//!
//! The wire format matches the Solidity `Message.sol` library exactly:
//! ```text
//! | version (1) | nonce (4) | origin (4) | sender (32) | destination (4) | recipient (32) | body (var) |
//! ```
//!
//! Offsets:
//! - version: 0
//! - nonce: 1
//! - origin: 5
//! - sender: 9
//! - destination: 41
//! - recipient: 45
//! - body: 77

use alloc::vec::Vec;

use bytecheck::CheckBytes;
use rkyv::{Archive, Deserialize, Serialize};

use crate::{MessageId, H256, VERSION};

// Wire format offsets matching Message.sol
const VERSION_OFFSET: usize = 0;
const NONCE_OFFSET: usize = 1;
const ORIGIN_OFFSET: usize = 5;
const SENDER_OFFSET: usize = 9;
const DESTINATION_OFFSET: usize = 41;
const RECIPIENT_OFFSET: usize = 45;
const BODY_OFFSET: usize = 77;

/// Minimum length of an encoded Hyperlane message (header only, no body).
pub const MIN_MESSAGE_LENGTH: usize = BODY_OFFSET;

/// A decoded Hyperlane message.
///
/// This struct is used for in-contract manipulation. For wire encoding,
/// use [`encode`] and for decoding use [`decode`] or the field accessor
/// functions on raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HyperlaneMessage {
    /// Protocol version.
    pub version: u8,
    /// Unique nonce on the origin chain.
    pub nonce: u32,
    /// Origin chain domain ID.
    pub origin: u32,
    /// Sender address (32 bytes, left-padded for EVM).
    pub sender: H256,
    /// Destination chain domain ID.
    pub destination: u32,
    /// Recipient address (32 bytes).
    pub recipient: H256,
    /// Message body.
    pub body: Vec<u8>,
}

impl HyperlaneMessage {
    /// Encode this message to the packed wire format.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        encode(
            self.version,
            self.nonce,
            self.origin,
            self.sender,
            self.destination,
            self.recipient,
            &self.body,
        )
    }

    /// Compute the message ID (keccak256 of the encoded message).
    ///
    /// Uses the keccak256 host function when running on-chain.
    #[must_use]
    pub fn id(&self) -> MessageId {
        let encoded = self.encode();
        keccak256(&encoded)
    }
}

/// Encode a Hyperlane message to the packed wire format matching `Message.sol`.
#[must_use]
pub fn encode(
    version: u8,
    nonce: u32,
    origin: u32,
    sender: H256,
    destination: u32,
    recipient: H256,
    body: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(BODY_OFFSET + body.len());
    buf.push(version);
    buf.extend_from_slice(&nonce.to_be_bytes());
    buf.extend_from_slice(&origin.to_be_bytes());
    buf.extend_from_slice(&sender);
    buf.extend_from_slice(&destination.to_be_bytes());
    buf.extend_from_slice(&recipient);
    buf.extend_from_slice(body);
    buf
}

/// Decode a Hyperlane message from raw bytes.
///
/// Returns `None` if the bytes are too short or the version doesn't match.
#[must_use]
pub fn decode(data: &[u8]) -> Option<HyperlaneMessage> {
    if data.len() < MIN_MESSAGE_LENGTH {
        return None;
    }

    let version = data[VERSION_OFFSET];
    if version != VERSION {
        return None;
    }

    Some(HyperlaneMessage {
        version,
        nonce: nonce(data),
        origin: origin(data),
        sender: sender(data),
        destination: destination(data),
        recipient: recipient(data),
        body: data[BODY_OFFSET..].to_vec(),
    })
}

/// Compute the message ID from raw encoded bytes.
#[must_use]
pub fn id(data: &[u8]) -> MessageId {
    keccak256(data)
}

/// Extract the version from raw encoded message bytes.
#[must_use]
pub fn version(data: &[u8]) -> u8 {
    data[VERSION_OFFSET]
}

/// Extract the nonce from raw encoded message bytes.
#[must_use]
pub fn nonce(data: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&data[NONCE_OFFSET..ORIGIN_OFFSET]);
    u32::from_be_bytes(buf)
}

/// Extract the origin domain from raw encoded message bytes.
#[must_use]
pub fn origin(data: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&data[ORIGIN_OFFSET..SENDER_OFFSET]);
    u32::from_be_bytes(buf)
}

/// Extract the sender from raw encoded message bytes.
#[must_use]
pub fn sender(data: &[u8]) -> H256 {
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&data[SENDER_OFFSET..DESTINATION_OFFSET]);
    buf
}

/// Extract the destination domain from raw encoded message bytes.
#[must_use]
pub fn destination(data: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&data[DESTINATION_OFFSET..RECIPIENT_OFFSET]);
    u32::from_be_bytes(buf)
}

/// Extract the recipient from raw encoded message bytes.
#[must_use]
pub fn recipient(data: &[u8]) -> H256 {
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&data[RECIPIENT_OFFSET..BODY_OFFSET]);
    buf
}

/// Extract the body from raw encoded message bytes.
#[must_use]
pub fn body(data: &[u8]) -> &[u8] {
    &data[BODY_OFFSET..]
}

/// Compute keccak256 hash.
///
/// On-chain this calls the Dusk VM host function. Off-chain it uses a
/// software implementation.
#[must_use]
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    // When running inside the Dusk VM (on-chain), use the host function.
    // The `abi` feature is only available when compiling for the contract target.
    #[cfg(feature = "abi")]
    {
        dusk_core::abi::keccak256(data.to_vec())
    }

    // Off-chain fallback: use a software keccak256 implementation.
    // We implement it here to avoid pulling in a heavy dependency.
    #[cfg(not(feature = "abi"))]
    {
        keccak256_software(data)
    }
}

/// Software keccak256 implementation for off-chain / testing use.
///
/// This is a minimal implementation of the Keccak-256 sponge function
/// following the NIST FIPS 202 / Keccak specification.
#[cfg(not(feature = "abi"))]
fn keccak256_software(data: &[u8]) -> [u8; 32] {
    const RATE: usize = 136; // rate in bytes for Keccak-256 (1088 bits)
    const ROUNDS: usize = 24;

    #[rustfmt::skip]
    const RC: [u64; 24] = [
        0x0000_0000_0000_0001, 0x0000_0000_0000_8082, 0x8000_0000_0000_808a,
        0x8000_0000_8000_8000, 0x0000_0000_0000_808b, 0x0000_0000_8000_0001,
        0x8000_0000_8000_8081, 0x8000_0000_0000_8009, 0x0000_0000_0000_008a,
        0x0000_0000_0000_0088, 0x0000_0000_8000_8009, 0x0000_0000_8000_000a,
        0x0000_0000_8000_808b, 0x8000_0000_0000_008b, 0x8000_0000_0000_8089,
        0x8000_0000_0000_8003, 0x8000_0000_0000_8002, 0x8000_0000_0000_0080,
        0x0000_0000_0000_800a, 0x8000_0000_8000_000a, 0x8000_0000_8000_8081,
        0x8000_0000_0000_8080, 0x0000_0000_8000_0001, 0x8000_0000_8000_8008,
    ];

    #[rustfmt::skip]
    const ROTC: [u32; 24] = [
        1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14,
        27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
    ];

    #[rustfmt::skip]
    const PILN: [usize; 24] = [
        10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4,
        15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
    ];

    fn keccak_f(st: &mut [u64; 25]) {
        for round in 0..ROUNDS {
            // Theta
            let mut bc = [0u64; 5];
            for i in 0..5 {
                bc[i] = st[i] ^ st[i + 5] ^ st[i + 10] ^ st[i + 15] ^ st[i + 20];
            }
            for i in 0..5 {
                let t = bc[(i + 4) % 5] ^ bc[(i + 1) % 5].rotate_left(1);
                for j in (0..25).step_by(5) {
                    st[j + i] ^= t;
                }
            }

            // Rho and Pi
            let mut t = st[1];
            for i in 0..24 {
                let j = PILN[i];
                let tmp = st[j];
                st[j] = t.rotate_left(ROTC[i]);
                t = tmp;
            }

            // Chi
            for j in (0..25).step_by(5) {
                let mut bc_local = [0u64; 5];
                for i in 0..5 {
                    bc_local[i] = st[j + i];
                }
                for i in 0..5 {
                    st[j + i] ^= (!bc_local[(i + 1) % 5]) & bc_local[(i + 2) % 5];
                }
            }

            // Iota
            st[0] ^= RC[round];
        }
    }

    let mut state = [0u64; 25];

    // Absorb
    let mut offset = 0;
    while offset + RATE <= data.len() {
        for i in 0..(RATE / 8) {
            let word = u64::from_le_bytes(
                data[offset + i * 8..offset + i * 8 + 8]
                    .try_into()
                    .unwrap(),
            );
            state[i] ^= word;
        }
        keccak_f(&mut state);
        offset += RATE;
    }

    // Pad last block
    let remaining = data.len() - offset;
    let mut last_block = [0u8; RATE];
    last_block[..remaining].copy_from_slice(&data[offset..]);
    last_block[remaining] = 0x01; // Keccak padding (NOT SHA-3 which uses 0x06)
    last_block[RATE - 1] |= 0x80;

    for i in 0..(RATE / 8) {
        let word = u64::from_le_bytes(last_block[i * 8..i * 8 + 8].try_into().unwrap());
        state[i] ^= word;
    }
    keccak_f(&mut state);

    // Squeeze
    let mut output = [0u8; 32];
    for i in 0..4 {
        output[i * 8..(i + 1) * 8].copy_from_slice(&state[i].to_le_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keccak256_empty() {
        // keccak256("") = c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        let hash = keccak256(&[]);
        assert_eq!(
            hash,
            [
                0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c, 0x92, 0x7e, 0x7d, 0xb2, 0xdc,
                0xc7, 0x03, 0xc0, 0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b, 0x7b, 0xfa,
                0xd8, 0x04, 0x5d, 0x85, 0xa4, 0x70,
            ]
        );
    }

    #[test]
    fn test_keccak256_hello() {
        // keccak256("hello") = 1c8aff950685c2ed4bc3174f3472287b56d9517b9c948127319a09a7a36deac8
        let hash = keccak256(b"hello");
        assert_eq!(
            hash,
            [
                0x1c, 0x8a, 0xff, 0x95, 0x06, 0x85, 0xc2, 0xed, 0x4b, 0xc3, 0x17, 0x4f, 0x34,
                0x72, 0x28, 0x7b, 0x56, 0xd9, 0x51, 0x7b, 0x9c, 0x94, 0x81, 0x27, 0x31, 0x9a,
                0x09, 0xa7, 0xa3, 0x6d, 0xea, 0xc8,
            ]
        );
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let msg = HyperlaneMessage {
            version: VERSION,
            nonce: 42,
            origin: 1,
            sender: [0xAA; 32],
            destination: 2,
            recipient: [0xBB; 32],
            body: alloc::vec![1, 2, 3, 4],
        };

        let encoded = msg.encode();
        assert_eq!(encoded.len(), BODY_OFFSET + 4);

        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_field_accessors() {
        let msg = HyperlaneMessage {
            version: VERSION,
            nonce: 100,
            origin: 11155111, // Sepolia
            sender: {
                let mut s = [0u8; 32];
                s[12..].copy_from_slice(&[0xDE; 20]);
                s
            },
            destination: 42161, // Arbitrum
            recipient: [0xCC; 32],
            body: alloc::vec![0xFF; 10],
        };

        let encoded = msg.encode();

        assert_eq!(version(&encoded), VERSION);
        assert_eq!(nonce(&encoded), 100);
        assert_eq!(origin(&encoded), 11155111);
        assert_eq!(destination(&encoded), 42161);
        assert_eq!(recipient(&encoded), [0xCC; 32]);
        assert_eq!(body(&encoded), &[0xFF; 10]);
    }

    #[test]
    fn test_decode_bad_version() {
        let mut encoded = HyperlaneMessage {
            version: VERSION,
            nonce: 0,
            origin: 0,
            sender: [0; 32],
            destination: 0,
            recipient: [0; 32],
            body: alloc::vec![],
        }
        .encode();

        encoded[0] = VERSION + 1; // wrong version
        assert!(decode(&encoded).is_none());
    }

    #[test]
    fn test_decode_too_short() {
        assert!(decode(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_message_id_matches_keccak() {
        let msg = HyperlaneMessage {
            version: VERSION,
            nonce: 1,
            origin: 1,
            sender: [0x11; 32],
            destination: 2,
            recipient: [0x22; 32],
            body: alloc::vec![0xAB, 0xCD],
        };

        let encoded = msg.encode();
        assert_eq!(msg.id(), keccak256(&encoded));
        assert_eq!(msg.id(), id(&encoded));
    }
}
