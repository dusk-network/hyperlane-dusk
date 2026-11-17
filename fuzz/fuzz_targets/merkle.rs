// SPDX-License-Identifier: MIT OR Apache-2.0

//! `types::merkle::IncrementalMerkle`, the MerkleTreeHook's tree, against a
//! naive depth-32 tree built level by level from all the leaves, with an
//! independent keccak256:
//! - the root after every insertion the input asks to check, and at the end;
//! - `latest_checkpoint`;
//! - every branch slot whose bit is set in the count holds the root of the
//!   last complete subtree of that height (what storage.md tells validators
//!   they may compare with the hook's `tree()`);
//! - the zero hashes and the empty root.
//! Leaf counts are steered to either side of powers of two.

#![no_main]

use std::sync::OnceLock;

use hyperlane_dusk_fuzz::{naive_root, naive_subtree_root, reference_zero_hashes, Reader};
use hyperlane_dusk_types::merkle::{IncrementalMerkle, INITIAL_ROOT, TREE_DEPTH, ZERO_HASHES};
use libfuzzer_sys::fuzz_target;

fn zeros() -> &'static [[u8; 32]; 33] {
    static ZEROS: OnceLock<[[u8; 32]; 33]> = OnceLock::new();
    ZEROS.get_or_init(|| {
        let zeros = reference_zero_hashes();
        assert_eq!(zeros, ZERO_HASHES);
        assert_eq!(zeros[TREE_DEPTH], INITIAL_ROOT);
        zeros
    })
}

fn check(tree: &IncrementalMerkle, leaves: &[[u8; 32]]) {
    let zeros = zeros();
    let count = leaves.len();
    assert_eq!(tree.count as usize, count);
    let root = naive_root(leaves, zeros);
    assert_eq!(tree.root(), root, "root at count {count}");
    match tree.latest_checkpoint() {
        None => assert_eq!(count, 0),
        Some((checkpoint_root, index)) => {
            assert_eq!(checkpoint_root, root);
            assert_eq!(index as usize, count - 1);
        }
    }
    for level in 0..TREE_DEPTH {
        if count >> level & 1 == 1 {
            let start = ((count >> level) << level) - (1 << level);
            let subtree = naive_subtree_root(&leaves[start..start + (1 << level)], level, zeros);
            assert_eq!(
                tree.branch[level], subtree,
                "branch[{level}] at count {count}"
            );
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader::new(data);
    // Up to 2^13 + 1 leaves, often right at a power of two.
    let count = match reader.u8() % 4 {
        0 => {
            let power = 1usize << (reader.u8() % 14);
            match reader.u8() % 3 {
                0 => power - 1,
                1 => power,
                _ => power + 1,
            }
        }
        _ => usize::from(reader.u16()) % 600,
    };
    let checks: Vec<usize> = (0..4)
        .map(|_| usize::from(reader.u16()) % (count + 1))
        .collect();
    let mut tree = IncrementalMerkle::new();
    let mut leaves = Vec::with_capacity(count);
    check(&tree, &leaves);
    for n in 0..count {
        // Leaves from the input while it lasts, then distinct generated ones;
        // a zero leaf is as valid as any.
        let leaf = if reader.is_empty() {
            let mut leaf = [0u8; 32];
            leaf[..8].copy_from_slice(&(n as u64).to_le_bytes());
            leaf[31] = 0xA5;
            leaf
        } else {
            reader.array()
        };
        tree.insert(leaf);
        leaves.push(leaf);
        if checks.contains(&leaves.len()) {
            check(&tree, &leaves);
        }
    }
    check(&tree, &leaves);
});
