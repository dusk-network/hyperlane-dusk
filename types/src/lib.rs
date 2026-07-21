// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane protocol types for Dusk.

#![no_std]
#![deny(missing_docs)]
#![deny(clippy::pedantic)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::module_name_repetitions)]

//! Shared types for the Hyperlane protocol on Dusk.
//!
//! This crate provides the core message encoding, event types, and utilities
//! used by all Hyperlane Dusk contracts.

extern crate alloc;

use bytecheck::CheckBytes;
use rkyv::{Archive, Deserialize, Serialize};

pub mod checkpoint;
pub mod caller;
#[cfg(feature = "drc20")]
pub mod drc20;
pub mod events;
pub mod merkle;
pub mod message;
pub mod metadata;
pub mod token_message;

/// Hyperlane protocol version.
pub const VERSION: u8 = 3;

/// A 32-byte identifier used for addresses in the Hyperlane protocol.
///
/// On EVM chains this is a 20-byte address left-padded to 32 bytes.
/// On Dusk this maps directly to a `ContractId` (which is 32 bytes).
pub type H256 = [u8; 32];

/// A message identifier, computed as `keccak256(encoded_message)`.
pub type MessageId = H256;

/// An Ethereum-style address (20 bytes), used for validator identifiers.
#[derive(
    Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Archive, Serialize, Deserialize,
)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EthAddress(pub [u8; 20]);

/// A validator checkpoint signed by validators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Checkpoint {
    /// The origin domain.
    pub origin: u32,
    /// The address of the merkle tree hook contract (as bytes32).
    pub merkle_tree_hook: H256,
    /// The merkle root at the checkpoint.
    pub root: H256,
    /// The index (tree count - 1) at the checkpoint.
    pub index: u32,
    /// The message ID at the checkpoint.
    pub message_id: MessageId,
}

/// Record of a delivered message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeliveryRecord {
    /// Who processed the message (the tx sender's contract or external address).
    pub block_height: u64,
}

/// ISM module types, matching the Solidity `IInterchainSecurityModule.Types` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IsmType {
    /// Unused.
    Unused = 0,
    /// Route to ISM based on origin.
    Routing = 1,
    /// Aggregate multiple ISMs.
    Aggregation = 2,
    /// Legacy multisig.
    LegacyMultisig = 3,
    /// Merkle root multisig.
    MerkleRootMultisig = 4,
    /// Message ID multisig.
    MessageIdMultisig = 5,
    /// Null ISM (no verification).
    Null = 6,
    /// CCIP read.
    CcipRead = 7,
    /// Weighted multisig.
    WeightedMultisig = 12,
}

/// Post-dispatch hook types, matching the Solidity `IPostDispatchHook.Types` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HookType {
    /// Unused.
    Unused = 0,
    /// Routing hook.
    Routing = 1,
    /// Aggregation hook.
    Aggregation = 2,
    /// Merkle tree hook.
    MerkleTree = 3,
    /// Interchain gas paymaster.
    Igp = 4,
    /// Fallback routing hook.
    FallbackRouting = 5,
    /// Protocol fee hook.
    ProtocolFee = 6,
}

/// Per-domain gas configuration for the IGP hook.
///
/// Stores the gas oracle data (exchange rate + gas price) and a fixed
/// overhead for each remote domain.
///
/// All fields use `u64` to avoid cross-architecture rkyv alignment issues
/// with `u128` (which has different alignment on `x86_64` vs `wasm32`).
/// The IGP contract casts to `u128` during the cost calculation to prevent
/// intermediate overflow.
#[derive(
    Default, Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize,
)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DomainGasConfig {
    /// Overhead gas added to every message (mailbox + ISM verification cost).
    pub gas_overhead: u64,
    /// Token exchange rate, scaled by 1e10 (i.e. 1e10 = 1:1 parity).
    pub token_exchange_rate: u64,
    /// Gas price on the remote chain (in remote chain's native denomination).
    pub gas_price: u64,
}

/// Stored gas payment record (for off-chain indexing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GasPaymentRecord {
    /// The message ID for which gas was paid.
    pub message_id: MessageId,
    /// Destination domain paid for.
    pub destination: u32,
    /// The gas amount paid for.
    pub gas_limit: u64,
    /// Payment amount (in LUX).
    pub payment: u64,
    /// Block height at which the payment was recorded.
    pub block_height: u64,
}
