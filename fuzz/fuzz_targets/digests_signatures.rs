// SPDX-License-Identifier: MIT OR Apache-2.0

//! What validators sign and what the ISM and ValidatorAnnounce accept:
//! - checkpoint and announcement digests (`types::checkpoint`) against
//!   `CheckpointLib` / `ValidatorAnnounce` computed with an independent
//!   keccak256;
//! - `signature::is_canonical` against `k256`'s view of `s` (OpenZeppelin's
//!   rule: `v` is 27 or 28 and `s` at most n/2);
//! - a signature an honest signer makes over a checkpoint is canonical and
//!   recovers to its address, and its high-s twin, which the host query
//!   would also recover, is refused.

#![no_main]

use hyperlane_dusk_fuzz::{
    reference_announcement_digest, reference_checkpoint_digest, reference_eth_signed, Reader,
};
use hyperlane_dusk_types::checkpoint::{
    announcement_digest, announcement_domain_hash, checkpoint_digest, domain_hash,
    eth_signed_message_hash,
};
use hyperlane_dusk_types::message::keccak256;
use hyperlane_dusk_types::signature::is_canonical;
use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use k256::elliptic_curve::scalar::IsHigh;
use k256::elliptic_curve::PrimeField;
use k256::Scalar;
use libfuzzer_sys::fuzz_target;

/// OpenZeppelin's acceptance rule, with `s` compared as a scalar by `k256`.
fn reference_canonical(signature: &[u8; 65]) -> bool {
    let s: [u8; 32] = signature[32..64].try_into().unwrap();
    let s_is_low = match Option::<Scalar>::from(Scalar::from_repr(s.into())) {
        // At or above the group order: not even a scalar, so above n/2.
        None => false,
        Some(scalar) => !bool::from(scalar.is_high()),
    };
    matches!(signature[64], 27 | 28) && s_is_low
}

fn eth_address(key: &VerifyingKey) -> [u8; 20] {
    let point = key.to_encoded_point(false);
    let hash = keccak256(&point.as_bytes()[1..]);
    hash[12..].try_into().unwrap()
}

fn to_65(signature: &Signature, recovery: RecoveryId) -> [u8; 65] {
    let mut out = [0u8; 65];
    out[..64].copy_from_slice(&signature.to_bytes());
    out[64] = 27 + recovery.to_byte();
    out
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader::new(data);
    let origin = reader.u32();
    let hook: [u8; 32] = reader.array();
    let root: [u8; 32] = reader.array();
    let index = reader.u32();
    let message_id: [u8; 32] = reader.array();
    let mailbox: [u8; 32] = reader.array();
    let raw_signature: [u8; 65] = reader.array();
    let secret: [u8; 32] = reader.array();
    let location = reader.rest();

    // Digests, against the independent computation.
    let digest = checkpoint_digest(origin, &hook, &root, index, &message_id);
    assert_eq!(
        digest,
        reference_checkpoint_digest(origin, &hook, &root, index, &message_id)
    );
    let mut domain_preimage = origin.to_be_bytes().to_vec();
    domain_preimage.extend_from_slice(&hook);
    domain_preimage.extend_from_slice(b"HYPERLANE");
    assert_eq!(domain_hash(origin, &hook), keccak256(&domain_preimage));
    assert_eq!(eth_signed_message_hash(&root), reference_eth_signed(&root));
    let mut announce_preimage = origin.to_be_bytes().to_vec();
    announce_preimage.extend_from_slice(&mailbox);
    announce_preimage.extend_from_slice(b"HYPERLANE_ANNOUNCEMENT");
    assert_eq!(
        announcement_domain_hash(origin, &mailbox),
        keccak256(&announce_preimage)
    );
    // Storage locations are strings; any valid UTF-8 the input holds.
    let location = match core::str::from_utf8(location) {
        Ok(text) => text,
        Err(error) => core::str::from_utf8(&location[..error.valid_up_to()]).unwrap(),
    };
    assert_eq!(
        announcement_digest(origin, &mailbox, location),
        reference_announcement_digest(origin, &mailbox, location.as_bytes())
    );

    // Canonicality of arbitrary bytes.
    assert_eq!(
        is_canonical(&raw_signature),
        reference_canonical(&raw_signature)
    );

    // An honest signature over the checkpoint.
    let Ok(key) = SigningKey::from_bytes(&secret.into()) else {
        return;
    };
    let (signature, recovery) = key
        .sign_prehash_recoverable(&digest)
        .expect("signing a 32-byte prehash succeeds");
    let bytes = to_65(&signature, recovery);
    assert!(is_canonical(&bytes), "an honest signature must be accepted");
    let signer = eth_address(key.verifying_key());
    let recovered = VerifyingKey::recover_from_prehash(&digest, &signature, recovery)
        .expect("an honest signature recovers");
    assert_eq!(eth_address(&recovered), signer);

    // Its twin, (r, n - s) with the other recovery bit, which the host query
    // recovers to the same key (security.md, LOW-2), is refused. (`k256`
    // refuses to recover a high-s signature itself, so it can't show that.)
    let (r, s) = signature.split_scalars();
    let twin = Signature::from_scalars(r, -*s).expect("n - s is a valid s");
    let twin_recovery = RecoveryId::new(!recovery.is_y_odd(), recovery.is_x_reduced());
    assert!(!is_canonical(&to_65(&twin, twin_recovery)));
    // The raw recovery ids 0/1 are refused as well.
    let mut raw_v = bytes;
    raw_v[64] -= 27;
    assert!(!is_canonical(&raw_v));
});
