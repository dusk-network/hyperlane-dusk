// SPDX-License-Identifier: MIT OR Apache-2.0

//! rkyv validation of the shared types, as the agents decode contract
//! replies and event data from an untrusted node: arbitrary bytes must be
//! refused or decode to a value that survives a serialize/validate/decode
//! round trip, and never panic or read out of bounds.

#![no_main]

use core::fmt::Debug;

use hyperlane_dusk_fuzz::Reader;
use hyperlane_dusk_types::events::{
    Dispatch, DispatchId, GasPayment, InsertedIntoTree, Process, ProcessId, ValidatorAnnouncement,
};
use hyperlane_dusk_types::merkle::IncrementalMerkle;
use hyperlane_dusk_types::message::HyperlaneMessage;
use hyperlane_dusk_types::{Checkpoint, DomainGasConfig, EthAddress, GasPaymentRecord};
use libfuzzer_sys::fuzz_target;
use rkyv::ser::serializers::AllocSerializer;
use rkyv::validation::validators::DefaultValidator;
use rkyv::{check_archived_root, AlignedVec, Archive, Deserialize, Infallible, Serialize};

fn round_trip<T>(bytes: &[u8])
where
    T: Archive + Serialize<AllocSerializer<256>> + PartialEq + Debug,
    T::Archived: Deserialize<T, Infallible> + for<'a> rkyv::CheckBytes<DefaultValidator<'a>>,
{
    // Contract replies arrive as a byte body; the agents copy them into an
    // aligned buffer before validating.
    let mut aligned = AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    let Ok(archived) = check_archived_root::<T>(&aligned) else {
        return;
    };
    let value: T = archived.deserialize(&mut Infallible).unwrap();
    let again = rkyv::to_bytes::<_, 256>(&value).unwrap();
    let archived_again = check_archived_root::<T>(&again).expect("a value it wrote validates");
    let value_again: T = archived_again.deserialize(&mut Infallible).unwrap();
    assert_eq!(value, value_again);
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader::new(data);
    let choice = reader.u8();
    let bytes = reader.rest();
    match choice % 17 {
        0 => round_trip::<Dispatch>(bytes),
        1 => round_trip::<DispatchId>(bytes),
        2 => round_trip::<Process>(bytes),
        3 => round_trip::<ProcessId>(bytes),
        4 => round_trip::<GasPayment>(bytes),
        5 => round_trip::<InsertedIntoTree>(bytes),
        6 => round_trip::<ValidatorAnnouncement>(bytes),
        7 => round_trip::<GasPaymentRecord>(bytes),
        8 => round_trip::<IncrementalMerkle>(bytes),
        9 => round_trip::<HyperlaneMessage>(bytes),
        10 => round_trip::<Checkpoint>(bytes),
        11 => round_trip::<DomainGasConfig>(bytes),
        12 => round_trip::<Vec<[u8; 32]>>(bytes),
        13 => round_trip::<(Vec<EthAddress>, u8)>(bytes),
        14 => round_trip::<Option<[u8; 32]>>(bytes),
        15 => round_trip::<Vec<GasPaymentRecord>>(bytes),
        _ => round_trip::<Vec<u8>>(bytes),
    }
});
