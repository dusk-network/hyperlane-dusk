//! BLS key loading from encrypted consensus.keys files or raw hex.

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

#[derive(Deserialize)]
struct EncryptedKeys {
    salt: String,
    iv: String,
    key_pair: Vec<u8>,
}

#[derive(Deserialize)]
struct BlsKeyPair {
    secret_key_bls: String,
    public_key_bls: String,
}

/// Load BLS keys from an encrypted consensus.keys file.
pub fn load_from_file(
    path: &str,
    password: &str,
) -> Result<(BlsSecretKey, BlsPublicKey), String> {
    let contents = std::fs::read(path)
        .map_err(|e| format!("Failed to read consensus.keys at {path}: {e}"))?;

    let encrypted: EncryptedKeys = serde_json::from_slice(&contents)
        .map_err(|e| format!("Failed to parse consensus.keys JSON: {e}"))?;

    let b64 = base64::engine::general_purpose::STANDARD;

    let salt = b64
        .decode(&encrypted.salt)
        .map_err(|e| format!("Failed to decode salt: {e}"))?;
    let iv = b64
        .decode(&encrypted.iv)
        .map_err(|e| format!("Failed to decode IV: {e}"))?;

    let mut aes_key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ROUNDS, &mut aes_key);

    let cipher = Aes256Gcm::new_from_slice(&aes_key)
        .map_err(|e| format!("Failed to create cipher: {e}"))?;
    let nonce = Nonce::from_slice(&iv);
    let plaintext = cipher
        .decrypt(nonce, encrypted.key_pair.as_ref())
        .map_err(|e| format!("Failed to decrypt keys: {e}"))?;

    let keypair: BlsKeyPair = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Failed to parse decrypted key pair: {e}"))?;

    let b64 = base64::engine::general_purpose::STANDARD;
    let sk_bytes = b64
        .decode(&keypair.secret_key_bls)
        .map_err(|e| format!("Failed to decode secret key: {e}"))?;
    let pk_bytes = b64
        .decode(&keypair.public_key_bls)
        .map_err(|e| format!("Failed to decode public key: {e}"))?;

    use dusk_bytes::DeserializableSlice;
    let sk = BlsSecretKey::from_slice(&sk_bytes)
        .map_err(|e| format!("Invalid BLS secret key: {e:?}"))?;
    let pk = BlsPublicKey::from_slice(&pk_bytes)
        .map_err(|e| format!("Invalid BLS public key: {e:?}"))?;

    Ok((sk, pk))
}

/// Load BLS keys from a hex-encoded secret key (64 hex chars = 32 bytes).
pub fn load_from_hex(secret_key_hex: &str) -> Result<(BlsSecretKey, BlsPublicKey), String> {
    let sk_bytes = hex::decode(secret_key_hex)
        .map_err(|e| format!("Invalid hex secret key: {e}"))?;
    if sk_bytes.len() != 32 {
        return Err(format!(
            "Secret key must be 32 bytes (64 hex chars), got {}",
            sk_bytes.len()
        ));
    }

    use dusk_bytes::DeserializableSlice;
    let sk = BlsSecretKey::from_slice(&sk_bytes)
        .map_err(|e| format!("Invalid BLS secret key: {e:?}"))?;
    let pk = BlsPublicKey::from(&sk);

    Ok((sk, pk))
}
