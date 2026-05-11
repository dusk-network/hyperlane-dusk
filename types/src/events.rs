// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Hyperlane event types for Dusk.

//! Event types emitted by Hyperlane contracts on Dusk.
//!
//! Each event struct has associated `TOPIC` constants used as the first
//! argument to `abi::emit(topic, data)`. The off-chain indexer subscribes
//! to these topics via RUES to track protocol state.

use alloc::string::String;
use alloc::vec::Vec;

use bytecheck::CheckBytes;
use rkyv::{Archive, Deserialize, Serialize};

use crate::{EthAddress, H256, MessageId};

// =========================================================================
// Mailbox events
// =========================================================================

/// Emitted when a message is dispatched via the Mailbox.
///
/// Matches the `Dispatch` event in `Mailbox.sol`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Dispatch {
    /// The sender of the dispatch call (msg.sender equivalent).
    pub sender: H256,
    /// The destination domain.
    pub destination: u32,
    /// The recipient address on the destination chain.
    pub recipient: H256,
    /// The full encoded Hyperlane message.
    pub message: Vec<u8>,
}

impl Dispatch {
    /// Event topic for message dispatch.
    pub const TOPIC: &'static str = "dispatch";
}

/// Emitted alongside `Dispatch` with just the message ID.
///
/// Matches the `DispatchId` event in `Mailbox.sol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DispatchId {
    /// The ID of the dispatched message.
    pub message_id: MessageId,
}

impl DispatchId {
    /// Event topic for message dispatch ID.
    pub const TOPIC: &'static str = "dispatch_id";
}

/// Emitted when a message is processed (delivered) by the Mailbox.
///
/// Matches the `Process` event in `Mailbox.sol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Process {
    /// The origin domain of the message.
    pub origin: u32,
    /// The sender address on the origin chain.
    pub sender: H256,
    /// The recipient address on this chain.
    pub recipient: H256,
}

impl Process {
    /// Event topic for message processing.
    pub const TOPIC: &'static str = "process";
}

/// Emitted alongside `Process` with just the message ID.
///
/// Matches the `ProcessId` event in `Mailbox.sol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessId {
    /// The ID of the processed message.
    pub message_id: MessageId,
}

impl ProcessId {
    /// Event topic for message process ID.
    pub const TOPIC: &'static str = "process_id";
}

// =========================================================================
// Mailbox admin events
// =========================================================================

/// Emitted when the default ISM is updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DefaultIsmSet {
    /// The new default ISM contract ID.
    pub module: H256,
}

impl DefaultIsmSet {
    /// Event topic.
    pub const TOPIC: &'static str = "default_ism_set";
}

/// Emitted when the default hook is updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DefaultHookSet {
    /// The new default hook contract ID.
    pub hook: H256,
}

impl DefaultHookSet {
    /// Event topic.
    pub const TOPIC: &'static str = "default_hook_set";
}

/// Emitted when the required hook is updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RequiredHookSet {
    /// The new required hook contract ID.
    pub hook: H256,
}

impl RequiredHookSet {
    /// Event topic.
    pub const TOPIC: &'static str = "required_hook_set";
}

// =========================================================================
// MerkleTreeHook events
// =========================================================================

/// Emitted when a message ID is inserted into the Merkle tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InsertedIntoTree {
    /// The message ID that was inserted.
    pub message_id: MessageId,
    /// The leaf index at which it was inserted.
    pub index: u32,
}

impl InsertedIntoTree {
    /// Event topic.
    pub const TOPIC: &'static str = "inserted_into_tree";
}

// =========================================================================
// ValidatorAnnounce events
// =========================================================================

/// Emitted when a validator announces their storage location.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidatorAnnouncement {
    /// The validator's Ethereum address (20 bytes).
    pub validator: EthAddress,
    /// The storage location string (e.g., S3 bucket URL).
    pub storage_location: String,
}

impl ValidatorAnnouncement {
    /// Event topic.
    pub const TOPIC: &'static str = "validator_announcement";
}

// =========================================================================
// ProtocolFee events
// =========================================================================

/// Emitted when a protocol fee is charged during post-dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProtocolFeePaid {
    /// The sender that triggered the dispatch.
    pub sender: H256,
    /// The fee amount charged (in LUX).
    pub fee: u64,
}

impl ProtocolFeePaid {
    /// Event topic.
    pub const TOPIC: &'static str = "protocol_fee_paid";
}

// =========================================================================
// IGP events
// =========================================================================

/// Emitted when an interchain gas payment is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GasPayment {
    /// The message ID for which gas was paid.
    pub message_id: MessageId,
    /// The gas limit used in the calculation.
    pub gas_limit: u64,
    /// The computed payment amount (in LUX).
    pub payment: u64,
}

impl GasPayment {
    /// Event topic.
    pub const TOPIC: &'static str = "gas_payment";
}

// =========================================================================
// Warp Route events
// =========================================================================

/// Emitted when a warp route transfer is sent to a remote chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SentTransferRemote {
    /// The destination domain.
    pub destination: u32,
    /// The recipient address on the destination chain.
    pub recipient: H256,
    /// The amount of tokens transferred.
    pub amount: u64,
}

impl SentTransferRemote {
    /// Event topic.
    pub const TOPIC: &'static str = "sent_transfer_remote";
}

/// Emitted when a warp route transfer is received from a remote chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReceivedTransferRemote {
    /// The origin domain.
    pub origin: u32,
    /// The recipient address on this chain.
    pub recipient: H256,
    /// The amount of tokens received.
    pub amount: u64,
}

impl ReceivedTransferRemote {
    /// Event topic.
    pub const TOPIC: &'static str = "received_transfer_remote";
}
