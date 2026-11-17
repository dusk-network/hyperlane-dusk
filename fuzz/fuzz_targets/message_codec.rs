// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hyperlane message encoding (`types::message`) against `Message.sol`'s
//! packed layout: decoding arbitrary bytes, the field accessors, encoding
//! arbitrary fields, and the message ID against an independent keccak256.

#![no_main]

use hyperlane_dusk_fuzz::{reference_keccak, Reader};
use hyperlane_dusk_types::message::{self, HyperlaneMessage, MIN_MESSAGE_LENGTH};
use hyperlane_dusk_types::VERSION;
use libfuzzer_sys::fuzz_target;

/// `Message.sol`'s layout, written out independently of the crate.
fn packed(
    version: u8,
    nonce: u32,
    origin: u32,
    sender: &[u8; 32],
    destination: u32,
    recipient: &[u8; 32],
    body: &[u8],
) -> Vec<u8> {
    [
        &[version][..],
        &nonce.to_be_bytes(),
        &origin.to_be_bytes(),
        sender,
        &destination.to_be_bytes(),
        recipient,
        body,
    ]
    .concat()
}

fuzz_target!(|data: &[u8]| {
    // Arbitrary bytes, as `process` receives them.
    match message::decode(data) {
        Some(decoded) => {
            assert!(data.len() >= MIN_MESSAGE_LENGTH);
            assert_eq!(data[0], VERSION);
            assert_eq!(decoded.version, VERSION);
            // Decoding loses nothing: the message encodes back to the input,
            // so its ID is the ID of the bytes the relayer submitted.
            assert_eq!(decoded.encode(), data);
            assert_eq!(decoded.id(), message::id(data));
            assert_eq!(message::version(data), decoded.version);
            assert_eq!(message::nonce(data), decoded.nonce);
            assert_eq!(message::origin(data), decoded.origin);
            assert_eq!(message::sender(data), decoded.sender);
            assert_eq!(message::destination(data), decoded.destination);
            assert_eq!(message::recipient(data), decoded.recipient);
            assert_eq!(message::body(data), decoded.body.as_slice());
            assert_eq!(
                packed(
                    decoded.version,
                    decoded.nonce,
                    decoded.origin,
                    &decoded.sender,
                    decoded.destination,
                    &decoded.recipient,
                    &decoded.body,
                ),
                data
            );
        }
        None => assert!(data.len() < MIN_MESSAGE_LENGTH || data[0] != VERSION),
    }
    assert_eq!(message::id(data), reference_keccak(data));

    // Arbitrary fields, as `dispatch` encodes them.
    let mut reader = Reader::new(data);
    let version = if reader.u8() & 1 == 0 {
        VERSION
    } else {
        reader.u8()
    };
    let fields = HyperlaneMessage {
        version,
        nonce: reader.u32(),
        origin: reader.u32(),
        sender: reader.array(),
        destination: reader.u32(),
        recipient: reader.array(),
        body: reader.rest().to_vec(),
    };
    let encoded = fields.encode();
    assert_eq!(
        encoded,
        packed(
            fields.version,
            fields.nonce,
            fields.origin,
            &fields.sender,
            fields.destination,
            &fields.recipient,
            &fields.body,
        )
    );
    assert_eq!(encoded.len(), MIN_MESSAGE_LENGTH + fields.body.len());
    assert_eq!(fields.id(), reference_keccak(&encoded));
    match message::decode(&encoded) {
        Some(decoded) => {
            assert_eq!(fields.version, VERSION);
            assert_eq!(decoded, fields);
        }
        None => assert_ne!(fields.version, VERSION),
    }
});
