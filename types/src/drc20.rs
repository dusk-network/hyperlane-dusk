// SPDX-License-Identifier: MIT OR Apache-2.0

//! DRC20 principal and call ABI.
//!
//! The archived layout intentionally matches `dusk-network/contracts`
//! `bc1b00ee0af059975e158b7b580b4d0c0f1bdf9f` exactly. Keep the compatibility
//! fixtures in this crate in sync before changing any variant or field.

use core::cmp::Ordering;

use alloc::vec::Vec;
use bytecheck::CheckBytes;
use dusk_core::abi::ContractId;
use rkyv::{Archive, Deserialize, Serialize};

/// Raw byte length of a Moonlight BLS public key.
pub const BLS_PUBLIC_KEY_BYTES: usize = 193;

/// Coarse principal kind used by the canonical JSON representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PrincipalKind {
    /// Transparent Moonlight public account.
    Moonlight,
    /// Privacy-preserving Phoenix authorization identity.
    Phoenix,
    /// Contract account.
    Contract,
}

/// A canonical DRC20 principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
pub enum Principal {
    /// Transparent Moonlight public account, encoded as raw BLS key bytes.
    Moonlight([u8; BLS_PUBLIC_KEY_BYTES]),
    /// Phoenix authorization identity, encoded as compressed Schnorr bytes.
    Phoenix([u8; 32]),
    /// Dusk contract account.
    Contract(ContractId),
}

/// Backward-compatible local name for the canonical principal type.
pub type Account = Principal;

/// The 32-byte key a principal's balance and allowances are stored under.
/// See [`account_id`].
pub type AccountId = [u8; 32];

/// Domain tag of [`account_id`]. It keeps account IDs apart from every other
/// keccak256 the contracts compute, such as recipient hashes and message IDs.
pub const ACCOUNT_ID_DOMAIN: &[u8] = b"hyperlane-dusk/drc20-account";

/// The ID a principal's balance and allowances are stored under:
/// `keccak256(ACCOUNT_ID_DOMAIN || kind || key bytes)`, where `kind || key
/// bytes` is [`Principal::to_bytes`].
///
/// `kind` is one byte, 0 for Moonlight, 1 for Phoenix and 2 for a contract.
/// The key bytes are the principal's as the DRC20 ABI carries them: the raw
/// 193-byte BLS key, the 32-byte Phoenix key or the 32-byte contract ID. So
/// two principals share an ID only if they are equal, as long as keccak256
/// resists collisions, and a future key format gets a kind byte, and IDs, of
/// its own. A stored ID is 32 bytes whatever the key's size.
#[must_use]
pub fn account_id(principal: &Principal) -> AccountId {
    let mut preimage = Vec::with_capacity(ACCOUNT_ID_DOMAIN.len() + 1 + BLS_PUBLIC_KEY_BYTES);
    preimage.extend_from_slice(ACCOUNT_ID_DOMAIN);
    principal.write_tagged(&mut preimage);
    crate::message::keccak256_vec(preimage)
}

impl Principal {
    /// Construct a Moonlight principal from a Dusk BLS public key.
    #[must_use]
    pub fn moonlight(public_key: &dusk_core::signatures::bls::PublicKey) -> Self {
        Self::Moonlight(public_key.to_raw_bytes())
    }

    /// Return the principal kind.
    #[must_use]
    pub const fn kind(&self) -> PrincipalKind {
        match self {
            Self::Moonlight(_) => PrincipalKind::Moonlight,
            Self::Phoenix(_) => PrincipalKind::Phoenix,
            Self::Contract(_) => PrincipalKind::Contract,
        }
    }

    /// Return true for the reserved all-zero principal value.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        match self {
            Self::Moonlight(bytes) => bytes.iter().all(|byte| *byte == 0),
            Self::Phoenix(bytes) => bytes.iter().all(|byte| *byte == 0),
            Self::Contract(contract) => contract.to_bytes().iter().all(|byte| *byte == 0),
        }
    }

    /// Stable tagged bytes used for hashing and replay keys: a kind byte (0
    /// Moonlight, 1 Phoenix, 2 contract), then the key bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_tagged(&mut out);
        out
    }

    /// Appends [`Self::to_bytes`] to `out`.
    fn write_tagged(&self, out: &mut Vec<u8>) {
        out.push(match self {
            Self::Moonlight(_) => 0,
            Self::Phoenix(_) => 1,
            Self::Contract(_) => 2,
        });
        match self {
            Self::Moonlight(bytes) => out.extend_from_slice(bytes),
            Self::Phoenix(bytes) => out.extend_from_slice(bytes),
            Self::Contract(contract) => out.extend_from_slice(&contract.to_bytes()),
        }
    }
}

impl PartialOrd for Principal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Principal {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Moonlight(lhs), Self::Moonlight(rhs)) => lhs.cmp(rhs),
            (Self::Phoenix(lhs), Self::Phoenix(rhs)) => lhs.cmp(rhs),
            (Self::Contract(lhs), Self::Contract(rhs)) => lhs.cmp(rhs),
            _ => self.kind().cmp(&other.kind()),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Principal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("Principal", 2)?;
        state.serialize_field("kind", &self.kind())?;
        match self {
            Self::Moonlight(bytes) => state.serialize_field("bytes", bytes.as_slice())?,
            Self::Phoenix(bytes) => state.serialize_field("bytes", bytes.as_slice())?,
            Self::Contract(contract) => {
                state.serialize_field("bytes", contract.to_bytes().as_slice())?
            }
        }
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Principal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct PrincipalJson {
            kind: PrincipalKind,
            bytes: Vec<u8>,
        }

        let principal = <PrincipalJson as serde::Deserialize>::deserialize(deserializer)?;
        match principal.kind {
            PrincipalKind::Moonlight => {
                principal
                    .bytes
                    .try_into()
                    .map(Self::Moonlight)
                    .map_err(|bytes: Vec<u8>| {
                        serde::de::Error::invalid_length(
                            bytes.len(),
                            &"193 Moonlight public-key bytes",
                        )
                    })
            }
            PrincipalKind::Phoenix => {
                principal
                    .bytes
                    .try_into()
                    .map(Self::Phoenix)
                    .map_err(|bytes: Vec<u8>| {
                        serde::de::Error::invalid_length(
                            bytes.len(),
                            &"32 Phoenix public-key bytes",
                        )
                    })
            }
            PrincipalKind::Contract => principal
                .bytes
                .try_into()
                .map(|bytes| Self::Contract(ContractId::from_bytes(bytes)))
                .map_err(|bytes: Vec<u8>| {
                    serde::de::Error::invalid_length(bytes.len(), &"32 contract-id bytes")
                }),
        }
    }
}

/// Input for `balance_of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BalanceOf {
    /// Account to query.
    pub account: Principal,
}

/// Input for `allowance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Allowance {
    /// Token owner.
    pub owner: Principal,
    /// Approved spender.
    pub spender: Principal,
}

/// Input for `transfer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransferCall {
    /// Recipient.
    pub to: Principal,
    /// Amount to transfer.
    pub amount: u64,
}

/// Input for `approve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApproveCall {
    /// Approved spender.
    pub spender: Principal,
    /// Allowance amount.
    pub amount: u64,
}

/// Input for `transfer_from`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransferFromCall {
    /// Account whose allowance is consumed.
    pub owner: Principal,
    /// Recipient.
    pub to: Principal,
    /// Amount to transfer.
    pub amount: u64,
}

/// Resolve the current DRC20 caller as a canonical principal.
///
/// Phoenix callers have no stable runtime principal and are rejected. They
/// must use an explicitly signed, replay-protected authorization flow.
///
/// # Panics
///
/// Panics when the VM reports a root Moonlight call without its public sender,
/// or a nested call without a contract caller. Both indicate an invalid host
/// call context and must fail closed before an account principal is produced.
#[cfg(feature = "abi")]
#[must_use]
pub fn sender_account() -> Principal {
    use dusk_core::abi;
    use dusk_core::transfer::TRANSFER_CONTRACT;

    let caller = abi::caller();
    if caller == Some(TRANSFER_CONTRACT) && abi::callstack().len() <= 1 {
        let public_key = abi::public_sender().expect("DRC20: Moonlight public sender unavailable");
        Principal::moonlight(&public_key)
    } else {
        Principal::Contract(caller.expect("DRC20: missing caller"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::keccak256;

    #[test]
    fn account_id_hashes_the_domain_and_the_tagged_key() {
        for principal in [
            Principal::Moonlight([0x11; BLS_PUBLIC_KEY_BYTES]),
            Principal::Phoenix([0x22; 32]),
            Principal::Contract(ContractId::from_bytes([0x33; 32])),
        ] {
            let mut preimage = ACCOUNT_ID_DOMAIN.to_vec();
            preimage.extend_from_slice(&principal.to_bytes());
            assert_eq!(account_id(&principal), keccak256(&preimage));
        }
    }

    #[test]
    fn account_ids_differ_by_kind_and_by_every_key_byte() {
        // The same 32 bytes as a Phoenix key and as a contract.
        assert_ne!(
            account_id(&Principal::Phoenix([7; 32])),
            account_id(&Principal::Contract(ContractId::from_bytes([7; 32])))
        );
        // Moonlight keys that differ in their raw bytes are different
        // principals, and keep different balances, even when they share a
        // compressed encoding.
        let key = [0x44; BLS_PUBLIC_KEY_BYTES];
        let mut twin = key;
        twin[BLS_PUBLIC_KEY_BYTES - 2] ^= 1;
        assert_ne!(
            account_id(&Principal::Moonlight(key)),
            account_id(&Principal::Moonlight(twin))
        );
    }

    /// Pins the encoding for tools that compute IDs themselves. Checked
    /// with `cast keccak 0x<hex of the domain tag>02<32 bytes of 0x33>`.
    #[test]
    fn account_id_of_a_contract_is_stable() {
        assert_eq!(
            account_id(&Principal::Contract(ContractId::from_bytes([0x33; 32]))),
            [
                0x90, 0x2d, 0xe9, 0x7a, 0xeb, 0x85, 0xcb, 0x60, 0xfe, 0x10, 0x15, 0x67, 0x6a, 0x4f,
                0x84, 0x1b, 0xe8, 0x55, 0xab, 0x38, 0x18, 0x1b, 0x22, 0x74, 0x77, 0xfc, 0x00, 0xae,
                0x32, 0xd7, 0x15, 0xba,
            ]
        );
    }
}
