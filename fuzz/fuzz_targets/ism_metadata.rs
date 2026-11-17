// SPDX-License-Identifier: MIT OR Apache-2.0

//! MessageIdMultisig metadata: the ISM's parsing rules against Solidity's.
//!
//! The ISM parses its metadata inside the wasm-only contract module
//! (`contracts/ism-multisig/src/lib.rs`, `verify`), which can't be built
//! natively, so `dusk_parse` below restates its checks and offsets line for
//! line. `solidity_parse` restates `MessageIdMultisigIsmMetadata.sol` and
//! `AbstractMultisigIsm.verify`, where a calldata slice out of range
//! reverts. The target checks that:
//! - metadata in the relayer's layout (hook, root, big-endian index, then
//!   65-byte signatures) parses to exactly its fields on both sides;
//! - whatever Dusk accepts, Solidity accepts with the same fields and the
//!   same signatures (Dusk is never more permissive);
//! - the only metadata Solidity accepts and Dusk refuses has trailing bytes
//!   that are not a whole signature (security.md, "Multisig metadata").
//!
//! The VM tests and a stateful fuzzer of the deployed contract are what
//! check the contract itself.

#![no_main]

use hyperlane_dusk_fuzz::Reader;
use libfuzzer_sys::fuzz_target;

const SIGNATURES_OFFSET: usize = 68;
const SIGNATURE_LENGTH: usize = 65;

#[derive(Debug, PartialEq, Eq)]
struct Parsed<'a> {
    hook: &'a [u8],
    root: &'a [u8],
    index: u32,
    /// The signatures `verify` checks: the first `threshold`.
    signatures: Vec<&'a [u8]>,
}

/// The ISM's checks, in its order, up to the signature loop.
fn dusk_parse(metadata: &[u8], threshold: u8) -> Result<Parsed<'_>, &'static str> {
    if threshold == 0 {
        return Err("MultisigISM: not initialized");
    }
    if metadata.len() < SIGNATURES_OFFSET {
        return Err("MultisigISM: metadata too short");
    }
    if (metadata.len() - SIGNATURES_OFFSET) % SIGNATURE_LENGTH != 0 {
        return Err("MultisigISM: metadata signature length mismatch");
    }
    let count = (metadata.len() - SIGNATURES_OFFSET) / SIGNATURE_LENGTH;
    if count < usize::from(threshold) {
        return Err("MultisigISM: not enough signatures");
    }
    let signatures = (0..usize::from(threshold))
        .map(|i| {
            let start = SIGNATURES_OFFSET + i * SIGNATURE_LENGTH;
            &metadata[start..start + SIGNATURE_LENGTH]
        })
        .collect();
    Ok(Parsed {
        hook: &metadata[0..32],
        root: &metadata[32..64],
        index: u32::from_be_bytes(metadata[64..68].try_into().unwrap()),
        signatures,
    })
}

/// Solidity: the field getters slice calldata (reverting when out of range),
/// and `verify` reads `signatureAt(i)` for `i < threshold`.
fn solidity_parse(metadata: &[u8], threshold: u8) -> Option<Parsed<'_>> {
    if threshold == 0 {
        return None; // "No MultisigISM threshold present for message"
    }
    let hook = metadata.get(0..32)?;
    let root = metadata.get(32..64)?;
    let index = u32::from_be_bytes(metadata.get(64..68)?.try_into().unwrap());
    let mut signatures = Vec::new();
    for i in 0..usize::from(threshold) {
        let start = SIGNATURES_OFFSET + i * SIGNATURE_LENGTH;
        signatures.push(metadata.get(start..start + SIGNATURE_LENGTH)?);
    }
    Some(Parsed {
        hook,
        root,
        index,
        signatures,
    })
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader::new(data);
    let threshold = reader.u8();

    // Arbitrary metadata.
    let metadata = reader.rest();
    match (
        dusk_parse(metadata, threshold),
        solidity_parse(metadata, threshold),
    ) {
        (Ok(dusk), Some(solidity)) => assert_eq!(dusk, solidity),
        (Ok(dusk), None) => panic!("Dusk accepts what Solidity refuses: {dusk:?}"),
        (Err(reason), Some(_)) => {
            assert_eq!(reason, "MultisigISM: metadata signature length mismatch")
        }
        (Err(_), None) => {}
    }

    // The relayer's layout, with `count` signatures.
    let mut reader = Reader::new(data);
    let threshold = reader.u8();
    let count = usize::from(reader.u8());
    let hook: [u8; 32] = reader.array();
    let root: [u8; 32] = reader.array();
    let index = reader.u32();
    let mut built = Vec::with_capacity(SIGNATURES_OFFSET + count * SIGNATURE_LENGTH);
    built.extend_from_slice(&hook);
    built.extend_from_slice(&root);
    built.extend_from_slice(&index.to_be_bytes());
    for _ in 0..count {
        built.extend_from_slice(&reader.array::<SIGNATURE_LENGTH>());
    }
    match dusk_parse(&built, threshold) {
        Ok(parsed) => {
            assert!(threshold > 0 && count >= usize::from(threshold));
            assert_eq!(parsed.hook, hook.as_slice());
            assert_eq!(parsed.root, root.as_slice());
            assert_eq!(parsed.index, index);
            for (i, signature) in parsed.signatures.iter().enumerate() {
                let start = SIGNATURES_OFFSET + i * SIGNATURE_LENGTH;
                assert_eq!(*signature, &built[start..start + SIGNATURE_LENGTH]);
            }
            assert_eq!(solidity_parse(&built, threshold), Some(parsed));
        }
        Err(_) => assert!(threshold == 0 || count < usize::from(threshold)),
    }
});
