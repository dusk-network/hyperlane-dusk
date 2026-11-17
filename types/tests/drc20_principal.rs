// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Principal::is_zero`, which WarpDrc20's `approve` relies on to refuse a
//! zero spender, and the principals' order. Mutation testing showed neither
//! was checked. Built only with the `drc20` feature.

#![cfg(feature = "drc20")]

use core::cmp::Ordering;

use dusk_core::abi::ContractId;
use hyperlane_dusk_types::drc20::{Principal, BLS_PUBLIC_KEY_BYTES};

fn principals(byte: Option<(usize, u8)>) -> [Principal; 3] {
    let mut moonlight = [0u8; BLS_PUBLIC_KEY_BYTES];
    let mut phoenix = [0u8; 32];
    let mut contract = [0u8; 32];
    if let Some((at, value)) = byte {
        moonlight[at % BLS_PUBLIC_KEY_BYTES] = value;
        phoenix[at % 32] = value;
        contract[at % 32] = value;
    }
    [
        Principal::Moonlight(moonlight),
        Principal::Phoenix(phoenix),
        Principal::Contract(ContractId::from_bytes(contract)),
    ]
}

#[test]
fn only_an_all_zero_principal_of_each_kind_is_zero() {
    for principal in principals(None) {
        assert!(principal.is_zero(), "{principal:?}");
    }
    for at in [0, 1, 15, 31, 100, 192] {
        for value in [1, 0x80, 0xff] {
            for principal in principals(Some((at, value))) {
                assert!(!principal.is_zero(), "{principal:?}");
            }
        }
    }
}

#[test]
fn principals_order_by_kind_then_by_bytes() {
    let [moonlight, phoenix, contract] = principals(Some((3, 7)));
    assert!(moonlight < phoenix && phoenix < contract);
    for (low, high) in principals(Some((5, 1)))
        .into_iter()
        .zip(principals(Some((5, 2))))
    {
        assert_eq!(low.cmp(&high), Ordering::Less, "{low:?}");
        assert_eq!(high.cmp(&low), Ordering::Greater);
        assert_eq!(low.partial_cmp(&high), Some(Ordering::Less));
        assert_eq!(low.cmp(&low), Ordering::Equal);
    }
    // The first byte decides before a later one does.
    let [early, ..] = principals(Some((0, 1)));
    let [late, ..] = principals(Some((50, 0xff)));
    assert_eq!(early.cmp(&late), Ordering::Greater);
}
