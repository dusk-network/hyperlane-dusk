// SPDX-License-Identifier: MIT OR Apache-2.0

//! Warp transfer bodies (`types::token_message`) against `TokenMessage.sol`
//! (`abi.encodePacked(bytes32 recipient, uint256 amount, bytes metadata)`),
//! with the amount read as the full uint256, and the hook metadata gas limit
//! (`types::metadata`).

#![no_main]

use hyperlane_dusk_fuzz::Reader;
use hyperlane_dusk_types::metadata::{gas_limit, DEFAULT_GAS_USAGE};
use hyperlane_dusk_types::token_message::{decode, encode, encode_with_metadata};
use libfuzzer_sys::fuzz_target;

/// The uint256 amount of a TokenMessage as a u64, if it fits; computed
/// without the crate's byte offsets.
fn reference_amount(amount: &[u8]) -> Option<u64> {
    assert_eq!(amount.len(), 32);
    // Big-endian uint256: fits a u64 exactly when its value is below 2^64.
    let mut value: u128 = 0;
    for (position, byte) in amount.iter().enumerate() {
        if position < 16 {
            if *byte != 0 {
                return None;
            }
            continue;
        }
        value = (value << 8) | u128::from(*byte);
    }
    u64::try_from(value).ok()
}

fuzz_target!(|data: &[u8]| {
    // Arbitrary bodies, as `handle` receives them.
    match decode(data) {
        Some(message) => {
            assert!(data.len() >= 64);
            assert_eq!(message.recipient.as_slice(), &data[..32]);
            assert_eq!(Some(message.amount), reference_amount(&data[32..64]));
            assert_eq!(message.metadata, &data[64..]);
            // Nothing is lost: the fields encode back to the input.
            assert_eq!(
                encode_with_metadata(message.recipient, message.amount, message.metadata),
                data
            );
        }
        None => assert!(data.len() < 64 || reference_amount(&data[32..64]).is_none()),
    }

    // Arbitrary fields, as `transfer_remote` encodes them.
    let mut reader = Reader::new(data);
    let recipient: [u8; 32] = reader.array();
    let amount = match reader.u8() % 6 {
        0 => 0,
        1 => 1,
        2 => u64::MAX,
        3 => u64::MAX - 1,
        4 => 1 << 63,
        _ => reader.u64(),
    };
    let metadata = reader.rest();
    let plain = encode(recipient, amount);
    assert_eq!(plain.len(), 64);
    assert_eq!(&plain[..32], &recipient);
    assert_eq!(&plain[32..56], &[0u8; 24]);
    assert_eq!(&plain[56..64], &amount.to_be_bytes());
    let with = encode_with_metadata(recipient, amount, metadata);
    assert_eq!(&with[..64], plain.as_slice());
    assert_eq!(&with[64..], metadata);
    let decoded = decode(&with).expect("an encoded body decodes");
    assert_eq!(decoded.recipient, recipient);
    assert_eq!(decoded.amount, amount);
    assert_eq!(decoded.metadata, metadata);

    // Amounts of 2^64 and above, at every byte of the uint256, are refused
    // rather than truncated.
    let byte = usize::from(reader.u8()) % 24;
    let mut too_big = plain.clone();
    too_big[32 + byte] |= 1 + (data.first().copied().unwrap_or(0) & 0x7f);
    assert!(decode(&too_big).is_none());

    // Hook metadata: the first 8 bytes, little endian, or the default.
    let expected = if data.len() >= 8 {
        u64::from_le_bytes(data[..8].try_into().unwrap())
    } else {
        DEFAULT_GAS_USAGE
    };
    assert_eq!(gas_limit(data), expected);
});
