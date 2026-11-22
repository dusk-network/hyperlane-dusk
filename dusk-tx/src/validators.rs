//! What a Hyperlane validator writes to its checkpoint storage: its signed
//! announcement and checkpoints, where an `s3://`, `gs://` or `file://`
//! location keeps them, and who signed them. The JSON is hyperlane-core's
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

/// The domain an S3 region's buckets are under: Hetzner's, DigitalOcean's,
/// or for any other region Amazon's, as the SDK names it. This mirrors the relayer's mapping in our
/// hyperlane-monorepo fork (`rust/main/hyperlane-base/src/types/s3_storage.rs`)
/// and must stay in sync with it.
fn s3_domain(region: &str) -> String {
    match region {
        "fsn1" | "nbg1" | "hel1" => format!("{region}.your-objectstorage.com"),
        "nyc3" | "ams3" | "sfo2" | "sfo3" | "sgp1" | "lon1" | "fra1" | "tor1" | "blr1" | "syd1"
        | "atl1" | "ric1" | "mkc1" => format!("{region}.digitaloceanspaces.com"),
        _ if region.starts_with("cn-") => format!("s3.{region}.amazonaws.com.cn"),
        _ => format!("s3.{region}.amazonaws.com"),
    }
}

/// A validator's checkpoint storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    /// `file://<path>`, a local rehearsal's.
    Local(PathBuf),
    /// `s3://bucket/region[/folder]` or `gs://bucket[/folder]/<key>`, read
    /// anonymously over HTTPS: the bucket's host, virtual-hosted on the
    /// provider the S3 region names, and the path to its objects.
    Remote {
        gcs: bool,
        host: String,
        path: Vec<String>,
    },
}

impl Storage {
    pub fn parse(location: &str) -> Result<Self, String> {
        if let Some(path) = location.strip_prefix("file://") {
            return Ok(Storage::Local(PathBuf::from(path)));
        }
        let bad = |why: &str| format!("Can't read the storage location '{location}': {why}");
        let expected =
            "expected s3://bucket/region[/folder], gs://bucket[/folder]/<key> or file://<path>";
        let (gcs, rest) = match location.split_once("://") {
            Some(("s3", rest)) => (false, rest),
            Some(("gs", rest)) => (true, rest),
            _ => return Err(bad(expected)),
        };
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 2 || rest.contains("://") || (gcs && parts.len() > 3) {
            return Err(bad(expected));
        }
        // The relayer ignores the GCS announcement's key after the folder.
        let (host, path) = if gcs {
            (
                "storage.googleapis.com".to_string(),
                &parts[..parts.len() - 1],
            )
        } else {
            let label = |s: &str| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            };
            let (bucket, region) = (parts[0], parts[1]);
            if !label(bucket) || !label(region) {
                return Err(bad("an S3 bucket and region have only lowercase letters, digits and hyphens here: a dot breaks the provider's certificate for the bucket's host"));
            }
            (format!("{bucket}.{}", s3_domain(region)), &parts[2..])
        };
        // Parts the URL keeps as they are, so it names the key the agent wrote.
        let plain = |s: &&str| {
            !matches!(*s, "" | "." | "..")
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        if !path.iter().all(plain) {
            return Err(bad("each part of its folder, and a GCS bucket, may have only letters, digits, '-', '_' and '.'"));
        }
        let path = path.iter().map(|s| s.to_string()).collect();
        Ok(Storage::Remote { gcs, host, path })
    }

    fn gcs(&self) -> bool {
        matches!(self, Storage::Remote { gcs: true, .. })
    }

    pub fn announcement(&self) -> &'static str {
        if self.gcs() {
            "gcsAnnouncementKey"
        } else {
            "announcement.json"
        }
    }

    pub fn latest_index(&self) -> &'static str {
        match self {
            Storage::Local(_) => "index.json",
            _ if self.gcs() => "gcsLatestIndexKey",
            _ => "checkpoint_latest_index.json",
        }
    }

    pub fn checkpoint(&self, index: u32) -> String {
        match self {
            Storage::Local(_) => format!("{index}_with_id.json"),
            _ => format!("checkpoint_{index}_with_id.json"),
        }
    }

    /// Whether the validator flagged a reorg: while it has, it signs nothing,
    /// and a flag where it announced stops the relayer delivering through an
    /// ISM that lists it.
    pub async fn reorg_flagged(&self) -> Result<bool, String> {
        if let Storage::Local(_) = self {
            return Ok(self.read("reorg_flag.json").await?.is_some());
        }
        for flag in self.reorg_flags()? {
            if read_url(&flag).await?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Where a remote validator's reorg flags are. One on GCS with a folder
    /// also stops signing on a flag at the bucket's root.
    pub fn reorg_flags(&self) -> Result<Vec<Url>, String> {
        match self {
            Storage::Remote {
                gcs: true,
                host,
                path,
            } => (1..=path.len())
                .map(|depth| url(host, &path[..depth], "gcsReorgFlagKey"))
                .collect(),
            _ => Ok(vec![self.url("reorg_flag.json")?]),
        }
    }

    /// The URL the object `name` is read from.
    pub fn url(&self, name: &str) -> Result<Url, String> {
        match self {
            Storage::Local(_) => Err("A file:// location has no URL".into()),
            Storage::Remote { host, path, .. } => url(host, path, name),
        }
    }

    /// Read the object `name`: its bytes, or `None` when it isn't there.
    pub async fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Storage::Local(dir) => read_file(&dir.join(name)),
            _ => read_url(&self.url(name)?).await,
        }
    }
}

fn url(host: &str, path: &[String], name: &str) -> Result<Url, String> {
    let mut url = Url::parse(&format!("https://{host}/"))
        .map_err(|e| format!("Can't read from {host}: {e}"))?;
    url.path_segments_mut()
        .map_err(|()| format!("Can't read from {host}"))?
        .pop_if_empty()
        .extend(path)
        .push(name);
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

    #[test]
    fn objects_are_read_where_each_provider_keeps_them() {
        // The announcement, the latest index, a checkpoint and the reorg
        // flags, where the agent writes them.
        let objects = |location: &str| {
            let s = Storage::parse(location)?;
            let mut urls = vec![
                s.url(s.announcement())?,
                s.url(s.latest_index())?,
                s.url(&s.checkpoint(7))?,
            ];
            urls.extend(s.reorg_flags()?);
            Ok::<_, String>(urls.into_iter().map(String::from).collect::<Vec<_>>())
        };
        let s3 = "announcement.json checkpoint_latest_index.json checkpoint_7_with_id.json reorg_flag.json";
        let gcs = "gcsAnnouncementKey gcsLatestIndexKey checkpoint_7_with_id.json gcsReorgFlagKey";
        for (location, base, names) in [
            ("s3://op/nbg1/v", "https://op.nbg1.your-objectstorage.com/v/", s3),
            ("s3://op/ams3/v/w", "https://op.ams3.digitaloceanspaces.com/v/w/", s3),
            ("s3://op/eu-west-1", "https://op.s3.eu-west-1.amazonaws.com/", s3),
            ("s3://op/no-such-region-9", "https://op.s3.no-such-region-9.amazonaws.com/", s3),
            ("s3://op/cn-north-1", "https://op.s3.cn-north-1.amazonaws.com.cn/", s3),
            ("gs://b.c/gcsAnnouncementKey", "https://storage.googleapis.com/b.c/", gcs),
            (
                "gs://hyperlane-testnet4-validator-0/sepolia/gcsAnnouncementKey",
                "https://storage.googleapis.com/hyperlane-testnet4-validator-0/",
                "sepolia/gcsAnnouncementKey sepolia/gcsLatestIndexKey sepolia/checkpoint_7_with_id.json gcsReorgFlagKey sepolia/gcsReorgFlagKey",
            ),
        ] {
            let expected: Vec<String> = names.split(' ').map(|name| format!("{base}{name}")).collect();
            assert_eq!(objects(location).unwrap(), expected, "{location}");
        }
        assert_eq!(
            Storage::parse("file:///v").unwrap().checkpoint(2),
            "2_with_id.json"
        );
        for (bad, why) in [
            ("s3://bucket", "expected"),
            ("gs://b", "expected"),
            ("gs://b/f/g/k", "expected"),
            ("https://example.com/x", "expected"),
            ("s3://a://b/c", "expected"),
            ("s3://my.bucket/eu-west-1", "a dot"),
            ("s3://B/nbg1", "a dot"),
            ("s3://b/nbg1/a/../c", "may have only"),
            ("s3://b/nbg1/a\t", "may have only"),
            ("s3://b/nbg1/", "may have only"),
            ("s3://b/nbg1/blue+green", "may have only"),
            ("gs://../f/k", "may have only"),
            ("gs://b/100%/k", "may have only"),
        ] {
            assert!(Storage::parse(bad).unwrap_err().contains(why), "{bad}");
        }
    }
}
