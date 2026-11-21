//! What a Hyperlane validator writes to its checkpoint storage: its signed
//! announcement and signed checkpoints, where a storage location keeps
//! them, and who signed them.
//!
//! The JSON is Hyperlane's `SignedType` (hyperlane-core): the signed value,
//! its signature as `{r, s, v}` and as `serialized_signature`. A location is
//! what the validator announces: `s3://bucket/region[/folder]`,
//! `gs://bucket[/folder]/<key>` or `file://<path>`, parsed as the agents'
//! `CheckpointSyncerConf` parses it, with the object names each storage
//! backend uses.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hyperlane_dusk_types::checkpoint::{announcement_digest, checkpoint_digest};
use hyperlane_dusk_types::message::keccak256;
use hyperlane_dusk_types::signature::is_canonical;
use reqwest::Url;
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use secp256k1::{Message, Secp256k1};
use serde::Deserialize;

/// The ValidatorAnnounce's `MAX_LOCATION_BYTES`.
pub const MAX_LOCATION_BYTES: usize = 1_024;
/// The most this reads of one object. A validator writes a few hundred
/// bytes for an announcement, a checkpoint or its latest index.
const MAX_OBJECT_BYTES: usize = 64 * 1024;
/// How long one read of a storage object may take.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A validator's checkpoint storage, from the location it announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    /// `file://<path>`: a directory on this host.
    Local(PathBuf),
    /// `s3://bucket/region[/folder]`.
    S3 {
        bucket: String,
        region: String,
        folder: Option<String>,
    },
    /// `gs://bucket[/folder]/<key>`: the agents ignore what follows the
    /// folder.
    Gcs {
        bucket: String,
        folder: Option<String>,
    },
}

/// Where to read one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Url(Url),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::File(path) => write!(f, "{}", path.display()),
            Source::Url(url) => write!(f, "{url}"),
        }
    }
}

impl Storage {
    /// Parse a location as `CheckpointSyncerConf::from_str` does.
    pub fn parse(location: &str) -> Result<Self, String> {
        let parts: Vec<&str> = location.split("://").collect();
        let [prefix, suffix] = parts[..] else {
            return Err(format!(
                "Can't read the storage location '{location}': expected <scheme>://<path>"
            ));
        };
        match prefix {
            "s3" => {
                let components: Vec<&str> = suffix.split('/').collect();
                if components.len() < 2 {
                    return Err(format!(
                        "Can't read the storage location '{location}': expected s3://bucket/region[/folder]"
                    ));
                }
                let folder = components[2..].join("/");
                Ok(Storage::S3 {
                    bucket: components[0].to_owned(),
                    region: components[1].to_owned(),
                    folder: (!folder.is_empty()).then_some(folder),
                })
            }
            "gs" => {
                let components: Vec<&str> = suffix.split('/').collect();
                match components.len() {
                    2 => Ok(Storage::Gcs {
                        bucket: components[0].to_owned(),
                        folder: None,
                    }),
                    3 => Ok(Storage::Gcs {
                        bucket: components[0].to_owned(),
                        folder: Some(components[1].to_owned()),
                    }),
                    _ => Err(format!(
                        "Can't read the storage location '{location}': expected gs://bucket[/folder]/<key>"
                    )),
                }
            }
            "file" => Ok(Storage::Local(PathBuf::from(suffix))),
            _ => Err(format!(
                "Can't read the storage location '{location}': the agents know s3://, gs:// and file://, not {prefix}://"
            )),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Storage::Local(_))
    }

    /// Where the validator keeps its signed announcement.
    pub fn announcement(&self, s3_endpoint: Option<&Url>) -> Result<Source, String> {
        self.object(
            match self {
                Storage::Gcs { .. } => "gcsAnnouncementKey",
                _ => "announcement.json",
            },
            s3_endpoint,
        )
    }

    /// Where the validator keeps the index of its latest checkpoint.
    pub fn latest_index(&self, s3_endpoint: Option<&Url>) -> Result<Source, String> {
        self.object(
            match self {
                Storage::Local(_) => "index.json",
                Storage::S3 { .. } => "checkpoint_latest_index.json",
                Storage::Gcs { .. } => "gcsLatestIndexKey",
            },
            s3_endpoint,
        )
    }

    /// Where the validator flags a reorg it saw. The relayer refuses a
    /// validator whose storage has one.
    pub fn reorg_flag(&self, s3_endpoint: Option<&Url>) -> Result<Source, String> {
        self.object(
            match self {
                Storage::Gcs { .. } => "gcsReorgFlagKey",
                _ => "reorg_flag.json",
            },
            s3_endpoint,
        )
    }

    /// Where the validator keeps its checkpoint at `index`.
    pub fn checkpoint(&self, index: u32, s3_endpoint: Option<&Url>) -> Result<Source, String> {
        let name = match self {
            Storage::Local(_) => format!("{index}_with_id.json"),
            _ => format!("checkpoint_{index}_with_id.json"),
        };
        self.object(&name, s3_endpoint)
    }

    /// The object `name` of this storage. S3 objects are read path-style
    /// from `s3_endpoint` when there is one, as an agent with
    /// AWS_ENDPOINT_URL set reads them, and otherwise from AWS. Both
    /// buckets are read anonymously: a validator's checkpoints are public.
    fn object(&self, name: &str, s3_endpoint: Option<&Url>) -> Result<Source, String> {
        match self {
            Storage::Local(path) => Ok(Source::File(path.join(name))),
            Storage::S3 {
                bucket,
                region,
                folder,
            } => {
                check_bucket(bucket)?;
                let base = match s3_endpoint {
                    Some(endpoint) => endpoint.clone(),
                    None => {
                        if region.is_empty()
                            || !region.bytes().all(|byte| {
                                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                            })
                        {
                            return Err(format!("'{region}' isn't an AWS region"));
                        }
                        let domain = if region.starts_with("cn-") {
                            "amazonaws.com.cn"
                        } else {
                            "amazonaws.com"
                        };
                        Url::parse(&format!("https://s3.{region}.{domain}/"))
                            .map_err(|e| format!("Can't make the URL of region {region}: {e}"))?
                    }
                };
                object_url(base, bucket, folder.as_deref(), name)
            }
            Storage::Gcs { bucket, folder } => {
                check_bucket(bucket)?;
                let base = Url::parse("https://storage.googleapis.com/").expect("valid URL");
                object_url(base, bucket, folder.as_deref(), name)
            }
        }
    }
}

/// A bucket name stays one path segment of the URL it is read from.
fn check_bucket(bucket: &str) -> Result<(), String> {
    if bucket.is_empty()
        || bucket == "."
        || bucket == ".."
        || !bucket
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(format!("'{bucket}' isn't a bucket name"));
    }
    Ok(())
}

fn object_url(
    mut base: Url,
    bucket: &str,
    folder: Option<&str>,
    name: &str,
) -> Result<Source, String> {
    let segments: Vec<&str> = folder
        .into_iter()
        .flat_map(|folder| folder.split('/'))
        .chain(std::iter::once(name))
        .collect();
    // The URL parser drops tabs and line breaks before it resolves `.` and
    // `..`, so a part that is neither until then could still leave the
    // bucket: only plain characters are taken.
    if segments.iter().any(|segment| {
        segment.is_empty()
            || *segment == "."
            || *segment == ".."
            || segment
                .chars()
                .any(|c| c.is_control() || c == '\\' || c == '%')
    }) {
        return Err(format!(
            "The folder '{}' has an empty, '.' or '..' part, or a character a key can't have here",
            folder.unwrap_or_default()
        ));
    }
    let shown = base.to_string();
    let prefix: Vec<String> = base
        .path_segments()
        .map(|parts| {
            parts
                .filter(|part| !part.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    {
        let mut path = base
            .path_segments_mut()
            .map_err(|()| format!("{shown} can't take a path"))?;
        path.pop_if_empty().push(bucket).extend(&segments);
    }
    let path: Vec<&str> = base
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    if path.len() != prefix.len() + 1 + segments.len()
        || path[..prefix.len()] != prefix[..]
        || path[prefix.len()] != bucket
    {
        return Err(format!(
            "{base} isn't an object of the bucket {bucket} at {shown}"
        ));
    }
    Ok(Source::Url(base))
}

/// Parse an S3 endpoint as the agents' AWS_ENDPOINT_URL takes it.
pub fn parse_endpoint(endpoint: &str) -> Result<Url, String> {
    let url = Url::parse(endpoint).map_err(|e| format!("Invalid S3 endpoint '{endpoint}': {e}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(format!(
            "Invalid S3 endpoint '{endpoint}': expected http(s)://host[:port][/path]"
        ));
    }
    Ok(url)
}

/// Read one object: its bytes, or `None` when it isn't there. A URL is
/// read without following redirects.
pub async fn read(source: &Source) -> Result<Option<Vec<u8>>, String> {
    match source {
        Source::File(path) => read_file(path),
        Source::Url(url) => {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(READ_TIMEOUT)
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
                    "Can't read {url}: HTTP {}. A validator's storage must be readable by anyone.",
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
    }
}

fn read_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("{} isn't a regular file", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => {}
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Can't read {}: {e}", path.display())),
    };
    let mut body = Vec::new();
    file.take(MAX_OBJECT_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("Can't read {}: {e}", path.display()))?;
    if body.len() > MAX_OBJECT_BYTES {
        return Err(format!(
            "{} is over {MAX_OBJECT_BYTES} bytes",
            path.display()
        ));
    }
    Ok(Some(body))
}

// ── Signed values ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SignedJson<T> {
    #[serde(alias = "announcement", alias = "checkpoint")]
    value: T,
    signature: Option<SignatureJson>,
    serialized_signature: Option<String>,
}

#[derive(Deserialize)]
struct SignatureJson {
    r: String,
    s: String,
    v: u64,
}

#[derive(Deserialize)]
struct AnnouncementJson {
    validator: String,
    mailbox_address: String,
    mailbox_domain: u32,
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

/// A validator's signed announcement of where its checkpoints are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub validator: [u8; 20],
    pub mailbox: [u8; 32],
    pub domain: u32,
    pub location: String,
    pub signature: [u8; 65],
}

impl Announcement {
    pub fn parse(json: &[u8]) -> Result<Self, String> {
        let signed: SignedJson<AnnouncementJson> = serde_json::from_slice(json)
            .map_err(|e| format!("Not a validator's signed announcement: {e}"))?;
        let signature = signature_of(&signed)?;
        let value = signed.value;
        Ok(Announcement {
            validator: fixed_hex(&value.validator, "validator")?,
            mailbox: fixed_hex(&value.mailbox_address, "mailbox_address")?,
            domain: value.mailbox_domain,
            location: value.storage_location,
            signature,
        })
    }

    /// The validator signed it, and the ValidatorAnnounce takes its
    /// location: 1 to `MAX_LOCATION_BYTES` bytes.
    pub fn check(&self) -> Result<(), String> {
        if self.location.is_empty() || self.location.len() > MAX_LOCATION_BYTES {
            return Err(format!(
                "The announced location is {} bytes; a ValidatorAnnounce takes 1 to {MAX_LOCATION_BYTES}",
                self.location.len()
            ));
        }
        let digest = announcement_digest(self.domain, &self.mailbox, &self.location);
        let signer = recover(&digest, &self.signature)?;
        if signer != self.validator {
            return Err(format!(
                "The announcement names the validator 0x{}, but 0x{} signed it",
                hex::encode(self.validator),
                hex::encode(signer)
            ));
        }
        Ok(())
    }
}

/// A validator's signed checkpoint: a Merkle root and index of an origin's
/// Merkle tree hook, with the ID of the message at that index.
#[derive(Debug, Clone, PartialEq, Eq)]
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
        let signed: SignedJson<CheckpointWithIdJson> = serde_json::from_slice(json)
            .map_err(|e| format!("Not a validator's signed checkpoint: {e}"))?;
        // The relayer reads `signature`, and refuses a checkpoint without it.
        if signed.signature.is_none() {
            return Err("The checkpoint has no `signature`, which the relayer reads".into());
        }
        let signature = signature_of(&signed)?;
        let value = signed.value;
        Ok(Checkpoint {
            merkle_tree_hook: fixed_hex(
                &value.checkpoint.merkle_tree_hook_address,
                "merkle_tree_hook_address",
            )?,
            domain: value.checkpoint.mailbox_domain,
            root: fixed_hex(&value.checkpoint.root, "root")?,
            index: value.checkpoint.index,
            message_id: fixed_hex(&value.message_id, "message_id")?,
            signature,
        })
    }

    /// Who signed it.
    pub fn signer(&self) -> Result<[u8; 20], String> {
        let digest = checkpoint_digest(
            self.domain,
            &self.merkle_tree_hook,
            &self.root,
            self.index,
            &self.message_id,
        );
        recover(&digest, &self.signature)
    }
}

/// A validator's latest index, as it writes it: a JSON number.
pub fn parse_latest_index(json: &[u8]) -> Result<u32, String> {
    serde_json::from_slice(json).map_err(|e| format!("Not a checkpoint index: {e}"))
}

/// The 65-byte signature, from `serialized_signature`, else from `{r, s,
/// v}`. When both are there they must agree.
fn signature_of<T>(signed: &SignedJson<T>) -> Result<[u8; 65], String> {
    let serialized = signed
        .serialized_signature
        .as_deref()
        .map(|hex| fixed_hex::<65>(hex, "serialized_signature"))
        .transpose()?;
    let parts = signed
        .signature
        .as_ref()
        .map(|parts| {
            let mut signature = [0u8; 65];
            signature[..32].copy_from_slice(&u256_hex(&parts.r, "signature.r")?);
            signature[32..64].copy_from_slice(&u256_hex(&parts.s, "signature.s")?);
            signature[64] = u8::try_from(parts.v)
                .map_err(|_| format!("signature.v {} is out of range", parts.v))?;
            Ok::<_, String>(signature)
        })
        .transpose()?;
    match (serialized, parts) {
        (Some(serialized), Some(parts)) if serialized != parts => {
            Err("serialized_signature and signature disagree".to_string())
        }
        (Some(signature), _) | (None, Some(signature)) => Ok(signature),
        (None, None) => Err("No signature".to_string()),
    }
}

/// `0x` and exactly N bytes of hex.
fn fixed_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N], String> {
    let digits = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{field} '{value}' isn't 0x-prefixed hex"))?;
    if digits.len() != 2 * N {
        return Err(format!("{field} '{value}' isn't {N} bytes of hex"));
    }
    let mut bytes = [0u8; N];
    hex::decode_to_slice(digits, &mut bytes).map_err(|e| format!("{field} '{value}': {e}"))?;
    Ok(bytes)
}

/// A U256 as Hyperlane serializes one: `0x` and up to 64 hex digits,
/// without leading zeros.
fn u256_hex(value: &str, field: &str) -> Result<[u8; 32], String> {
    let digits = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{field} '{value}' isn't 0x-prefixed hex"))?;
    if digits.is_empty() || digits.len() > 64 {
        return Err(format!("{field} '{value}' isn't a 256-bit number"));
    }
    let padded = format!("{digits:0>64}");
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(padded, &mut bytes).map_err(|e| format!("{field} '{value}': {e}"))?;
    Ok(bytes)
}

/// The address that signed `digest`, for a signature the contracts accept:
/// `v` 27 or 28 and `s` in the lower half of the order
/// (`hyperlane_dusk_types::signature::is_canonical`).
pub fn recover(digest: &[u8; 32], signature: &[u8; 65]) -> Result<[u8; 20], String> {
    if !is_canonical(signature) {
        return Err(
            "The signature isn't one the contracts accept: v must be 27 or 28 and s in the lower half"
                .to_string(),
        );
    }
    let id = RecoveryId::try_from(i32::from(signature[64] - 27))
        .map_err(|e| format!("Invalid recovery ID: {e}"))?;
    let signature = RecoverableSignature::from_compact(&signature[..64], id)
        .map_err(|e| format!("Invalid signature: {e}"))?;
    let public = Secp256k1::verification_only()
        .recover_ecdsa(Message::from_digest(*digest), &signature)
        .map_err(|e| format!("Can't recover the signer: {e}"))?
        .serialize_uncompressed();
    let hash = keccak256(&public[1..]);
    let mut address = [0u8; 20];
    address.copy_from_slice(&hash[12..]);
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secp256k1::SecretKey;

    const MAILBOX: [u8; 32] = [0xA1; 32];
    const HOOK: [u8; 32] = [0xA4; 32];
    const DOMAIN: u32 = 1_146_442_572;

    fn key(byte: u8) -> SecretKey {
        SecretKey::from_byte_array([byte; 32]).unwrap()
    }

    fn address_of(secret: &SecretKey) -> [u8; 20] {
        let public = secp256k1::PublicKey::from_secret_key(&Secp256k1::new(), secret)
            .serialize_uncompressed();
        keccak256(&public[1..])[12..].try_into().unwrap()
    }

    fn sign(secret: &SecretKey, digest: [u8; 32]) -> [u8; 65] {
        let signature =
            Secp256k1::new().sign_ecdsa_recoverable(Message::from_digest(digest), secret);
        let (id, compact) = signature.serialize_compact();
        let mut encoded = [0u8; 65];
        encoded[..64].copy_from_slice(&compact);
        encoded[64] = u8::try_from(i32::from(id)).unwrap() + 27;
        encoded
    }

    /// The JSON a validator writes, with the signature in both forms, as
    /// hyperlane-core serializes `SignedType`: r and s without leading
    /// zeros.
    fn signed_json(value: serde_json::Value, signature: &[u8; 65]) -> Vec<u8> {
        let number = |bytes: &[u8]| {
            let digits = hex::encode(bytes);
            let digits = digits.trim_start_matches('0');
            format!("0x{}", if digits.is_empty() { "0" } else { digits })
        };
        serde_json::to_vec_pretty(&serde_json::json!({
            "value": value,
            "signature": {
                "r": number(&signature[..32]),
                "s": number(&signature[32..64]),
                "v": signature[64],
            },
            "serialized_signature": format!("0x{}", hex::encode(signature)),
        }))
        .unwrap()
    }

    fn announcement_json(secret: &SecretKey, location: &str) -> Vec<u8> {
        let signature = sign(secret, announcement_digest(DOMAIN, &MAILBOX, location));
        signed_json(
            serde_json::json!({
                "validator": format!("0x{}", hex::encode(address_of(secret))),
                "mailbox_address": format!("0x{}", hex::encode(MAILBOX)),
                "mailbox_domain": DOMAIN,
                "storage_location": location,
            }),
            &signature,
        )
    }

    fn checkpoint_json(secret: &SecretKey, index: u32, message_id: [u8; 32]) -> Vec<u8> {
        let root = [0x5A; 32];
        let signature = sign(
            secret,
            checkpoint_digest(DOMAIN, &HOOK, &root, index, &message_id),
        );
        signed_json(
            serde_json::json!({
                "checkpoint": {
                    "merkle_tree_hook_address": format!("0x{}", hex::encode(HOOK)),
                    "mailbox_domain": DOMAIN,
                    "root": format!("0x{}", hex::encode(root)),
                    "index": index,
                },
                "message_id": format!("0x{}", hex::encode(message_id)),
            }),
            &signature,
        )
    }

    #[test]
    fn a_validators_announcement_checks_out() {
        let secret = key(7);
        let parsed =
            Announcement::parse(&announcement_json(&secret, "s3://bucket/nbg1/op")).unwrap();
        assert_eq!(parsed.validator, address_of(&secret));
        assert_eq!(parsed.mailbox, MAILBOX);
        assert_eq!(parsed.domain, DOMAIN);
        assert_eq!(parsed.location, "s3://bucket/nbg1/op");
        parsed.check().unwrap();
    }

    #[test]
    fn an_announcement_naming_another_validator_is_refused() {
        let json = announcement_json(&key(7), "s3://bucket/nbg1/op");
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["value"]["validator"] = format!("0x{}", hex::encode(address_of(&key(8)))).into();
        let parsed = Announcement::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        let error = parsed.check().unwrap_err();
        assert!(error.contains("signed it"), "{error}");
    }

    #[test]
    fn an_announcement_for_another_location_or_mailbox_is_refused() {
        let secret = key(7);
        let mut parsed =
            Announcement::parse(&announcement_json(&secret, "s3://bucket/nbg1/op")).unwrap();
        parsed.location = "s3://bucket/nbg1/other".into();
        assert!(parsed.check().is_err());
        let mut parsed =
            Announcement::parse(&announcement_json(&secret, "s3://bucket/nbg1/op")).unwrap();
        parsed.mailbox = [0xA2; 32];
        assert!(parsed.check().is_err());
        let mut parsed =
            Announcement::parse(&announcement_json(&secret, "s3://bucket/nbg1/op")).unwrap();
        parsed.domain += 1;
        assert!(parsed.check().is_err());
    }

    #[test]
    fn a_location_the_contract_refuses_is_refused() {
        let secret = key(7);
        let long = format!("s3://bucket/nbg1/{}", "a".repeat(MAX_LOCATION_BYTES));
        let error = Announcement::parse(&announcement_json(&secret, &long))
            .unwrap()
            .check()
            .unwrap_err();
        assert!(error.contains("1 to 1024"), "{error}");
        let empty = Announcement::parse(&announcement_json(&secret, ""))
            .unwrap()
            .check()
            .unwrap_err();
        assert!(empty.contains("1 to 1024"), "{empty}");
    }

    #[test]
    fn a_signature_the_contracts_refuse_is_refused() {
        let secret = key(7);
        let json = announcement_json(&secret, "s3://bucket/nbg1/op");
        let mut parsed = Announcement::parse(&json).unwrap();
        // v as a raw recovery ID, which OpenZeppelin's ECDSA refuses.
        parsed.signature[64] -= 27;
        assert!(parsed.check().unwrap_err().contains("v must be 27 or 28"));
        // The same signature with s in the upper half.
        let mut parsed = Announcement::parse(&json).unwrap();
        let order = secp256k1::constants::CURVE_ORDER;
        let mut s = [0u8; 32];
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let value = i16::from(order[i]) - i16::from(parsed.signature[32 + i]) - borrow;
            borrow = i16::from(value < 0);
            s[i] = u8::try_from(value.rem_euclid(256)).unwrap();
        }
        parsed.signature[32..64].copy_from_slice(&s);
        parsed.signature[64] = if parsed.signature[64] == 27 { 28 } else { 27 };
        assert!(parsed.check().unwrap_err().contains("lower half"));
    }

    #[test]
    fn the_signature_comes_from_either_form_and_both_must_agree() {
        let secret = key(7);
        let json = announcement_json(&secret, "s3://bucket/nbg1/op");
        let full: serde_json::Value = serde_json::from_slice(&json).unwrap();
        for drop in ["signature", "serialized_signature"] {
            let mut value = full.clone();
            value.as_object_mut().unwrap().remove(drop);
            Announcement::parse(&serde_json::to_vec(&value).unwrap())
                .unwrap()
                .check()
                .unwrap();
        }
        let mut value = full.clone();
        value["signature"]["v"] = if full["signature"]["v"] == 27 { 28 } else { 27 }.into();
        let error = Announcement::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(error.contains("disagree"), "{error}");
        let mut value = full;
        value.as_object_mut().unwrap().remove("signature");
        value
            .as_object_mut()
            .unwrap()
            .remove("serialized_signature");
        assert!(Announcement::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn a_checkpoint_names_its_signer() {
        let secret = key(9);
        let parsed = Checkpoint::parse(&checkpoint_json(&secret, 41, [0x11; 32])).unwrap();
        assert_eq!(parsed.index, 41);
        assert_eq!(parsed.domain, DOMAIN);
        assert_eq!(parsed.merkle_tree_hook, HOOK);
        assert_eq!(parsed.message_id, [0x11; 32]);
        assert_eq!(parsed.signer().unwrap(), address_of(&secret));
        let mut moved = parsed.clone();
        moved.index = 40;
        assert_ne!(moved.signer().unwrap(), address_of(&secret));
        let mut other = parsed;
        other.message_id = [0x12; 32];
        assert_ne!(other.signer().unwrap(), address_of(&secret));
    }

    #[test]
    fn a_checkpoint_needs_the_signature_the_relayer_reads() {
        let json = checkpoint_json(&key(9), 1, [0x11; 32]);
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value.as_object_mut().unwrap().remove("signature");
        let error = Checkpoint::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
        assert!(error.contains("relayer"), "{error}");
    }

    #[test]
    fn fields_must_be_fixed_width_hex() {
        let secret = key(9);
        let json = checkpoint_json(&secret, 1, [0x11; 32]);
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["value"]["message_id"] = "0x11".into();
        assert!(Checkpoint::parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        value["value"]["checkpoint"]["root"] = hex::encode([0x5A; 32]).into();
        assert!(Checkpoint::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn locations_parse_as_the_agents_parse_them() {
        assert_eq!(
            Storage::parse("s3://dusk-hyperlane-testnet/nbg1/validator-1").unwrap(),
            Storage::S3 {
                bucket: "dusk-hyperlane-testnet".into(),
                region: "nbg1".into(),
                folder: Some("validator-1".into()),
            }
        );
        assert_eq!(
            Storage::parse("s3://bucket/us-east-1").unwrap(),
            Storage::S3 {
                bucket: "bucket".into(),
                region: "us-east-1".into(),
                folder: None,
            }
        );
        assert_eq!(
            Storage::parse("s3://bucket/eu-west-1/a/b").unwrap(),
            Storage::S3 {
                bucket: "bucket".into(),
                region: "eu-west-1".into(),
                folder: Some("a/b".into()),
            }
        );
        assert_eq!(
            Storage::parse("gs://hyperlane-testnet4-validator-0/sepolia/gcsAnnouncementKey")
                .unwrap(),
            Storage::Gcs {
                bucket: "hyperlane-testnet4-validator-0".into(),
                folder: Some("sepolia".into()),
            }
        );
        assert_eq!(
            Storage::parse("gs://bucket/gcsAnnouncementKey").unwrap(),
            Storage::Gcs {
                bucket: "bucket".into(),
                folder: None,
            }
        );
        assert_eq!(
            Storage::parse("file:///var/lib/checkpoints").unwrap(),
            Storage::Local("/var/lib/checkpoints".into())
        );
        for bad in [
            "s3://bucket",
            "gs://bucket",
            "gs://a/b/c/d",
            "https://example.com/x",
            "s3:/bucket/region",
            "s3://a://b",
        ] {
            assert!(Storage::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn objects_are_read_where_each_backend_writes_them() {
        let endpoint = parse_endpoint("https://nbg1.your-objectstorage.com").unwrap();
        let s3 = Storage::parse("s3://dusk-hyperlane-testnet/nbg1/validator-1").unwrap();
        assert_eq!(
            s3.latest_index(Some(&endpoint)).unwrap().to_string(),
            "https://nbg1.your-objectstorage.com/dusk-hyperlane-testnet/validator-1/checkpoint_latest_index.json"
        );
        assert_eq!(
            s3.checkpoint(7, Some(&endpoint)).unwrap().to_string(),
            "https://nbg1.your-objectstorage.com/dusk-hyperlane-testnet/validator-1/checkpoint_7_with_id.json"
        );
        assert_eq!(
            s3.announcement(Some(&endpoint)).unwrap().to_string(),
            "https://nbg1.your-objectstorage.com/dusk-hyperlane-testnet/validator-1/announcement.json"
        );
        let aws = Storage::parse("s3://bucket/us-east-1").unwrap();
        assert_eq!(
            aws.latest_index(None).unwrap().to_string(),
            "https://s3.us-east-1.amazonaws.com/bucket/checkpoint_latest_index.json"
        );
        let china = Storage::parse("s3://bucket/cn-north-1/f").unwrap();
        assert_eq!(
            china.announcement(None).unwrap().to_string(),
            "https://s3.cn-north-1.amazonaws.com.cn/bucket/f/announcement.json"
        );
        let gcs = Storage::parse("gs://hyperlane-testnet4-validator-0/sepolia/gcsAnnouncementKey")
            .unwrap();
        assert_eq!(
            gcs.latest_index(None).unwrap().to_string(),
            "https://storage.googleapis.com/hyperlane-testnet4-validator-0/sepolia/gcsLatestIndexKey"
        );
        assert_eq!(
            gcs.checkpoint(3, None).unwrap().to_string(),
            "https://storage.googleapis.com/hyperlane-testnet4-validator-0/sepolia/checkpoint_3_with_id.json"
        );
        let local = Storage::parse("file:///tmp/v").unwrap();
        assert_eq!(
            local.latest_index(None).unwrap(),
            Source::File("/tmp/v/index.json".into())
        );
        assert_eq!(
            local.checkpoint(2, None).unwrap(),
            Source::File("/tmp/v/2_with_id.json".into())
        );
        // An endpoint with a path keeps it, as path-style S3 does.
        let proxied = parse_endpoint("http://127.0.0.1:9000/s3/").unwrap();
        assert_eq!(
            s3.latest_index(Some(&proxied)).unwrap().to_string(),
            "http://127.0.0.1:9000/s3/dusk-hyperlane-testnet/validator-1/checkpoint_latest_index.json"
        );
    }

    #[test]
    fn a_location_cant_leave_its_bucket() {
        for location in [
            "s3://../nbg1/f",
            "s3://bucket/nbg1/../other",
            "s3://bucket/nbg1/a//b",
            "s3://buck%2Fet/nbg1",
            "s3://bucket/no.such.region",
            "s3://bucket/nbg1/.\t.",
            "s3://bucket/nbg1/a\n/..",
            "s3://bucket/nbg1/%2e%2e",
            "s3://bucket/nbg1/a\\b",
        ] {
            let storage = Storage::parse(location).unwrap();
            assert!(storage.latest_index(None).is_err(), "{location}");
        }
        for endpoint in [
            "ftp://host",
            "https://user:pw@host",
            "https://host/?x=1",
            "host",
        ] {
            assert!(parse_endpoint(endpoint).is_err(), "{endpoint}");
        }
    }

    #[test]
    fn missing_files_read_as_none_and_large_ones_fail() {
        let dir = std::env::temp_dir().join(format!("dusk-tx-validators-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_file(&dir.join("index.json")).unwrap(), None);
        std::fs::write(dir.join("index.json"), b"42").unwrap();
        let bytes = read_file(&dir.join("index.json")).unwrap().unwrap();
        assert_eq!(parse_latest_index(&bytes).unwrap(), 42);
        std::fs::write(dir.join("big.json"), vec![b' '; MAX_OBJECT_BYTES + 1]).unwrap();
        assert!(read_file(&dir.join("big.json")).is_err());
        // A directory, or anything else that isn't a regular file, is refused.
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert!(read_file(&dir.join("sub")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
