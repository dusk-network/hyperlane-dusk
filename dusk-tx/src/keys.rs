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
    let iv: [u8; 12] = b64
        .decode(&encrypted.iv)
        .map_err(|e| format!("Failed to decode IV: {e}"))?
        .try_into()
        .map_err(|_| "Invalid IV length: expected 12 bytes".to_string())?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

    struct KeyFile(PathBuf);

    impl KeyFile {
        fn with_iv_length(length: usize) -> Self {
            let path = std::env::temp_dir().join(format!(
                "dusk-tx-key-input-{}-{}.json",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            let b64 = base64::engine::general_purpose::STANDARD;
            let input = serde_json::json!({
                "salt": b64.encode([0u8; 16]),
                "iv": b64.encode(vec![0u8; length]),
                "key_pair": [],
            });
            file.write_all(input.to_string().as_bytes()).unwrap();
            Self(path)
        }
    }

    impl Drop for KeyFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn malformed_key_file_iv_returns_error_without_panicking() {
        for length in [0, 11, 13, 16, 32] {
            let file = KeyFile::with_iv_length(length);
            let result = std::panic::catch_unwind(|| {
                load_from_file(file.0.to_str().unwrap(), "synthetic-fixture-password")
            });
            assert!(result.is_ok(), "IV length {length} must not panic");
            let error = result.unwrap().err().expect("invalid IV must fail");
            assert!(error.contains("IV"), "unexpected error: {error}");
        }
    }

    #[test]
    fn valid_iv_length_reaches_authenticated_decryption() {
        let file = KeyFile::with_iv_length(12);
        let error = load_from_file(file.0.to_str().unwrap(), "synthetic-fixture-password")
            .err()
            .expect("empty ciphertext must fail authentication");
        assert!(error.contains("Failed to decrypt keys"));
    }
}
