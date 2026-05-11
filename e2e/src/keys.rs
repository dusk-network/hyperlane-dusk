//! BLS key loading from encrypted consensus.keys files.
//!
//! The consensus.keys file is AES-256-GCM encrypted with a password
//! (default: "password") using PBKDF2-HMAC-SHA256 key derivation.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use dusk_core::signatures::bls::{
    PublicKey as BlsPublicKey, SecretKey as BlsSecretKey,
};
use pbkdf2::pbkdf2_hmac;
use serde::Deserialize;
use sha2::Sha256;

const PBKDF2_ROUNDS: u32 = 10_000;

/// JSON structure of the encrypted consensus.keys file.
#[derive(Deserialize)]
struct EncryptedKeys {
    salt: String,
    iv: String,
    key_pair: Vec<u8>,
}

/// JSON structure of the decrypted key pair.
#[derive(Deserialize)]
struct BlsKeyPair {
    secret_key_bls: String,
    public_key_bls: String,
}

/// Load BLS keys from an encrypted consensus.keys file.
pub fn load_bls_keys(
    path: &str,
    password: &str,
) -> Result<(BlsSecretKey, BlsPublicKey), Box<dyn std::error::Error>> {
    let contents = std::fs::read(path)
        .map_err(|e| format!("Failed to read consensus.keys at {path}: {e}"))?;

    let encrypted: EncryptedKeys = serde_json::from_slice(&contents)
        .map_err(|e| format!("Failed to parse consensus.keys JSON: {e}"))?;

    let b64 = base64::engine::general_purpose::STANDARD;

    // Decode salt and IV
    let salt = b64.decode(&encrypted.salt)
        .map_err(|e| format!("Failed to decode salt: {e}"))?;
    let iv = b64.decode(&encrypted.iv)
        .map_err(|e| format!("Failed to decode IV: {e}"))?;

    // Derive AES key via PBKDF2
    let mut aes_key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ROUNDS, &mut aes_key);

    // Decrypt with AES-256-GCM
    let cipher = Aes256Gcm::new_from_slice(&aes_key)
        .map_err(|e| format!("Failed to create cipher: {e}"))?;
    let nonce = Nonce::from_slice(&iv);
    let plaintext = cipher
        .decrypt(nonce, encrypted.key_pair.as_ref())
        .map_err(|e| format!("Failed to decrypt keys: {e}"))?;

    // Parse decrypted JSON
    let keypair: BlsKeyPair = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Failed to parse decrypted key pair: {e}"))?;

    // Decode BLS keys from base64
    let sk_bytes = b64.decode(&keypair.secret_key_bls)
        .map_err(|e| format!("Failed to decode secret key: {e}"))?;
    let pk_bytes = b64.decode(&keypair.public_key_bls)
        .map_err(|e| format!("Failed to decode public key: {e}"))?;

    // Parse BLS key types
    use dusk_bytes::DeserializableSlice;
    let sk = BlsSecretKey::from_slice(&sk_bytes)
        .map_err(|e| format!("Invalid BLS secret key: {e:?}"))?;
    let pk = BlsPublicKey::from_slice(&pk_bytes)
        .map_err(|e| format!("Invalid BLS public key: {e:?}"))?;

    Ok((sk, pk))
}
