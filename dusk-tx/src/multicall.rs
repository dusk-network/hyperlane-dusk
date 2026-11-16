//! What `deploy-wasm --check-multicall` asks a contract to tell it is
//! Multicall for Dusk (`HDauven/dusk-multicall`): `version`, and an
//! `aggregate` whose answer is known.
//!
//! The types replicate the contract's ABI (rkyv 0.7 archives, `size_32`), as
//! the agents' do. The tests check them against the encodings of the
//! contract's own crate and the replies of the contract on testnet, which
//! the agents' fixtures hold.

use rkyv::{Archive, Deserialize, Serialize};

use crate::rues::RuesClient;

/// The ABI version `version` returns.
pub const VERSION: u32 = 1;

/// The transfer contract, whose `chain_id` the check reads through the
/// multicall.
const TRANSFER_CONTRACT: [u8; 32] = {
    let mut id = [0u8; 32];
    id[0] = 1;
    id
};

/// One read of an `aggregate`.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub struct Call {
    /// The contract to call (a `ContractId`, archived as its 32 bytes).
    pub contract: [u8; 32],
    /// The function to call.
    pub fn_name: String,
    /// The argument, in the callee's own encoding.
    pub fn_args: Vec<u8>,
    /// The most gas the call may spend, or `0` for a fair share of what the
    /// query has left.
    pub gas_limit: u64,
}

/// What `aggregate` returns.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub struct Aggregate {
    /// The height of the block whose state the query ran on.
    pub height: u64,
    /// One result per call, in call order: the bytes the callee returned,
    /// or why the call failed.
    pub results: Vec<Result<Vec<u8>, CallError>>,
}

/// Why a call of an `aggregate` failed.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub enum CallError {
    /// The callee panicked with this message.
    Panic(String),
    /// The callee ran out of the gas the call had.
    OutOfGas,
    /// No contract has this ID.
    DoesNotExist,
    /// Any other failure, such as a missing function or a trap.
    Unknown,
}

/// The `aggregate` the check sends: one read of the transfer contract's
/// `chain_id`, whose reply, its one byte, the check knows.
pub fn chain_id_calls() -> Vec<Call> {
    vec![Call {
        contract: TRANSFER_CONTRACT,
        fn_name: String::from("chain_id"),
        fn_args: Vec::new(),
        gas_limit: 0,
    }]
}

/// Whether an `aggregate` reply to [`chain_id_calls`] is the multicall's on
/// the chain with this ID: exactly one result, the chain ID.
pub fn answers_chain_id(aggregate: &Aggregate, chain_id: u8) -> Result<(), String> {
    match aggregate.results.as_slice() {
        [Ok(reply)] if reply.as_slice() == [chain_id] => Ok(()),
        results => Err(format!(
            "its aggregate of the transfer contract's chain_id answered {results:?}, \
             not [Ok([{chain_id}])]"
        )),
    }
}

/// Check that the contract at `contract` answers as Multicall for Dusk
/// v1 on the chain with ID `chain_id`: `version` with 1, and an `aggregate`
/// of the transfer contract's `chain_id` with that chain ID as its only
/// result.
pub async fn check(client: &RuesClient, contract: &[u8; 32], chain_id: u8) -> Result<(), String> {
    let version: u32 = client
        .contract_query(contract, "version", &())
        .await
        .map_err(|e| format!("its version query failed: {e}"))?;
    if version != VERSION {
        return Err(format!("it answers version {version}, not {VERSION}"));
    }
    let aggregate: Aggregate = client
        .contract_query(contract, "aggregate", &chain_id_calls())
        .await
        .map_err(|e| format!("its aggregate query failed: {e}"))?;
    answers_chain_id(&aggregate, chain_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rues::{rkyv_deserialize, rkyv_serialize};

    /// The encoding of [`chain_id_calls`] by the contract's own crate, and
    /// the reply of the contract deployed on testnet (chain ID 2) to it at
    /// height 4,867,363, from the agents' fixtures
    /// (`hyperlane-dusk/tests/fixtures/multicall_vectors.json` and
    /// `multicall_testnet.json` in the monorepo fork).
    const CHAIN_ID_REQUEST: &str = "636861696e5f69640100000000000000000000000000000000000000000000\
         00000000000000000008000000d8ffffffd8ffffff000000000000000000000000c8ffffff01000000";
    const CHAIN_ID_REPLY: &str =
        "0200000000000000f8ffffff01000000000000000000000023454a0000000000e4ffffff01000000";
    /// `version`'s reply, and an empty `aggregate`'s at the same height.
    const VERSION_REPLY: &str = "01000000";
    const EMPTY_REPLY: &str = "23454a0000000000f8ffffff00000000";

    #[test]
    fn the_types_read_and_write_what_the_contract_does() {
        assert_eq!(
            hex::encode(rkyv_serialize(&chain_id_calls())),
            CHAIN_ID_REQUEST
        );
        let reply: Aggregate = rkyv_deserialize(&hex::decode(CHAIN_ID_REPLY).unwrap()).unwrap();
        assert_eq!(
            reply,
            Aggregate {
                height: 4_867_363,
                results: vec![Ok(vec![2])],
            }
        );
        assert_eq!(answers_chain_id(&reply, 2), Ok(()));
        assert!(answers_chain_id(&reply, 3).is_err());
        let version: u32 = rkyv_deserialize(&hex::decode(VERSION_REPLY).unwrap()).unwrap();
        assert_eq!(version, VERSION);
        let empty: Aggregate = rkyv_deserialize(&hex::decode(EMPTY_REPLY).unwrap()).unwrap();
        assert!(empty.results.is_empty());
        assert!(answers_chain_id(&empty, 2).is_err());
        let failed = Aggregate {
            height: 7,
            results: vec![Err(CallError::DoesNotExist)],
        };
        assert!(answers_chain_id(&failed, 2).is_err());
    }
}
