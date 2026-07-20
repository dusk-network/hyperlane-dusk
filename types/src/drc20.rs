// SPDX-License-Identifier: MIT OR Apache-2.0

//! Current Dusk DRC20 account and call types.

use core::cmp::Ordering;

#[cfg(feature = "serde")]
use alloc::vec::Vec;
use bytecheck::CheckBytes;
#[cfg(feature = "serde")]
use dusk_bytes::Serializable;
use dusk_core::abi::ContractId;
use dusk_core::signatures::bls::PublicKey;
use rkyv::{Archive, Deserialize, Serialize};

/// A DRC20 account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
pub enum Account {
    /// An externally owned Moonlight account.
    External(PublicKey),
    /// A contract account.
    Contract(ContractId),
}

#[cfg(feature = "serde")]
#[derive(serde::Serialize, serde::Deserialize)]
enum SerdeAccount {
    External(Vec<u8>),
    Contract([u8; 32]),
}

#[cfg(feature = "serde")]
impl serde::Serialize for Account {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::External(public_key) => serde::Serialize::serialize(
                &SerdeAccount::External(public_key.to_bytes().to_vec()),
                serializer,
            ),
            Self::Contract(contract) => serde::Serialize::serialize(
                &SerdeAccount::Contract(contract.to_bytes()),
                serializer,
            ),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Account {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match serde::Deserialize::deserialize(deserializer)? {
            SerdeAccount::External(bytes) => {
                let bytes: [u8; 96] = bytes.try_into().map_err(|_| {
                    serde::de::Error::custom("external DRC20 account must be a 96-byte public key")
                })?;
                let public_key = PublicKey::from_bytes(&bytes)
                    .map_err(|_| serde::de::Error::custom("invalid external DRC20 public key"))?;
                Ok(Self::External(public_key))
            }
            SerdeAccount::Contract(contract) => {
                Ok(Self::Contract(ContractId::from_bytes(contract)))
            }
        }
    }
}

impl PartialOrd for Account {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Account {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::External(lhs), Self::External(rhs)) => {
                lhs.to_raw_bytes().cmp(&rhs.to_raw_bytes())
            }
            (Self::Contract(lhs), Self::Contract(rhs)) => lhs.cmp(rhs),
            (Self::External(_), Self::Contract(_)) => Ordering::Less,
            (Self::Contract(_), Self::External(_)) => Ordering::Greater,
        }
    }
}

/// Input for `balance_of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BalanceOf {
    /// Account to query.
    pub account: Account,
}

/// Input for `allowance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Allowance {
    /// Token owner.
    pub owner: Account,
    /// Approved spender.
    pub spender: Account,
}

/// Input for `transfer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransferCall {
    /// Recipient.
    pub to: Account,
    /// Amount to transfer.
    pub value: u64,
}

/// Input for `approve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApproveCall {
    /// Approved spender.
    pub spender: Account,
    /// Allowance amount.
    pub value: u64,
}

/// Input for `transfer_from`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TransferFromCall {
    /// Account whose allowance is consumed.
    pub owner: Account,
    /// Recipient.
    pub to: Account,
    /// Amount to transfer.
    pub value: u64,
}

/// Resolve the transaction or contract caller as a DRC20 account.
///
/// # Panics
///
/// Panics for shielded root calls or malformed nested call frames.
#[cfg(feature = "abi")]
#[must_use]
pub fn sender_account() -> Account {
    use dusk_core::abi;

    if abi::callstack().len() == 1 {
        Account::External(abi::public_sender().expect("DRC20: shielded transactions not supported"))
    } else {
        Account::Contract(abi::caller().expect("DRC20: missing caller"))
    }
}
