// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Incremental Merkle tree for Hyperlane on Dusk.

//! Depth-32 incremental Merkle tree using keccak256.
//!
//! This implements the same algorithm as `MerkleLib.sol` in the Hyperlane
//! Solidity contracts and `accumulator::incremental` in `hyperlane-core`.
//! The zero hashes are identical to ensure cross-chain compatibility.

use bytecheck::CheckBytes;
use rkyv::{Archive, Deserialize, Serialize};

use crate::message::keccak256;

/// Depth of the Merkle tree (supports up to 2^32 - 1 leaves).
pub const TREE_DEPTH: usize = 32;

/// Precomputed zero hashes for each level of the tree.
///
/// `ZERO_HASHES[0]` is `[0u8; 32]` (the empty leaf).
/// `ZERO_HASHES[i+1]` is `keccak256(ZERO_HASHES[i] || ZERO_HASHES[i])`.
///
/// These match exactly the constants in `hyperlane-core::accumulator::zero_hashes`.
pub const ZERO_HASHES: [[u8; 32]; TREE_DEPTH + 1] = [
    // Z_0
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    // Z_1
    [173, 50, 40, 182, 118, 247, 211, 205, 66, 132, 165, 68, 63, 23, 241, 150, 43, 54, 228, 145, 179, 10, 64, 178, 64, 88, 73, 229, 151, 186, 95, 181],
    // Z_2
    [180, 193, 25, 81, 149, 124, 111, 143, 100, 44, 74, 246, 28, 214, 178, 70, 64, 254, 198, 220, 127, 198, 7, 238, 130, 6, 169, 158, 146, 65, 13, 48],
    // Z_3
    [33, 221, 185, 163, 86, 129, 92, 63, 172, 16, 38, 182, 222, 197, 223, 49, 36, 175, 186, 219, 72, 92, 155, 165, 163, 227, 57, 138, 4, 183, 186, 133],
    // Z_4
    [229, 135, 105, 179, 42, 27, 234, 241, 234, 39, 55, 90, 68, 9, 90, 13, 31, 182, 100, 206, 45, 211, 88, 231, 252, 191, 183, 140, 38, 161, 147, 68],
    // Z_5
    [14, 176, 30, 191, 201, 237, 39, 80, 12, 212, 223, 201, 121, 39, 45, 31, 9, 19, 204, 159, 102, 84, 13, 126, 128, 5, 129, 17, 9, 225, 207, 45],
    // Z_6
    [136, 124, 34, 189, 135, 80, 211, 64, 22, 172, 60, 102, 181, 255, 16, 45, 172, 221, 115, 246, 176, 20, 231, 16, 181, 30, 128, 34, 175, 154, 25, 104],
    // Z_7
    [255, 215, 1, 87, 228, 128, 99, 252, 51, 201, 122, 5, 15, 127, 100, 2, 51, 191, 100, 108, 201, 141, 149, 36, 198, 185, 43, 207, 58, 181, 111, 131],
    // Z_8
    [152, 103, 204, 95, 127, 25, 107, 147, 186, 225, 226, 126, 99, 32, 116, 36, 69, 210, 144, 242, 38, 56, 39, 73, 139, 84, 254, 197, 57, 247, 86, 175],
    // Z_9
    [206, 250, 212, 229, 8, 192, 152, 185, 167, 225, 216, 254, 177, 153, 85, 251, 2, 186, 150, 117, 88, 80, 120, 113, 9, 105, 211, 68, 15, 80, 84, 224],
    // Z_10
    [249, 220, 62, 127, 224, 22, 224, 80, 239, 242, 96, 51, 79, 24, 165, 212, 254, 57, 29, 130, 9, 35, 25, 245, 150, 79, 46, 46, 183, 193, 195, 165],
    // Z_11
    [248, 177, 58, 73, 226, 130, 246, 9, 195, 23, 168, 51, 251, 141, 151, 109, 17, 81, 124, 87, 29, 18, 33, 162, 101, 210, 90, 247, 120, 236, 248, 146],
    // Z_12
    [52, 144, 198, 206, 235, 69, 10, 236, 220, 130, 226, 130, 147, 3, 29, 16, 199, 215, 59, 248, 94, 87, 191, 4, 26, 151, 54, 10, 162, 197, 217, 156],
    // Z_13
    [193, 223, 130, 217, 196, 184, 116, 19, 234, 226, 239, 4, 143, 148, 180, 211, 85, 76, 234, 115, 217, 43, 15, 122, 249, 110, 2, 113, 198, 145, 226, 187],
    // Z_14
    [92, 103, 173, 215, 198, 202, 243, 2, 37, 106, 222, 223, 122, 177, 20, 218, 10, 207, 232, 112, 212, 73, 163, 164, 137, 247, 129, 214, 89, 232, 190, 204],
    // Z_15
    [218, 123, 206, 159, 78, 134, 24, 182, 189, 47, 65, 50, 206, 121, 140, 220, 122, 96, 231, 225, 70, 10, 114, 153, 227, 198, 52, 42, 87, 150, 38, 210],
    // Z_16
    [39, 51, 229, 15, 82, 110, 194, 250, 25, 162, 43, 49, 232, 237, 80, 242, 60, 209, 253, 249, 76, 145, 84, 237, 58, 118, 9, 162, 241, 255, 152, 31],
    // Z_17
    [225, 211, 181, 200, 7, 178, 129, 228, 104, 60, 198, 214, 49, 92, 249, 91, 154, 222, 134, 65, 222, 252, 179, 35, 114, 241, 193, 38, 227, 152, 239, 122],
    // Z_18
    [90, 45, 206, 10, 138, 127, 104, 187, 116, 86, 15, 143, 113, 131, 124, 44, 46, 187, 203, 247, 255, 251, 66, 174, 24, 150, 241, 63, 124, 116, 121, 160],
    // Z_19
    [180, 106, 40, 182, 245, 85, 64, 248, 148, 68, 246, 61, 224, 55, 142, 61, 18, 27, 224, 158, 6, 204, 157, 237, 28, 32, 230, 88, 118, 211, 106, 160],
    // Z_20
    [198, 94, 150, 69, 100, 71, 134, 182, 32, 226, 221, 42, 214, 72, 221, 252, 191, 74, 126, 91, 26, 58, 78, 207, 231, 246, 70, 103, 163, 240, 183, 226],
    // Z_21
    [244, 65, 133, 136, 237, 53, 162, 69, 140, 255, 235, 57, 185, 61, 38, 241, 141, 42, 177, 59, 220, 230, 174, 229, 142, 123, 153, 53, 158, 194, 223, 217],
    // Z_22
    [90, 156, 22, 220, 0, 214, 239, 24, 183, 147, 58, 111, 141, 198, 92, 203, 85, 102, 113, 56, 119, 111, 125, 234, 16, 16, 112, 220, 135, 150, 227, 119],
    // Z_23
    [77, 248, 79, 64, 174, 12, 130, 41, 208, 214, 6, 158, 92, 143, 57, 167, 194, 153, 103, 122, 9, 211, 103, 252, 123, 5, 227, 188, 56, 14, 230, 82],
    // Z_24
    [205, 199, 37, 149, 247, 76, 123, 16, 67, 208, 225, 255, 186, 183, 52, 100, 140, 131, 141, 251, 5, 39, 217, 113, 182, 2, 188, 33, 108, 150, 25, 239],
    // Z_25
    [10, 191, 90, 201, 116, 161, 237, 87, 244, 5, 10, 165, 16, 221, 156, 116, 245, 8, 39, 123, 57, 215, 151, 59, 178, 223, 204, 197, 238, 176, 97, 141],
    // Z_26
    [184, 205, 116, 4, 111, 243, 55, 240, 167, 191, 44, 142, 3, 225, 15, 100, 44, 24, 134, 121, 141, 113, 128, 106, 177, 232, 136, 217, 229, 238, 135, 208],
    // Z_27
    [131, 140, 86, 85, 203, 33, 198, 203, 131, 49, 59, 90, 99, 17, 117, 223, 244, 150, 55, 114, 204, 233, 16, 129, 136, 179, 74, 200, 124, 129, 196, 30],
    // Z_28
    [102, 46, 228, 221, 45, 215, 178, 188, 112, 121, 97, 177, 230, 70, 196, 4, 118, 105, 220, 182, 88, 79, 13, 141, 119, 13, 175, 93, 126, 125, 235, 46],
    // Z_29
    [56, 138, 178, 14, 37, 115, 209, 113, 168, 129, 8, 231, 157, 130, 14, 152, 242, 108, 11, 132, 170, 139, 47, 74, 164, 150, 141, 187, 129, 142, 163, 34],
    // Z_30
    [147, 35, 124, 80, 186, 117, 238, 72, 95, 76, 34, 173, 242, 247, 65, 64, 11, 223, 141, 106, 156, 199, 223, 126, 202, 229, 118, 34, 22, 101, 215, 53],
    // Z_31
    [132, 72, 129, 139, 180, 174, 69, 98, 132, 158, 148, 158, 23, 172, 22, 224, 190, 22, 104, 142, 21, 107, 92, 241, 94, 9, 140, 98, 124, 0, 86, 169],
    // Z_32
    [39, 174, 91, 160, 141, 114, 145, 201, 108, 140, 189, 220, 193, 72, 191, 72, 166, 214, 140, 121, 116, 185, 67, 86, 245, 55, 84, 239, 97, 113, 215, 87],
];

/// The root of an empty tree (Z_32).
pub const INITIAL_ROOT: [u8; 32] = ZERO_HASHES[TREE_DEPTH];

/// An incremental Merkle tree with depth 32 and keccak256 hashing.
///
/// This matches the algorithm in `hyperlane-core::accumulator::incremental::IncrementalMerkle`
/// exactly, ensuring cross-chain compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[archive_attr(derive(CheckBytes))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IncrementalMerkle {
    /// The branch nodes (one per level).
    pub branch: [[u8; 32]; TREE_DEPTH],
    /// Number of leaves inserted.
    pub count: u32,
}

impl IncrementalMerkle {
    /// Create a new empty incremental Merkle tree.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            branch: [[0u8; 32]; TREE_DEPTH],
            count: 0,
        }
    }

    /// Insert a new leaf into the tree.
    ///
    /// This matches `IncrementalMerkle::ingest` in hyperlane-core.
    ///
    /// # Panics
    /// Panics if the tree is full (2^32 - 1 leaves).
    pub fn insert(&mut self, element: [u8; 32]) {
        let mut node = element;
        assert!(
            (self.count as usize) < u32::MAX as usize,
            "Merkle tree full"
        );
        self.count += 1;
        let mut size = self.count as usize;

        for i in 0..TREE_DEPTH {
            if (size & 1) == 1 {
                self.branch[i] = node;
                return;
            }
            node = hash_pair(self.branch[i], node);
            size /= 2;
        }
    }

    /// Compute the current root of the tree.
    ///
    /// This matches `IncrementalMerkle::root` in hyperlane-core.
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        let mut node = [0u8; 32];
        let mut size = self.count as usize;

        for i in 0..TREE_DEPTH {
            if (size & 1) == 1 {
                node = hash_pair(self.branch[i], node);
            } else {
                node = hash_pair(node, ZERO_HASHES[i]);
            }
            size /= 2;
        }

        node
    }

    /// Get the latest checkpoint (root, index).
    ///
    /// Returns `None` if the tree is empty.
    #[must_use]
    pub fn latest_checkpoint(&self) -> Option<([u8; 32], u32)> {
        if self.count == 0 {
            return None;
        }
        Some((self.root(), self.count - 1))
    }
}

/// Hash two 32-byte values together: `keccak256(left || right)`.
#[must_use]
pub fn hash_pair(left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    let mut combined = [0u8; 64];
    combined[..32].copy_from_slice(&left);
    combined[32..].copy_from_slice(&right);
    keccak256(&combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_hashes_chain() {
        // Verify Z_0 is all zeros
        assert_eq!(ZERO_HASHES[0], [0u8; 32]);

        // Verify each subsequent zero hash is correctly computed
        for i in 0..TREE_DEPTH {
            let expected = hash_pair(ZERO_HASHES[i], ZERO_HASHES[i]);
            assert_eq!(
                ZERO_HASHES[i + 1], expected,
                "ZERO_HASHES[{}] doesn't match computed value",
                i + 1
            );
        }
    }

    #[test]
    fn test_empty_tree_root_matches_initial_root() {
        let tree = IncrementalMerkle::new();
        assert_eq!(tree.root(), INITIAL_ROOT);
    }

    #[test]
    fn test_initial_root_value() {
        // This matches the INITIAL_ROOT constant in hyperlane-core:
        // 0x27ae5ba08d7291c96c8cbddcc148bf48a6d68c7974b94356f53754ef6171d757
        assert_eq!(
            INITIAL_ROOT,
            [
                39, 174, 91, 160, 141, 114, 145, 201, 108, 140, 189, 220, 193, 72, 191, 72, 166,
                214, 140, 121, 116, 185, 67, 86, 245, 55, 84, 239, 97, 113, 215, 87,
            ]
        );
    }

    #[test]
    fn test_insert_one() {
        let mut tree = IncrementalMerkle::new();
        let leaf = keccak256(b"test leaf");
        tree.insert(leaf);
        assert_eq!(tree.count, 1);

        let checkpoint = tree.latest_checkpoint();
        assert!(checkpoint.is_some());
        let (root, index) = checkpoint.unwrap();
        assert_eq!(index, 0);
        assert_ne!(root, INITIAL_ROOT);
    }

    #[test]
    fn test_insert_two_different_roots() {
        let mut tree = IncrementalMerkle::new();
        tree.insert(keccak256(b"leaf 0"));
        let root1 = tree.root();

        tree.insert(keccak256(b"leaf 1"));
        let root2 = tree.root();

        assert_ne!(root1, root2);
        assert_eq!(tree.count, 2);
    }

    #[test]
    fn test_deterministic() {
        let mut tree1 = IncrementalMerkle::new();
        let mut tree2 = IncrementalMerkle::new();

        for i in 0..10u32 {
            let leaf = keccak256(&i.to_be_bytes());
            tree1.insert(leaf);
            tree2.insert(leaf);
        }

        assert_eq!(tree1.root(), tree2.root());
        assert_eq!(tree1.count, tree2.count);
    }

    #[test]
    fn test_empty_checkpoint_is_none() {
        let tree = IncrementalMerkle::new();
        assert!(tree.latest_checkpoint().is_none());
    }
}
