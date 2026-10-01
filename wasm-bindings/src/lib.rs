// SPDX-License-Identifier: MIT OR Apache-2.0
//
// WASM bindings for hyperlane-dusk-types.
//
// Exposes message encoding/decoding, token message encoding, and rkyv
// serialization helpers to JavaScript via wasm-bindgen.

use wasm_bindgen::prelude::*;

use hyperlane_dusk_types::{message, token_message, VERSION};

// =============================================================================
// Message encoding / decoding
// =============================================================================

/// Encode a Hyperlane message.
///
/// Returns the raw encoded message bytes.
#[wasm_bindgen]
pub fn encode_message(
    version: u8,
    nonce: u32,
    origin: u32,
    sender: &[u8],
    destination: u32,
    recipient: &[u8],
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let sender_h256 = checked_bytes32(sender)?;
    let recipient_h256 = checked_bytes32(recipient)?;
    Ok(message::encode(
        version,
        nonce,
        origin,
        sender_h256,
        destination,
        recipient_h256,
        body,
    ))
}

/// Decode a Hyperlane message and return it as a JSON string.
///
/// Returns undefined if the message is invalid.
#[wasm_bindgen]
pub fn decode_message(encoded: &[u8]) -> Option<String> {
    let msg = message::decode(encoded)?;
    let json = serde_json::json!({
        "version": msg.version,
        "nonce": msg.nonce,
        "origin": msg.origin,
        "sender": hex::encode(&msg.sender),
        "destination": msg.destination,
        "recipient": hex::encode(&msg.recipient),
        "body": hex::encode(&msg.body),
    });
    Some(json.to_string())
}

/// Compute the message ID (keccak256 hash) of an encoded message.
#[wasm_bindgen]
pub fn message_id(encoded: &[u8]) -> Vec<u8> {
    message::id(encoded).to_vec()
}

/// Get the current Hyperlane protocol version.
#[wasm_bindgen]
pub fn protocol_version() -> u8 {
    VERSION
}

/// Extract the destination domain from an encoded message.
#[wasm_bindgen]
pub fn message_destination(encoded: &[u8]) -> u32 {
    message::destination(encoded)
}

/// Extract the nonce from an encoded message.
#[wasm_bindgen]
pub fn message_nonce(encoded: &[u8]) -> u32 {
    message::nonce(encoded)
}

// =============================================================================
// Token message encoding / decoding
// =============================================================================

/// Encode a token message body (recipient + amount).
#[wasm_bindgen]
pub fn encode_token_message(recipient: &[u8], amount: u64) -> Result<Vec<u8>, String> {
    Ok(token_message::encode(checked_bytes32(recipient)?, amount))
}

/// Decode a token message body and return it as a JSON string.
///
/// Amounts are decimal strings to preserve all u64 values in JavaScript.
/// Returns undefined if the body is invalid.
#[wasm_bindgen]
pub fn decode_token_message(body: &[u8]) -> Option<String> {
    let msg = token_message::decode(body)?;
    let json = serde_json::json!({
        "recipient": hex::encode(&msg.recipient),
        "amount": msg.amount.to_string(),
        "metadata": hex::encode(msg.metadata),
    });
    Some(json.to_string())
}

// =============================================================================
// Utility: keccak256
// =============================================================================

/// Compute keccak256 of input bytes.
#[wasm_bindgen]
pub fn keccak256(data: &[u8]) -> Vec<u8> {
    message::keccak256(data).to_vec()
}

// =============================================================================
// rkyv helpers: serialize common argument types for RUES contract queries
// =============================================================================

/// Serialize a u32 value using rkyv (for contract query arguments).
#[wasm_bindgen]
pub fn rkyv_serialize_u32(value: u32) -> Vec<u8> {
    rkyv::to_bytes::<_, 256>(&value)
        .expect("u32 serialization should not fail")
        .to_vec()
}

/// Serialize a u64 value using rkyv.
#[wasm_bindgen]
pub fn rkyv_serialize_u64(value: u64) -> Vec<u8> {
    rkyv::to_bytes::<_, 256>(&value)
        .expect("u64 serialization should not fail")
        .to_vec()
}

/// Serialize a bool value using rkyv.
#[wasm_bindgen]
pub fn rkyv_serialize_bool(value: bool) -> Vec<u8> {
    rkyv::to_bytes::<_, 256>(&value)
        .expect("bool serialization should not fail")
        .to_vec()
}

/// Serialize a bytes32 (H256) value using rkyv.
#[wasm_bindgen]
pub fn rkyv_serialize_bytes32(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(rkyv::to_bytes::<_, 256>(&checked_bytes32(data)?)
        .expect("H256 serialization should not fail")
        .to_vec())
}

/// Serialize an empty tuple () using rkyv (for no-argument queries).
#[wasm_bindgen]
pub fn rkyv_serialize_unit() -> Vec<u8> {
    rkyv::to_bytes::<_, 256>(&())
        .expect("unit serialization should not fail")
        .to_vec()
}

/// Deserialize a u8 from rkyv bytes (for token decimals).
#[wasm_bindgen]
pub fn rkyv_deserialize_u8(data: &[u8]) -> u8 {
    *rkyv::check_archived_root::<u8>(data).expect("invalid rkyv u8 data")
}

/// Deserialize a String, including rkyv's inline short-string representation.
#[wasm_bindgen]
pub fn rkyv_deserialize_string(data: &[u8]) -> String {
    rkyv::check_archived_root::<String>(data)
        .expect("invalid rkyv String data")
        .as_str()
        .to_owned()
}

/// Deserialize a u32 from rkyv bytes.
#[wasm_bindgen]
pub fn rkyv_deserialize_u32(data: &[u8]) -> u32 {
    let archived = rkyv::check_archived_root::<u32>(data).expect("invalid rkyv u32 data");
    *archived
}

/// Deserialize a u64 from rkyv bytes.
#[wasm_bindgen]
pub fn rkyv_deserialize_u64(data: &[u8]) -> u64 {
    let archived = rkyv::check_archived_root::<u64>(data).expect("invalid rkyv u64 data");
    *archived
}

/// Deserialize a bool from rkyv bytes.
#[wasm_bindgen]
pub fn rkyv_deserialize_bool(data: &[u8]) -> bool {
    let archived = rkyv::check_archived_root::<bool>(data).expect("invalid rkyv bool data");
    *archived
}

/// Deserialize a bytes32 (H256) from rkyv bytes, returned as hex string.
#[wasm_bindgen]
pub fn rkyv_deserialize_bytes32(data: &[u8]) -> String {
    let archived = rkyv::check_archived_root::<[u8; 32]>(data).expect("invalid rkyv H256 data");
    hex::encode(archived)
}

/// Deserialize raw bytes (Vec<u8>) from rkyv bytes.
#[wasm_bindgen]
pub fn rkyv_deserialize_bytes(data: &[u8]) -> Vec<u8> {
    let archived = rkyv::check_archived_root::<Vec<u8>>(data).expect("invalid rkyv Vec<u8> data");
    archived.to_vec()
}

// =============================================================================
// Hex module (minimal, no_std compatible)
// =============================================================================

mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(data: &[u8]) -> String {
        let mut s = String::with_capacity(data.len() * 2);
        for &byte in data {
            s.push(HEX_CHARS[(byte >> 4) as usize] as char);
            s.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
        }
        s
    }
}

/// Validate addresses before encoding; truncation or padding changes identity.
fn checked_bytes32(data: &[u8]) -> Result<[u8; 32], String> {
    data.try_into()
        .map_err(|_| "Expected exactly 32 bytes".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rkyv_string_decoder_handles_inline_and_allocated_strings() {
        for value in ["", "wDUSK", "Wrapped Dusk", "Dusk 🌘"] {
            let encoded = rkyv::to_bytes::<_, 256>(&value.to_owned()).unwrap();
            assert_eq!(rkyv_deserialize_string(&encoded), value);
        }
    }

    #[test]
    fn decimals_use_the_one_byte_rkyv_representation() {
        for value in [0u8, 9, 18, 255] {
            let encoded = rkyv::to_bytes::<_, 256>(&value).unwrap();
            assert_eq!(encoded.len(), 1);
            assert_eq!(rkyv_deserialize_u8(&encoded), value);
        }
    }
}
