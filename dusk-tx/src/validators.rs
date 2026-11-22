//! What a Hyperlane validator writes to its checkpoint storage: its signed
//! announcement and checkpoints, where an `s3://` or `file://` location
//! keeps them, and who signed them. The JSON is hyperlane-core's
//! `SignedType`; locations parse as the agents' `CheckpointSyncerConf`.

use std::path::PathBuf;
use std::time::Duration;

use hyperlane_dusk_types::message::keccak256;
use hyperlane_dusk_types::signature::is_canonical;
use reqwest::Url;
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use secp256k1::{Message, Secp256k1};
use serde::Deserialize;

/// The most read of one object, below the relayer's limit for one
/// (`MAX_CHECKPOINT_OBJECT_SIZE`, 50 KiB): a validator writes a few hundred
/// bytes.
const MAX_OBJECT_BYTES: usize = 50 * 1024 - 1;

/// A validator's checkpoint storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    /// `file://<path>`, a local rehearsal's.
    Local(PathBuf),
    /// `s3://bucket/region[/folder]`, read path-style from an endpoint.
    S3 {
        bucket: String,
        folder: Option<String>,
    },
}

impl Storage {
    pub fn parse(location: &str) -> Result<Self, String> {
        let bad = || {
            format!("Can't read the storage location '{location}': expected s3://bucket/region[/folder] or file://<path>")
        };
        if let Some(path) = location.strip_prefix("file://") {
            return Ok(Storage::Local(PathBuf::from(path)));
        }
        let rest = location.strip_prefix("s3://").ok_or_else(bad)?;
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 2 || rest.contains("://") {
            return Err(bad());
        }
        let folder = parts[2..].join("/");
        Ok(Storage::S3 {
            bucket: parts[0].to_owned(),
            folder: (!folder.is_empty()).then_some(folder),
        })
    }

    pub fn announcement(&self) -> &'static str {
        "announcement.json"
    }

    pub fn latest_index(&self) -> &'static str {
        match self {
            Storage::Local(_) => "index.json",
            Storage::S3 { .. } => "checkpoint_latest_index.json",
        }
    }

    pub fn checkpoint(&self, index: u32) -> String {
        match self {
            Storage::Local(_) => format!("{index}_with_id.json"),
            Storage::S3 { .. } => format!("checkpoint_{index}_with_id.json"),
        }
    }

    /// A reorg the validator saw: while it is there, the relayer delivers
    /// nothing through an ISM that lists the validator.
    pub fn reorg_flag(&self) -> &'static str {
        "reorg_flag.json"
    }

    /// Read the object `name`: its bytes, or `None` when it isn't there. S3
    /// objects are read anonymously and path-style from `endpoint`, as the
    /// agents read them with AWS_ENDPOINT_URL set, without following
    /// redirects.
    pub async fn read(
        &self,
        name: &str,
        endpoint: Option<&Url>,
    ) -> Result<Option<Vec<u8>>, String> {
        match self {
            Storage::Local(dir) => read_file(&dir.join(name)),
            Storage::S3 { bucket, folder } => {
                let mut url = endpoint
                    .ok_or("Reading an s3:// location needs the S3 endpoint (--s3-endpoint)")?
                    .clone();
                let mut segments = vec![bucket.as_str()];
                segments.extend(folder.iter().flat_map(|folder| folder.split('/')));
                segments.push(name);
                if segments.iter().any(|segment| {
                    segment.is_empty()
                        || *segment == "."
                        || *segment == ".."
                        || segment
                            .chars()
                            .any(|c| c.is_control() || matches!(c, '\\' | '%'))
                }) {
                    return Err(format!(
                        "'{bucket}' or its folder has a part a key can't have"
                    ));
                }
                url.path_segments_mut()
                    .map_err(|()| "The S3 endpoint can't take a path".to_string())?
                    .pop_if_empty()
                    .extend(&segments);
                read_url(&url).await
            }
        }
    }
}

/// Parse an S3 endpoint as the agents' AWS_ENDPOINT_URL takes it.
pub fn parse_endpoint(endpoint: &str) -> Result<Url, String> {
    let url = Url::parse(endpoint).map_err(|e| format!("Invalid S3 endpoint '{endpoint}': {e}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
    {
        return Err(format!(
            "Invalid S3 endpoint '{endpoint}': expected http(s)://host[:port][/path]"
        ));
    }
    Ok(url)
}

async fn read_url(url: &Url) -> Result<Option<Vec<u8>>, String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Can't make an HTTP client: {e}"))?;
    let mut response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| format!("Can't read {url}: {e}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "Can't read {url}: HTTP {}. A validator's bucket must be readable by anyone.",
            response.status()
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Can't read {url}: {e}"))?
    {
        if body.len() + chunk.len() > MAX_OBJECT_BYTES {
            return Err(format!("{url} is over {MAX_OBJECT_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

fn read_file(path: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    // A symlink could lead out of the checkpoint directory.
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_OBJECT_BYTES as u64 => {
            std::fs::read(path)
                .map(Some)
                .map_err(|e| format!("Can't read {}: {e}", path.display()))
        }
        _ => Err(format!(
            "{} isn't a file of at most {MAX_OBJECT_BYTES} bytes",
            path.display()
        )),
    }
}

// ── Signed values ───────────────────────────────────────────────────────────

/// hyperlane-core's `SignedType` JSON. Derived, so a duplicate field fails
/// as it does for the relayer.
#[derive(Deserialize)]
struct Signed<T> {
    value: T,
    signature: SignatureJson,
}

#[derive(Deserialize)]
struct SignatureJson {
    r: String,
    s: String,
    v: u8,
}

#[derive(Deserialize)]
struct AnnouncementJson {
    validator: String,
    storage_location: String,
}

#[derive(Deserialize)]
struct CheckpointWithIdJson {
    checkpoint: CheckpointJson,
    message_id: String,
}

#[derive(Deserialize)]
struct CheckpointJson {
    merkle_tree_hook_address: String,
    mailbox_domain: u32,
    root: String,
    index: u32,
}

fn signed<'a, T: Deserialize<'a>>(json: &'a [u8]) -> Result<(T, [u8; 65]), String> {
    let signed: Signed<T> =
        serde_json::from_slice(json).map_err(|e| format!("Not a validator's signed value: {e}"))?;
    let mut signature = [0u8; 65];
    for (value, range) in [(&signed.signature.r, 0..32), (&signed.signature.s, 32..64)] {
        // hyperlane-core writes r and s without leading zeros.
        let digits = value.strip_prefix("0x").unwrap_or_default();
        if digits.is_empty() || digits.len() > 64 {
            return Err(format!(
                "The signature's '{value}' isn't a 256-bit hex number"
            ));
        }
        hex::decode_to_slice(format!("{digits:0>64}"), &mut signature[range])
            .map_err(|e| format!("The signature's '{value}': {e}"))?;
    }
    signature[64] = signed.signature.v;
    Ok((signed.value, signature))
}

/// `0x` and exactly N bytes of hex.
fn hex_bytes<const N: usize>(value: &str) -> Result<[u8; N], String> {
    let digits = value.strip_prefix("0x").unwrap_or_default();
    let mut bytes = [0u8; N];
    if digits.len() != 2 * N || hex::decode_to_slice(digits, &mut bytes).is_err() {
        return Err(format!("'{value}' isn't 0x and {N} bytes of hex"));
    }
    Ok(bytes)
}

/// A validator's signed announcement of where its checkpoints are. The
/// ValidatorAnnounce checks the rest when it is sent.
pub struct Announcement {
    pub validator: [u8; 20],
    pub location: String,
    pub signature: [u8; 65],
}

impl Announcement {
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let (value, signature) = signed::<AnnouncementJson>(json)?;
        Ok(Announcement {
            validator: hex_bytes(&value.validator)?,
            location: value.storage_location,
            signature,
        })
    }
}

/// A validator's signed checkpoint of a Merkle tree hook.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub merkle_tree_hook: [u8; 32],
    pub domain: u32,
    pub root: [u8; 32],
    pub index: u32,
    pub message_id: [u8; 32],
    pub signature: [u8; 65],
}

impl Checkpoint {
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let (value, signature) = signed::<CheckpointWithIdJson>(json)?;
        Ok(Checkpoint {
            merkle_tree_hook: hex_bytes(&value.checkpoint.merkle_tree_hook_address)?,
            domain: value.checkpoint.mailbox_domain,
            root: hex_bytes(&value.checkpoint.root)?,
            index: value.checkpoint.index,
            message_id: hex_bytes(&value.message_id)?,
            signature,
        })
    }

    pub fn signer(&self) -> Result<[u8; 20], String> {
        let digest = hyperlane_dusk_types::checkpoint::checkpoint_digest(
            self.domain,
            &self.merkle_tree_hook,
            &self.root,
            self.index,
            &self.message_id,
        );
        recover(&digest, &self.signature)
    }
}

/// Who signed `digest`, for a signature the contracts accept (`v` 27 or 28,
/// `s` in the lower half).
pub fn recover(digest: &[u8; 32], signature: &[u8; 65]) -> Result<[u8; 20], String> {
    if !is_canonical(signature) {
        return Err("The signature isn't one the contracts accept: v must be 27 or 28 and s in the lower half".into());
    }
    let id = RecoveryId::try_from(i32::from(signature[64] - 27)).map_err(|e| e.to_string())?;
    let signature =
        RecoverableSignature::from_compact(&signature[..64], id).map_err(|e| e.to_string())?;
    let public = Secp256k1::verification_only()
        .recover_ecdsa(Message::from_digest(*digest), &signature)
        .map_err(|e| format!("Can't recover the signer: {e}"))?
        .serialize_uncompressed();
    Ok(keccak256(&public[1..])[12..].try_into().expect("20 bytes"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use hyperlane_dusk_types::checkpoint::{announcement_digest, checkpoint_digest};
    use secp256k1::SecretKey;

    pub(crate) const MAILBOX: [u8; 32] = [0xA1; 32];
    pub(crate) const HOOK: [u8; 32] = [0xA4; 32];
    pub(crate) const DOMAIN: u32 = 1_146_442_572;

    pub(crate) fn key(byte: u8) -> SecretKey {
        SecretKey::from_byte_array([byte; 32]).unwrap()
    }

    pub(crate) fn address_of(secret: &SecretKey) -> [u8; 20] {
        let public = secp256k1::PublicKey::from_secret_key(&Secp256k1::new(), secret)
            .serialize_uncompressed();
        keccak256(&public[1..])[12..].try_into().unwrap()
    }

    /// What a validator writes, as hyperlane-core serializes `SignedType`.
    fn signed_json(value: serde_json::Value, secret: &SecretKey, digest: [u8; 32]) -> Vec<u8> {
        let signed = Secp256k1::new().sign_ecdsa_recoverable(Message::from_digest(digest), secret);
        let (id, compact) = signed.serialize_compact();
        let number = |bytes: &[u8]| format!("0x{}", hex::encode(bytes).trim_start_matches('0'));
        serde_json::to_vec(&serde_json::json!({
            "value": value,
            "signature": {"r": number(&compact[..32]), "s": number(&compact[32..]), "v": i32::from(id) + 27},
        }))
        .unwrap()
    }

    pub(crate) fn announcement_json(secret: &SecretKey, location: &str) -> Vec<u8> {
        signed_json(
            serde_json::json!({
                "validator": format!("0x{}", hex::encode(address_of(secret))),
                "mailbox_address": format!("0x{}", hex::encode(MAILBOX)),
                "mailbox_domain": DOMAIN,
                "storage_location": location,
            }),
            secret,
            announcement_digest(DOMAIN, &MAILBOX, location),
        )
    }

    pub(crate) fn checkpoint_json(secret: &SecretKey, index: u32, message_id: [u8; 32]) -> Vec<u8> {
        let root = [0x5A; 32];
        signed_json(
            serde_json::json!({
                "checkpoint": {
                    "merkle_tree_hook_address": format!("0x{}", hex::encode(HOOK)),
                    "mailbox_domain": DOMAIN, "root": format!("0x{}", hex::encode(root)), "index": index,
                },
                "message_id": format!("0x{}", hex::encode(message_id)),
            }),
            secret,
            checkpoint_digest(DOMAIN, &HOOK, &root, index, &message_id),
        )
    }

    #[test]
    fn signed_values_name_their_signers() {
        let parsed = Announcement::parse(&announcement_json(&key(7), "s3://b/nbg1/op")).unwrap();
        assert_eq!(
            (parsed.validator, parsed.location.as_str()),
            (address_of(&key(7)), "s3://b/nbg1/op")
        );
        let checkpoint = Checkpoint::parse(&checkpoint_json(&key(9), 41, [0x11; 32])).unwrap();
        assert_eq!(checkpoint.signer().unwrap(), address_of(&key(9)));
        let mut raw_v = checkpoint.clone();
        raw_v.signature[64] -= 27;
        assert!(raw_v.signer().unwrap_err().contains("v must be 27 or 28"));
        let mut moved = checkpoint;
        moved.index = 40;
        assert_ne!(moved.signer().unwrap(), address_of(&key(9)));
        // A duplicate field fails, as for the relayer.
        let json = String::from_utf8(checkpoint_json(&key(9), 41, [0x11; 32])).unwrap();
        let doubled = json.replacen(
            "\"message_id\"",
            "\"message_id\":\"0x00\",\"message_id\"",
            1,
        );
        assert!(Checkpoint::parse(doubled.as_bytes())
            .unwrap_err()
            .contains("duplicate field"));
    }

    #[tokio::test]
    async fn locations_are_where_the_agents_keep_them_and_stay_in_their_bucket() {
        let s3 = Storage::parse("s3://dusk-hyperlane-testnet/nbg1/validator-1").unwrap();
        assert_eq!(
            s3,
            Storage::S3 {
                bucket: "dusk-hyperlane-testnet".into(),
                folder: Some("validator-1".into())
            }
        );
        assert_eq!(s3.checkpoint(2), "checkpoint_2_with_id.json");
        assert_eq!(
            Storage::parse("file:///v").unwrap().checkpoint(2),
            "2_with_id.json"
        );
        for bad in [
            "s3://bucket",
            "gs://b/f",
            "https://example.com/x",
            "s3://a://b/c",
        ] {
            assert!(Storage::parse(bad).is_err(), "{bad}");
        }
        assert!(parse_endpoint("https://user:pw@host").is_err());
        let endpoint = parse_endpoint("http://127.0.0.1:9/").unwrap();
        for location in [
            "s3://../nbg1/f",
            "s3://b/nbg1/a/../c",
            "s3://b/nbg1/a\t",
            "s3://b%2F/nbg1",
        ] {
            let read = Storage::parse(location)
                .unwrap()
                .read("index.json", Some(&endpoint))
                .await;
            assert!(read.unwrap_err().contains("can't have"), "{location}");
        }
    }
}
