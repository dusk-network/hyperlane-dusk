// SPDX-License-Identifier: MIT OR Apache-2.0

//! Known answers from implementations other than this crate's, for what the
//! unit tests only checked against the crate itself. Mutation testing showed
//! these paths could change without a test failing:
//! - the software keccak256 (the agents' only keccak for message IDs and
//!   event data) on inputs of one rate (136 bytes) or more, which every
//!   message with a body over 58 bytes is: Foundry's `cast keccak`;
//! - `keccak256_vec`, which the contracts hash with;
//! - decoding a message of exactly 77 bytes (an empty body), and a short
//!   buffer that starts with the right version;
//! - the announcement and checkpoint digests: Solidity's
//!   `ValidatorAnnounce` and `CheckpointLib`, from the monorepo's
//!   `rust/main/chains/hyperlane-dusk/tests/fixtures/solidity_vectors.jsonl`.

use hyperlane_dusk_types::checkpoint::{
    announcement_digest, announcement_domain_hash, checkpoint_digest, domain_hash,
};
use hyperlane_dusk_types::message::{self, keccak256, keccak256_vec, MIN_MESSAGE_LENGTH};
use hyperlane_dusk_types::storage::{BlockHeights, ChunkedLog};
use hyperlane_dusk_types::VERSION;

fn hex32(text: &str) -> [u8; 32] {
    let text = text.strip_prefix("0x").unwrap_or(text);
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

/// keccak256 of the bytes `0, 1, 2, ...` (mod 256) of each length, from
/// `cast keccak`: around one and two rates, and several blocks.
const KECCAK: [(usize, &str); 9] = [
    (
        0,
        "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470",
    ),
    (
        1,
        "bc36789e7a1e281436464229828f817d6612f7b477d66591ff96a9e064bcc98a",
    ),
    (
        135,
        "cbdfd9dee5faad3818d6b06f95a219fd290b0e1706f6a82e5a595b9ce9faca62",
    ),
    (
        136,
        "7ce759f1ab7f9ce437719970c26b0a66ff11fe3e38e17df89cf5d29c7d7f807e",
    ),
    (
        137,
        "ac73d4fae68b8453f764007c1a20ce95994187861f0c3227a3a8e99a73a3b1db",
    ),
    (
        271,
        "7c974895b2a88303ff2dc6b58f438ceb0b298cac91099ac0539cc0f477506191",
    ),
    (
        272,
        "fdf2ec49e749960d3c8521a0219af8d03e30e2b3bf19bd16150ee0eaf133d66e",
    ),
    (
        273,
        "4f707289a9c3ccd0c4a51f2f17339f5dd171d371c04ff7783b735b5b22682eaf",
    ),
    (
        1000,
        "aca79e4146e30eb1c733f6d6060d72471c36ea4e01ebf45d7f4916249c2bbd82",
    ),
];

#[test]
fn keccak256_matches_cast_across_rate_boundaries() {
    for (len, expected) in KECCAK {
        let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
        assert_eq!(keccak256(&data), hex32(expected), "length {len}");
        assert_eq!(keccak256_vec(data.clone()), hex32(expected), "length {len}");
        assert_eq!(message::id(&data), hex32(expected), "length {len}");
    }
}

#[test]
fn a_message_of_exactly_77_bytes_decodes_and_shorter_ones_do_not() {
    let encoded = message::encode(VERSION, 9, 1, [0x11; 32], 2, [0x22; 32], &[]);
    assert_eq!(encoded.len(), MIN_MESSAGE_LENGTH);
    let decoded = message::decode(&encoded).unwrap();
    assert!(decoded.body.is_empty());
    assert_eq!(decoded.encode(), encoded);
    for len in [0, 1, 5, 9, 40, 41, 45, 76] {
        let mut short = encoded[..len].to_vec();
        if let Some(first) = short.first_mut() {
            *first = VERSION;
        }
        assert!(message::decode(&short).is_none(), "{len} bytes");
    }
}

/// `ValidatorAnnounce.getAnnouncementDigest` for two announcements.
#[test]
fn announcement_digests_match_solidity() {
    let cases = [
        (
            1_146_442_572u32,
            "0x545d8412ce73fd467a0ed4beae10742305ebf1d30d6be524d09634ae91b39b6d",
            "",
            "0x4aed3ce6174ceedfa1f1cceab4b1e6983f87e73332cdf3f12dc18993588bb1ed",
        ),
        (
            2_243_745_709,
            "0x000000000000000000000000cfaa25a96872212400aae649c04141ce52ec6c77",
            "s3://hyperlane-testnet/us-east-1/validator-0",
            "0x88ba9a10b95c49593fe09aff4eff3357273c4a680932fbbd985347c14d0d2ef5",
        ),
    ];
    for (domain, mailbox, location, digest) in cases {
        let mailbox = hex32(mailbox);
        assert_eq!(
            announcement_digest(domain, &mailbox, location),
            hex32(digest),
            "{location:?}"
        );
        // The domain hash is the one the digest is built on.
        let mut preimage = domain.to_be_bytes().to_vec();
        preimage.extend_from_slice(&mailbox);
        preimage.extend_from_slice(b"HYPERLANE_ANNOUNCEMENT");
        assert_eq!(
            announcement_domain_hash(domain, &mailbox),
            keccak256(&preimage)
        );
    }
}

/// `CheckpointLib.domainHash` and `CheckpointLib.digest`.
#[test]
fn checkpoint_digest_matches_solidity() {
    let origin = 1_146_442_572;
    let hook = hex32("0x9a255765124a806eec11a0b0fd625966bf455698447b0f313207201f0f2ac507");
    let root = hex32("0xde2c483c85d74a0c52ff808ac119f60452b9b47c6dc7b508cc6bae25c21efbfa");
    let index = 3_802_620_492;
    let message_id = hex32("0x9951e435fc8f524a2dc024cb72d4c626c859b2b9583e6122a0d18e417d9cb95c");
    assert_eq!(
        domain_hash(origin, &hook),
        hex32("0x079b56b0ca9815fe920b0e4e67c183b93aee9e3d70a340a6cbad078811e752a0")
    );
    assert_eq!(
        checkpoint_digest(origin, &hook, &root, index, &message_id),
        hex32("0x5426cdd2a7bf2408fd336c93f9f6a09e277f50bdc339acc7caf33d2bb650c738")
    );
}

#[test]
fn logs_are_empty_exactly_until_their_first_entry() {
    let mut log = ChunkedLog::<[u8; 32]>::new();
    assert!(log.is_empty());
    log.push([1; 32]);
    assert!(!log.is_empty());
    let mut heights = BlockHeights::new();
    assert!(heights.is_empty());
    heights.push(7);
    assert!(!heights.is_empty());
    assert_eq!(heights.len(), 1);
}
