// SPDX-License-Identifier: MIT OR Apache-2.0

//! The types crate's software keccak256, which the agents use for every
//! message ID, digest and Merkle node off chain, against the `sha3` crate.
//! Inputs cross the 136-byte rate boundary many times.

#![no_main]

use hyperlane_dusk_fuzz::reference_keccak;
use hyperlane_dusk_types::merkle::hash_pair;
use hyperlane_dusk_types::message::{keccak256, keccak256_vec};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let expected = reference_keccak(data);
    assert_eq!(keccak256(data), expected);
    assert_eq!(keccak256_vec(data.to_vec()), expected);
    // Every length around a multiple of the rate, from the same bytes.
    for cut in [135usize, 136, 137, 271, 272, 273] {
        if data.len() >= cut {
            assert_eq!(keccak256(&data[..cut]), reference_keccak(&data[..cut]));
        }
    }
    if data.len() >= 64 {
        let mut left = [0u8; 32];
        let mut right = [0u8; 32];
        left.copy_from_slice(&data[..32]);
        right.copy_from_slice(&data[32..64]);
        assert_eq!(hash_pair(left, right), reference_keccak(&data[..64]));
    }
});
