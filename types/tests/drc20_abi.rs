// SPDX-License-Identifier: MIT OR Apache-2.0

//! Wire-compatibility proof against the exact canonical Dusk DRC20 source.

use canonical_dusk_contract_standards as canonical;
use canonical_dusk_core::abi::ContractId as CanonicalContractId;
use dusk_core::abi::ContractId;
use hyperlane_dusk_types::drc20 as local;

fn archived<T: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<256>>>(
    value: &T,
) -> Vec<u8> {
    rkyv::to_bytes::<_, 256>(value)
        .expect("fixture serialization should succeed")
        .to_vec()
}

#[test]
fn principal_variants_match_canonical_archive_layout() {
    let moonlight = [0x11; local::BLS_PUBLIC_KEY_BYTES];
    assert_eq!(
        archived(&local::Principal::Moonlight(moonlight)),
        archived(&canonical::core::Principal::Moonlight(moonlight))
    );

    let phoenix = [0x22; 32];
    assert_eq!(
        archived(&local::Principal::Phoenix(phoenix)),
        archived(&canonical::core::Principal::Phoenix(phoenix))
    );

    let contract = [0x33; 32];
    assert_eq!(
        archived(&local::Principal::Contract(ContractId::from_bytes(
            contract
        ))),
        archived(&canonical::core::Principal::Contract(
            CanonicalContractId::from_bytes(contract)
        ))
    );
}

#[test]
fn calls_match_canonical_archive_layout() {
    let owner = local::Principal::Phoenix([0x44; 32]);
    let canonical_owner = canonical::core::Principal::Phoenix([0x44; 32]);
    let recipient = local::Principal::Contract(ContractId::from_bytes([0x55; 32]));
    let canonical_recipient =
        canonical::core::Principal::Contract(CanonicalContractId::from_bytes([0x55; 32]));

    assert_eq!(
        archived(&local::BalanceOf { account: owner }),
        archived(&canonical::token::drc20::BalanceOf {
            account: canonical_owner,
        })
    );
    assert_eq!(
        archived(&local::Allowance {
            owner,
            spender: recipient,
        }),
        archived(&canonical::token::drc20::Allowance {
            owner: canonical_owner,
            spender: canonical_recipient,
        })
    );
    assert_eq!(
        archived(&local::TransferCall {
            to: recipient,
            amount: 0x0102_0304_0506_0708,
        }),
        archived(&canonical::token::drc20::TransferCall {
            to: canonical_recipient,
            amount: 0x0102_0304_0506_0708,
        })
    );
    assert_eq!(
        archived(&local::ApproveCall {
            spender: recipient,
            amount: 17,
        }),
        archived(&canonical::token::drc20::ApproveCall {
            spender: canonical_recipient,
            amount: 17,
        })
    );
    assert_eq!(
        archived(&local::TransferFromCall {
            owner,
            to: recipient,
            amount: 23,
        }),
        archived(&canonical::token::drc20::TransferFromCall {
            owner: canonical_owner,
            to: canonical_recipient,
            amount: 23,
        })
    );
}

#[test]
fn events_match_canonical_topics_and_archive_layout() {
    use hyperlane_dusk_types::events::{Drc20Approval, Drc20Transfer};

    assert_eq!(
        Drc20Transfer::TOPIC,
        canonical::token::drc20::events::TRANSFER_TOPIC
    );
    assert_eq!(
        Drc20Approval::TOPIC,
        canonical::token::drc20::events::APPROVAL_TOPIC
    );

    let from = local::Principal::Moonlight([0x66; local::BLS_PUBLIC_KEY_BYTES]);
    let to = local::Principal::Phoenix([0x77; 32]);
    assert_eq!(
        archived(&Drc20Transfer {
            from,
            to,
            amount: 29,
        }),
        archived(&canonical::token::drc20::events::Transfer {
            from: canonical::core::Principal::Moonlight([0x66; local::BLS_PUBLIC_KEY_BYTES]),
            to: canonical::core::Principal::Phoenix([0x77; 32]),
            amount: 29,
        })
    );
    assert_eq!(
        archived(&Drc20Approval {
            owner: from,
            spender: to,
            amount: 31,
        }),
        archived(&canonical::token::drc20::events::Approval {
            owner: canonical::core::Principal::Moonlight([0x66; local::BLS_PUBLIC_KEY_BYTES]),
            spender: canonical::core::Principal::Phoenix([0x77; 32]),
            amount: 31,
        })
    );
}
