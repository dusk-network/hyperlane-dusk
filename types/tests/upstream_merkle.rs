// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Merkle hook's tree against the one a validator builds.
//!
//! Validators rebuild the hook's tree with hyperlane-core's
//! `accumulator::incremental::IncrementalMerkle`, replicated here from
//! `hyperlane-monorepo/rust/main/hyperlane-core/src/accumulator/` rather than
//! taken as a dependency. Its `default()` starts branch slot `l` at
//! `ZERO_HASHES[l]`; the hook's tree (`types::merkle::IncrementalMerkle`)
//! starts every slot at zero. Neither ever reads a slot before writing it, so
//! the roots agree, but the branch arrays differ in the slots the tree hasn't
//! written yet. docs/storage.md tells validators what to compare.

use hyperlane_dusk_types::merkle::{IncrementalMerkle, TREE_DEPTH};
use hyperlane_dusk_types::message::keccak256;

/// The counts the trees are compared at: one leaf, each side of a power of
/// two, and a count with several bits set.
const COUNTS: [usize; 10] = [1, 2, 3, 4, 5, 8, 31, 32, 33, 100];

/// hyperlane-core's `hash_concat`: keccak256 of the two nodes.
fn hash_concat(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    let mut both = [0u8; 64];
    both[..32].copy_from_slice(&left);
    both[32..].copy_from_slice(&right);
    keccak256(&both)
}

/// hyperlane-core's `ZERO_HASHES`, computed the way its own test computes
/// them: the empty leaf, then each level hashed with itself.
fn upstream_zero_hashes() -> [[u8; 32]; TREE_DEPTH + 1] {
    let mut hashes = [[0u8; 32]; TREE_DEPTH + 1];
    for i in 0..TREE_DEPTH {
        hashes[i + 1] = hash_concat(hashes[i], hashes[i]);
    }
    hashes
}

/// hyperlane-core's `IncrementalMerkle`: `ingest` and `root` as upstream
/// writes them.
struct UpstreamTree {
    branch: [[u8; 32]; TREE_DEPTH],
    count: usize,
    zero_hashes: [[u8; 32]; TREE_DEPTH + 1],
}

impl UpstreamTree {
    /// `IncrementalMerkle::default()`: slot `l` starts at `ZERO_HASHES[l]`.
    fn default() -> Self {
        let zero_hashes = upstream_zero_hashes();
        let mut branch = [[0u8; 32]; TREE_DEPTH];
        branch.copy_from_slice(&zero_hashes[..TREE_DEPTH]);
        Self {
            branch,
            count: 0,
            zero_hashes,
        }
    }

    /// The same tree with every slot starting at zero, as the hook's do.
    fn zero_initialized() -> Self {
        Self {
            branch: [[0u8; 32]; TREE_DEPTH],
            count: 0,
            zero_hashes: upstream_zero_hashes(),
        }
    }

    fn ingest(&mut self, element: [u8; 32]) {
        let mut node = element;
        assert!(self.count < u32::MAX as usize);
        self.count += 1;
        let mut size = self.count;
        for i in 0..TREE_DEPTH {
            if (size & 1) == 1 {
                self.branch[i] = node;
                return;
            }
            node = hash_concat(self.branch[i], node);
            size /= 2;
        }
    }

    fn root(&self) -> [u8; 32] {
        // Low zero bits of the count hash empty subtrees, which upstream
        // takes from the precomputed zero hashes instead of the branch.
        let skip = self.count.trailing_zeros() as usize;
        if skip >= TREE_DEPTH {
            return self.zero_hashes[TREE_DEPTH];
        }
        let mut node = self.zero_hashes[skip];
        let mut size = self.count >> skip;
        for (i, elem) in self.branch.iter().enumerate().skip(skip) {
            node = if (size & 1) == 1 {
                hash_concat(*elem, node)
            } else {
                hash_concat(node, self.zero_hashes[i])
            };
            size /= 2;
        }
        node
    }
}

fn leaf(index: usize) -> [u8; 32] {
    keccak256(&(index as u64).to_be_bytes())
}

/// The empty tree's root is hyperlane-core's `INITIAL_ROOT` in both.
#[test]
fn empty_trees_have_hyperlane_cores_initial_root() {
    const INITIAL_ROOT: [u8; 32] = [
        0x27, 0xae, 0x5b, 0xa0, 0x8d, 0x72, 0x91, 0xc9, 0x6c, 0x8c, 0xbd, 0xdc, 0xc1, 0x48, 0xbf,
        0x48, 0xa6, 0xd6, 0x8c, 0x79, 0x74, 0xb9, 0x43, 0x56, 0xf5, 0x37, 0x54, 0xef, 0x61, 0x71,
        0xd7, 0x57,
    ];
    assert_eq!(UpstreamTree::default().root(), INITIAL_ROOT);
    assert_eq!(IncrementalMerkle::new().root(), INITIAL_ROOT);
    assert_eq!(
        upstream_zero_hashes(),
        hyperlane_dusk_types::merkle::ZERO_HASHES
    );
}

/// A validator that rebuilds the hook's tree the upstream way gets the
/// hook's root and count at every count, though the branch arrays differ:
/// a slot is written first when the count reaches `2^l`, so every level
/// with `2^l` above the count still holds its initial value, zero in the
/// hook and `ZERO_HASHES[l]` upstream. The slots of the levels whose bit is
/// set in the count, the frontier the root is built from, agree, and a tree
/// built the upstream way from zeroed slots matches the hook's exactly.
#[test]
fn upstream_trees_have_the_hooks_root_but_not_its_branch() {
    let zero_hashes = upstream_zero_hashes();
    let mut dusk = IncrementalMerkle::new();
    let mut upstream = UpstreamTree::default();
    let mut zeroed = UpstreamTree::zero_initialized();
    let mut inserted = 0;
    for count in COUNTS {
        while inserted < count {
            let id = leaf(inserted);
            dusk.insert(id);
            upstream.ingest(id);
            zeroed.ingest(id);
            inserted += 1;
        }

        assert_eq!(dusk.count as usize, count);
        assert_eq!(upstream.count, count);
        assert_eq!(upstream.root(), dusk.root(), "root at count {count}");
        assert_eq!(zeroed.root(), dusk.root(), "root at count {count}");

        assert_ne!(upstream.branch, dusk.branch, "branch at count {count}");
        for level in 0..TREE_DEPTH {
            let written = (1usize << level) <= count;
            if written {
                assert_eq!(
                    upstream.branch[level], dusk.branch[level],
                    "count {count}, level {level}: written by both"
                );
            } else {
                assert_eq!(
                    dusk.branch[level], [0u8; 32],
                    "count {count}, level {level}"
                );
                assert_eq!(
                    upstream.branch[level], zero_hashes[level],
                    "count {count}, level {level}"
                );
            }
            if count & (1 << level) != 0 {
                assert!(written, "count {count}, level {level}");
            }
        }
        assert_eq!(zeroed.branch, dusk.branch, "zeroed branch at count {count}");
    }
}
