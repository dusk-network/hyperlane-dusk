// SPDX-License-Identifier: MIT OR Apache-2.0

//! Dusk caller resolution for contracts that accept either accounts or contracts.

#[cfg(feature = "abi")]
use dusk_bytes::Serializable;
#[cfg(feature = "abi")]
use dusk_core::abi::{self, ContractId};
#[cfg(feature = "abi")]
use dusk_core::transfer::TRANSFER_CONTRACT;

#[cfg(feature = "abi")]
use crate::message;
#[cfg(feature = "abi")]
use crate::H256;

/// Resolve the effective caller to Hyperlane's 32-byte address representation.
///
/// A root Moonlight call is represented by `keccak256(public_key)`. A nested
/// contract call is represented by the immediate caller's `ContractId` bytes.
/// Queries have no meaningful caller and are rejected.
///
/// # Panics
///
/// Panics when invoked outside a transaction, from an unexpected root caller,
/// or from a shielded transaction without a public sender.
#[cfg(feature = "abi")]
#[must_use]
pub fn effective_caller() -> H256 {
    let depth = abi::callstack().len();
    assert!(depth > 0, "Hyperlane: caller unavailable in query context");

    let caller = abi::caller().expect("Hyperlane: caller unavailable");
    if depth == 1 {
        assert!(
            caller == TRANSFER_CONTRACT,
            "Hyperlane: invalid root caller"
        );
        let public_sender =
            abi::public_sender().expect("Hyperlane: shielded transactions not supported");
        message::keccak256(&public_sender.to_bytes())
    } else {
        caller.to_bytes()
    }
}

/// Return whether the current frame is an authentic contract-to-contract
/// transfer callback whose declared source is `expected_source`.
///
/// Root Moonlight calls also see the transfer contract as immediate caller,
/// but have call-stack depth one. Requiring a deeper stack prevents callers
/// from spoofing a [`dusk_core::transfer::ReceiveFromContract`] argument.
#[cfg(feature = "abi")]
#[must_use]
pub fn authentic_transfer_callback(
    declared_source: ContractId,
    expected_source: ContractId,
) -> bool {
    abi::caller() == Some(TRANSFER_CONTRACT)
        && abi::callstack().len() >= 2
        && declared_source == expected_source
}
