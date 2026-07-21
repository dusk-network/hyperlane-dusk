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

use crate::{DomainGasConfig, EthAddress, MessageId, H256};

#[cfg(feature = "abi")]
macro_rules! impl_contract_events {
    ($($event:ty),+ $(,)?) => {
        $(
            impl dusk_forge::ContractEvent for $event {
                const TOPICS: &'static [&'static str] = &[<$event>::TOPIC];
            }
        )+
    };
}

// =========================================================================
// Contract identifiers for generic operational events
// =========================================================================

/// Mailbox contract type identifier.
pub const CONTRACT_MAILBOX: u8 = 1;
/// `MerkleTreeHook` contract type identifier.
pub const CONTRACT_MERKLE_TREE_HOOK: u8 = 2;
/// `ValidatorAnnounce` contract type identifier.
pub const CONTRACT_VALIDATOR_ANNOUNCE: u8 = 3;
/// `ProtocolFee` contract type identifier.
pub const CONTRACT_PROTOCOL_FEE: u8 = 4;
/// `InterchainGasPaymaster` contract type identifier.
pub const CONTRACT_IGP: u8 = 5;
/// `MessageIdMultisigISM` contract type identifier.
pub const CONTRACT_ISM_MULTISIG: u8 = 6;
/// `WarpDrc20` contract type identifier.
pub const CONTRACT_WARP_DRC20: u8 = 7;
/// `WarpDrc20Collateral` contract type identifier.
pub const CONTRACT_WARP_DRC20_COLLATERAL: u8 = 8;
/// `WarpNative` contract type identifier.
pub const CONTRACT_WARP_NATIVE: u8 = 9;
/// `AggregationHook` contract type identifier.
pub const CONTRACT_AGGREGATION_HOOK: u8 = 10;

// =========================================================================
// Operational/admin events
// =========================================================================

/// Emitted when a contract is initialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Initialized {
    /// Contract type identifier, using the `CONTRACT_*` constants above.
    pub contract_type: u8,
    /// Initial owner address, or zero if the contract has no owner.
    pub owner: H256,
    /// Initial mailbox contract ID, or zero if not applicable.
    pub mailbox: H256,
    /// Local domain, or zero if not applicable.
    pub local_domain: u32,
}

impl Initialized {
    /// Event topic.
    pub const TOPIC: &'static str = "initialized";
}

/// Emitted when contract ownership changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OwnershipTransferred {
    /// Previous owner.
    pub previous_owner: H256,
    /// New owner.
    pub new_owner: H256,
}

impl OwnershipTransferred {
    /// Event topic.
    pub const TOPIC: &'static str = "ownership_transferred";
}

/// Emitted when ownership is renounced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OwnershipRenounced {
    /// Previous owner.
    pub previous_owner: H256,
}

impl OwnershipRenounced {
    /// Event topic.
    pub const TOPIC: &'static str = "ownership_renounced";
}

/// Emitted when an account key is registered for an H256 recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountRegistered {
    /// Keccak256 hash of the registered public key.
    pub account_hash: H256,
}

impl AccountRegistered {
    /// Event topic.
    pub const TOPIC: &'static str = "account_registered";
}

/// Emitted when a remote router is enrolled for a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RemoteRouterEnrolled {
    /// Remote domain.
    pub domain: u32,
    /// Remote router address.
    pub router: H256,
}

impl RemoteRouterEnrolled {
    /// Event topic.
    pub const TOPIC: &'static str = "remote_router_enrolled";
}

/// Emitted when a warp route hook override changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HookSet {
    /// New hook contract ID.
    pub hook: H256,
}

impl HookSet {
    /// Event topic.
    pub const TOPIC: &'static str = "hook_set";
}

/// Emitted when a warp route ISM override changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IsmSet {
    /// New ISM contract ID.
    pub ism: H256,
}

impl IsmSet {
    /// Event topic.
    pub const TOPIC: &'static str = "ism_set";
}

/// Emitted when a fee beneficiary changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BeneficiarySet {
    /// New beneficiary contract ID.
    pub beneficiary: H256,
}

impl BeneficiarySet {
    /// Event topic.
    pub const TOPIC: &'static str = "beneficiary_set";
}

/// Emitted when the fixed protocol fee changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProtocolFeeSet {
    /// New protocol fee.
    pub fee: u64,
}

impl ProtocolFeeSet {
    /// Event topic.
    pub const TOPIC: &'static str = "protocol_fee_set";
}

/// Emitted when an IGP domain gas configuration changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DomainGasConfigSet {
    /// Remote domain.
    pub domain: u32,
    /// New gas configuration.
    pub config: DomainGasConfig,
}

impl DomainGasConfigSet {
    /// Event topic.
    pub const TOPIC: &'static str = "domain_gas_config_set";
}

/// Emitted when the multisig validator set or threshold changes.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidatorsAndThresholdSet {
    /// New validator set.
    pub validators: Vec<EthAddress>,
    /// New threshold.
    pub threshold: u8,
}

impl ValidatorsAndThresholdSet {
    /// Event topic.
    pub const TOPIC: &'static str = "validators_and_threshold_set";
}

/// Emitted when a pending warp transfer is claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PendingTransferClaimed {
    /// Recipient hash that held the pending transfer.
    pub recipient: H256,
    /// Claimed amount.
    pub amount: u64,
}

impl PendingTransferClaimed {
    /// Event topic.
    pub const TOPIC: &'static str = "pending_transfer_claimed";
}

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

/// Emitted when native DUSK is deposited into a Mailbox dispatch-fee credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DispatchFeeFunded {
    /// Message sender identity whose credit was funded.
    pub payer: H256,
    /// Amount deposited in LUX.
    pub amount: u64,
}

impl DispatchFeeFunded {
    /// Event topic.
    pub const TOPIC: &'static str = "dispatch_fee_funded";
}

/// Emitted when native DUSK is withdrawn from a Mailbox dispatch-fee credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DispatchFeeWithdrawn {
    /// Message sender identity whose credit was withdrawn.
    pub payer: H256,
    /// Keccak256 hash of the recipient Moonlight public key.
    pub recipient: H256,
    /// Amount withdrawn in LUX.
    pub amount: u64,
}

impl DispatchFeeWithdrawn {
    /// Event topic.
    pub const TOPIC: &'static str = "dispatch_fee_withdrawn";
}

/// Emitted when a dispatch consumes native DUSK fee credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DispatchFeePaid {
    /// Message sender identity whose credit was consumed.
    pub payer: H256,
    /// Dispatched message ID.
    pub message_id: MessageId,
    /// Total amount paid to hooks in LUX.
    pub amount: u64,
}

impl DispatchFeePaid {
    /// Event topic.
    pub const TOPIC: &'static str = "dispatch_fee_paid";
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

/// Emitted when `WarpDrc20` balances change.
#[cfg(feature = "drc20")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Drc20Transfer {
    /// Source principal. The zero contract principal means mint.
    pub from: crate::drc20::Principal,
    /// Destination principal. The zero contract principal means burn.
    pub to: crate::drc20::Principal,
    /// Transfer amount.
    pub amount: u64,
}

#[cfg(feature = "drc20")]
impl Drc20Transfer {
    /// Event topic.
    pub const TOPIC: &'static str = "drc20/transfer";
}

/// Emitted when a `WarpDrc20` allowance changes.
#[cfg(feature = "drc20")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Drc20Approval {
    /// Token owner principal.
    pub owner: crate::drc20::Principal,
    /// Approved spender principal.
    pub spender: crate::drc20::Principal,
    /// New allowance amount.
    pub amount: u64,
}

#[cfg(feature = "drc20")]
impl Drc20Approval {
    /// Event topic.
    pub const TOPIC: &'static str = "drc20/approval";
}

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

#[cfg(feature = "abi")]
impl_contract_events!(
    Initialized,
    OwnershipTransferred,
    OwnershipRenounced,
    AccountRegistered,
    RemoteRouterEnrolled,
    HookSet,
    IsmSet,
    BeneficiarySet,
    ProtocolFeeSet,
    DomainGasConfigSet,
    ValidatorsAndThresholdSet,
    PendingTransferClaimed,
    Dispatch,
    DispatchId,
    DispatchFeeFunded,
    DispatchFeeWithdrawn,
    DispatchFeePaid,
    Process,
    ProcessId,
    DefaultIsmSet,
    DefaultHookSet,
    RequiredHookSet,
    InsertedIntoTree,
    ValidatorAnnouncement,
    ProtocolFeePaid,
    GasPayment,
    Drc20Approval,
    Drc20Transfer,
    SentTransferRemote,
    ReceivedTransferRemote,
);
