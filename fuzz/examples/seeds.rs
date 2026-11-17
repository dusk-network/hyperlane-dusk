// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writes seed inputs for the fuzz targets into `corpus/<target>/`:
//! valid messages, token messages and archived values, so the targets start
//! past the formats' outer checks. Run from `fuzz/`:
//! `cargo run --example seeds`.

use std::fs;
use std::path::Path;

use hyperlane_dusk_types::events::{
    Dispatch, DispatchId, GasPayment, InsertedIntoTree, Process, ProcessId, ValidatorAnnouncement,
};
use hyperlane_dusk_types::merkle::IncrementalMerkle;
use hyperlane_dusk_types::message::HyperlaneMessage;
use hyperlane_dusk_types::{
    token_message, Checkpoint, DomainGasConfig, EthAddress, GasPaymentRecord, VERSION,
};
use rkyv::ser::serializers::AllocSerializer;
use rkyv::Serialize;

fn write(target: &str, name: &str, bytes: &[u8]) {
    let dir = Path::new("corpus").join(target);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("seed-{name}")), bytes).unwrap();
}

fn archived<T: Serialize<AllocSerializer<256>>>(choice: u8, value: &T) -> Vec<u8> {
    let mut out = vec![choice];
    out.extend_from_slice(&rkyv::to_bytes::<_, 256>(value).unwrap());
    out
}

fn message(body: Vec<u8>) -> HyperlaneMessage {
    HyperlaneMessage {
        version: VERSION,
        nonce: 7,
        origin: 11_155_111,
        sender: [0x11; 32],
        destination: 1_146_442_572,
        recipient: [0x22; 32],
        body,
    }
}

fn main() {
    for (name, body) in [
        ("empty", vec![]),
        ("transfer", token_message::encode([0x33; 32], 1_000_000_000)),
        ("long", vec![0xAB; 300]),
    ] {
        write("message_codec", name, &message(body).encode());
    }
    write(
        "token_message",
        "plain",
        &token_message::encode([0x44; 32], u64::MAX),
    );
    write(
        "token_message",
        "metadata",
        &token_message::encode_with_metadata([0x44; 32], 5, b"metadata"),
    );
    write("keccak", "rate", &[0x5A; 136]);
    write("keccak", "two-rates", &[0xA5; 273]);

    let record = GasPaymentRecord {
        message_id: [0x66; 32],
        destination: 11_155_111,
        gas_limit: 2_170_000,
        payment: 6_000_000_000,
        block_height: 4_860_000,
    };
    let mut tree = IncrementalMerkle::new();
    for leaf in 0..5u8 {
        tree.insert([leaf; 32]);
    }
    let seeds: Vec<Vec<u8>> = vec![
        archived(
            0,
            &Dispatch {
                sender: [1; 32],
                destination: 2,
                recipient: [3; 32],
                message: message(vec![9; 40]).encode(),
            },
        ),
        archived(
            1,
            &DispatchId {
                message_id: [4; 32],
            },
        ),
        archived(
            2,
            &Process {
                origin: 5,
                sender: [6; 32],
                recipient: [7; 32],
            },
        ),
        archived(
            3,
            &ProcessId {
                message_id: [8; 32],
            },
        ),
        archived(
            4,
            &GasPayment {
                message_id: [9; 32],
                gas_limit: 10,
                payment: 11,
            },
        ),
        archived(
            5,
            &InsertedIntoTree {
                message_id: [12; 32],
                index: 13,
            },
        ),
        archived(
            6,
            &ValidatorAnnouncement {
                validator: EthAddress([14; 20]),
                storage_location: "s3://bucket/region/validator-0".into(),
            },
        ),
        archived(7, &record),
        archived(8, &tree),
        archived(9, &message(vec![1, 2, 3])),
        archived(
            10,
            &Checkpoint {
                origin: 1,
                merkle_tree_hook: [2; 32],
                root: [3; 32],
                index: 4,
                message_id: [5; 32],
            },
        ),
        archived(
            11,
            &DomainGasConfig {
                gas_overhead: 1,
                token_exchange_rate: 2,
                gas_price: 3,
            },
        ),
        archived(12, &vec![[0x77u8; 32]; 3]),
        archived(13, &(vec![EthAddress([1; 20]), EthAddress([2; 20])], 2u8)),
        archived(14, &Some([0x88u8; 32])),
        archived(15, &vec![record; 2]),
        archived(16, &vec![1u8, 2, 3, 4, 5]),
    ];
    for (index, seed) in seeds.iter().enumerate() {
        write("rkyv_types", &format!("{index:02}"), seed);
    }

    // chunked_log: type selector, then "push many" ops crossing chunks.
    for selector in 0..7u8 {
        let mut input = vec![selector];
        for _ in 0..4 {
            input.extend_from_slice(&[1, 0xff, 0x0f, 2, 0, 0, 0, 0, 3, 0]);
        }
        write("chunked_log", &format!("{selector}"), &input);
    }
    // block_heights: a start height, then batches of new blocks.
    let mut heights = 1_000u64.to_le_bytes().to_vec();
    heights.extend_from_slice(&[3, 0x00, 0x20, 0, 4, 0, 1, 7, 3, 0x10, 0x10, 2, 5, 1]);
    write("block_heights", "batches", &heights);
    // merkle: each side of 2^10 and 2^13.
    for (name, input) in [
        ("1023", vec![0u8, 10, 0]),
        ("1024", vec![0, 10, 1]),
        ("8193", vec![0, 13, 2]),
    ] {
        write("merkle", name, &input);
    }
    // digests_signatures: fields, then a valid secret key (1).
    let mut digests = vec![0x42; 4 + 32 * 4 + 4 + 65];
    let mut secret = [0u8; 32];
    secret[31] = 1;
    digests.extend_from_slice(&secret);
    digests.extend_from_slice(b"s3://bucket/validator");
    write("digests_signatures", "key-one", &digests);
    // ism_metadata: threshold 2, three signatures in the relayer's layout.
    let mut ism = vec![2u8, 3];
    ism.extend_from_slice(&[0x10; 32 + 32 + 4]);
    ism.extend_from_slice(&[0x20; 3 * 65]);
    write("ism_metadata", "two-of-three", &ism);
}
