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

    /// Stable tagged bytes used for hashing and replay keys.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
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
        out
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
