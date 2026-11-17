// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared pieces of the fuzz targets: an input reader, independent reference
//! implementations, and an allocator that records what a call allocates.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sha3::{Digest, Keccak256};

/// Reads typed values from fuzz input. Past the end it returns zeros, so
/// every input is a valid program.
pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.position >= self.data.len()
    }

    pub fn u8(&mut self) -> u8 {
        let byte = self.data.get(self.position).copied().unwrap_or(0);
        self.position += 1;
        byte
    }

    pub fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        for byte in &mut out {
            *byte = self.u8();
        }
        out
    }

    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.array())
    }

    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.array())
    }

    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.array())
    }

    /// Up to `max` bytes, the count read first.
    pub fn bytes(&mut self, max: usize) -> &'a [u8] {
        let wanted = usize::from(self.u16()) % (max + 1);
        let start = self.position.min(self.data.len());
        let end = start.saturating_add(wanted).min(self.data.len());
        self.position = start + wanted;
        &self.data[start..end]
    }

    /// The rest of the input.
    pub fn rest(&mut self) -> &'a [u8] {
        let start = self.position.min(self.data.len());
        self.position = self.data.len();
        &self.data[start..]
    }
}

/// keccak256 from the `sha3` crate, independent of the types crate's own.
pub fn reference_keccak(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

fn reference_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// The zero subtree roots, computed from scratch: `zeros[0]` is the empty
/// leaf, `zeros[l + 1] = keccak(zeros[l] || zeros[l])`.
pub fn reference_zero_hashes() -> [[u8; 32]; 33] {
    let mut zeros = [[0u8; 32]; 33];
    for level in 0..32 {
        zeros[level + 1] = reference_pair(&zeros[level], &zeros[level]);
    }
    zeros
}

/// Root of the depth-32 tree whose first leaves are `leaves` and whose other
/// leaves are zero, computed level by level: Hyperlane's `MerkleLib` without
/// any incremental state.
pub fn naive_root(leaves: &[[u8; 32]], zeros: &[[u8; 32]; 33]) -> [u8; 32] {
    naive_subtree_root(leaves, 32, zeros)
}

/// Root of the subtree of height `height` whose leaves are `leaves`, padded
/// with zero leaves. `leaves.len()` must be at most `2^height`.
pub fn naive_subtree_root(leaves: &[[u8; 32]], height: usize, zeros: &[[u8; 32]; 33]) -> [u8; 32] {
    assert!(height <= 32);
    assert!((leaves.len() as u64) <= 1u64 << height);
    if leaves.is_empty() {
        return zeros[height];
    }
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    for depth in 0..height {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            let right = pair.get(1).unwrap_or(&zeros[depth]);
            next.push(reference_pair(&pair[0], right));
        }
        level = next;
    }
    assert_eq!(level.len(), 1);
    level[0]
}

/// `abi.encodePacked(bytes32 root, uint32 index, ...)`-style checkpoint
/// digest, computed with the reference keccak: `CheckpointLib.digest` then
/// `ECDSA.toEthSignedMessageHash`.
pub fn reference_checkpoint_digest(
    origin: u32,
    merkle_tree_hook: &[u8; 32],
    root: &[u8; 32],
    index: u32,
    message_id: &[u8; 32],
) -> [u8; 32] {
    let mut domain = Keccak256::new();
    domain.update(origin.to_be_bytes());
    domain.update(merkle_tree_hook);
    domain.update(b"HYPERLANE");
    let domain: [u8; 32] = domain.finalize().into();
    let mut checkpoint = Keccak256::new();
    checkpoint.update(domain);
    checkpoint.update(root);
    checkpoint.update(index.to_be_bytes());
    checkpoint.update(message_id);
    let checkpoint: [u8; 32] = checkpoint.finalize().into();
    reference_eth_signed(&checkpoint)
}

/// `ValidatorAnnounce.getAnnouncementDigest`, with the reference keccak.
pub fn reference_announcement_digest(domain: u32, mailbox: &[u8; 32], location: &[u8]) -> [u8; 32] {
    let mut domain_hash = Keccak256::new();
    domain_hash.update(domain.to_be_bytes());
    domain_hash.update(mailbox);
    domain_hash.update(b"HYPERLANE_ANNOUNCEMENT");
    let domain_hash: [u8; 32] = domain_hash.finalize().into();
    let mut inner = Keccak256::new();
    inner.update(domain_hash);
    inner.update(location);
    let inner: [u8; 32] = inner.finalize().into();
    reference_eth_signed(&inner)
}

/// `ECDSA.toEthSignedMessageHash(bytes32)`.
pub fn reference_eth_signed(hash: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(b"\x19Ethereum Signed Message:\n32");
    hasher.update(hash);
    hasher.finalize().into()
}

/// A global allocator that can record the sizes a closure asks for, so a
/// target can compare what a push allocates with what the log predicted.
pub struct TrackingAllocator;

const MAX_TRACKED: usize = 16;
static ARMED: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static SIZES: [AtomicUsize; MAX_TRACKED] = [const { AtomicUsize::new(0) }; MAX_TRACKED];

fn record(size: usize) {
    if ARMED.load(Ordering::Relaxed) {
        let slot = COUNT.fetch_add(1, Ordering::Relaxed);
        if slot < MAX_TRACKED {
            SIZES[slot].store(size, Ordering::Relaxed);
        }
    }
}

// SAFETY: every call forwards to `System` with the same arguments; recording
// only touches atomics and never allocates.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Runs `f` and returns what it returned and the sizes of the allocations
/// (and reallocations, by their new size) it made, in order. Single-threaded
/// use only, as libFuzzer runs a target.
pub fn tracked<R>(f: impl FnOnce() -> R) -> (R, Vec<usize>) {
    COUNT.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::SeqCst);
    let result = f();
    ARMED.store(false, Ordering::SeqCst);
    let count = COUNT.load(Ordering::Relaxed);
    assert!(
        count <= MAX_TRACKED,
        "more allocations than tracked: {count}"
    );
    let sizes = SIZES[..count]
        .iter()
        .map(|size| size.load(Ordering::Relaxed))
        .collect();
    (result, sizes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naive_root_of_nothing_is_the_initial_root() {
        let zeros = reference_zero_hashes();
        assert_eq!(
            naive_root(&[], &zeros),
            hyperlane_dusk_types::merkle::INITIAL_ROOT
        );
    }
}
