// SPDX-License-Identifier: MIT OR Apache-2.0

//! Append-only storage for contract state that only grows.
//!
//! A contract's state lives in its wasm32 linear memory: at most 4 GiB, never
//! given back, and managed by dlmalloc. A `Vec` that doubles as it grows needs
//! one allocation as large as everything it holds, and no allocation can
//! reach 2 GiB (`isize::MAX` on wasm32), so a `Vec<[u8; 32]>` stops at 2^25
//! entries with most of memory still free, and each doubling allocates a new
//! block while the old one is still in use.
//!
//! [`ChunkedLog`] keeps its entries in fixed chunks of [`CHUNK_BYTES`]
//! instead. No entry moves once written. The only block that is ever
//! reallocated is the table of chunk pointers, which doubles and stays under
//! 768 KiB, and [`ChunkedLog::push_allocations`] says what the next push will
//! allocate before it happens, so a contract can refuse an entry it has no
//! room for. [`BlockHeights`] records the block of each entry of such a log,
//! once per block rather than once per entry.

use alloc::vec::Vec;
use core::mem::size_of;

/// Size in bytes of one chunk of a [`ChunkedLog`].
pub const CHUNK_BYTES: usize = 64 * 1024;

/// Entries of `size` bytes that fit in one chunk.
#[allow(clippy::cast_possible_truncation)] // At most CHUNK_BYTES, which fits a u32.
const fn entries_per_chunk(size: usize) -> u32 {
    assert!(
        size > 0 && size <= CHUNK_BYTES,
        "ChunkedLog: an entry must take 1 to CHUNK_BYTES bytes"
    );
    (CHUNK_BYTES / size) as u32
}

/// How many chunk slots to add to a full chunk table: as many as it has,
/// and 4 for the first, so the table doubles.
fn table_growth(capacity: usize) -> usize {
    capacity.max(4)
}

/// An append-only log of `T` stored in chunks of [`CHUNK_BYTES`].
///
/// Entries are addressed by a `u32` index, like the nonces and leaf indexes
/// the contracts expose. A chunk is allocated, at its full size, when the
/// first entry that falls into it is pushed. The table of chunks grows by
/// doubling; it holds one `Vec` header per chunk, at most 768 KiB for a log
/// that fills 4 GiB.
///
/// Not `Clone`: a cloned `Vec` has no spare capacity, so a clone's last
/// chunk would reallocate on its next push.
#[derive(Debug)]
pub struct ChunkedLog<T> {
    chunks: Vec<Vec<T>>,
    len: u32,
}

impl<T> Default for ChunkedLog<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ChunkedLog<T> {
    /// Creates an empty log. Allocates nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }

    /// Number of entries.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// Whether the log has no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T: Copy> ChunkedLog<T> {
    /// Entries per chunk: [`CHUNK_BYTES`] divided by the size of `T`. A type
    /// larger than a chunk doesn't compile.
    pub const PER_CHUNK: u32 = entries_per_chunk(size_of::<T>());

    /// Returns the entry at `index`, or `None` past the end.
    #[must_use]
    pub fn get(&self, index: u32) -> Option<T> {
        if index >= self.len {
            return None;
        }
        let chunk = &self.chunks[(index / Self::PER_CHUNK) as usize];
        Some(chunk[(index % Self::PER_CHUNK) as usize])
    }

    /// Returns the last entry, or `None` if the log is empty.
    #[must_use]
    pub fn last(&self) -> Option<T> {
        self.len.checked_sub(1).and_then(|index| self.get(index))
    }

    /// Returns the entries from `start` up to, not including, `start + limit`,
    /// or to the end of the log. Empty if `start` is at or past the end.
    #[must_use]
    pub fn page(&self, start: u32, limit: u32) -> Vec<T> {
        let end = start.saturating_add(limit).min(self.len);
        let mut page = Vec::with_capacity(end.saturating_sub(start) as usize);
        let mut index = start;
        while index < end {
            let chunk = &self.chunks[(index / Self::PER_CHUNK) as usize];
            let offset = index % Self::PER_CHUNK;
            let take = (end - index).min(Self::PER_CHUNK - offset);
            page.extend_from_slice(&chunk[offset as usize..(offset + take) as usize]);
            index += take;
        }
        page
    }

    /// Appends `value` and returns its index.
    ///
    /// # Panics
    ///
    /// Panics if the log already holds `u32::MAX` entries, or if memory for a
    /// new chunk can't be allocated.
    pub fn push(&mut self, value: T) -> u32 {
        let index = self.len;
        let len = index.checked_add(1).expect("ChunkedLog: log is full");
        if index.is_multiple_of(Self::PER_CHUNK) {
            if self.chunks.len() == self.chunks.capacity() {
                // An exact request, so the table allocation is the size
                // `push_allocations` reports.
                self.chunks
                    .reserve_exact(table_growth(self.chunks.capacity()));
            }
            self.chunks
                .push(Vec::with_capacity(Self::PER_CHUNK as usize));
        }
        // Every chunk is created with room for PER_CHUNK entries, so this
        // never reallocates.
        self.chunks
            .last_mut()
            .expect("ChunkedLog: the last chunk exists")
            .push(value);
        self.len = len;
        index
    }

    /// The sizes in bytes of the allocations the next [`push`](Self::push)
    /// makes: `[chunk, table]`. The first is a new chunk, when the last one
    /// is full; the second a larger chunk table, when that is full too. Zero
    /// means no allocation.
    #[must_use]
    pub fn push_allocations(&self) -> [usize; 2] {
        if !self.len.is_multiple_of(Self::PER_CHUNK) {
            return [0, 0];
        }
        let table = if self.chunks.len() == self.chunks.capacity() {
            let capacity = self.chunks.capacity();
            (capacity + table_growth(capacity)) * size_of::<Vec<T>>()
        } else {
            0
        };
        [Self::PER_CHUNK as usize * size_of::<T>(), table]
    }
}

/// The block height of each entry of a log, stored once per block.
///
/// Each block that adds entries adds one run: the index of its first entry
/// and its height. The height of an entry is that of the last run starting
/// at or before it, found by binary search. A run takes 16 bytes, so a log
/// that gets many entries per block stores almost nothing here, and one that
/// gets one entry per block at most 16 bytes per entry.
#[derive(Debug, Default)]
pub struct BlockHeights {
    runs: ChunkedLog<HeightRun>,
    len: u32,
}

/// The entries from `first_index` until the next run were added at `height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeightRun {
    first_index: u32,
    height: u64,
}

impl BlockHeights {
    /// Creates an empty record. Allocates nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            runs: ChunkedLog::new(),
            len: 0,
        }
    }

    /// Number of entries recorded.
    #[must_use]
    pub const fn len(&self) -> u32 {
        self.len
    }

    /// Whether no entry has been recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of runs, at most one per block that added entries.
    #[must_use]
    pub const fn runs(&self) -> u32 {
        self.runs.len()
    }

    /// Records that the next entry was added at `height`, and returns its
    /// index.
    ///
    /// # Panics
    ///
    /// Panics if `u32::MAX` entries are already recorded, or if memory for a
    /// new chunk of runs can't be allocated.
    pub fn push(&mut self, height: u64) -> u32 {
        let index = self.len;
        let len = index.checked_add(1).expect("BlockHeights: log is full");
        if !self.continues_last_run(height) {
            self.runs.push(HeightRun {
                first_index: index,
                height,
            });
        }
        self.len = len;
        index
    }

    /// Returns the block height of entry `index`, or `None` past the end.
    #[must_use]
    pub fn get(&self, index: u32) -> Option<u64> {
        if index >= self.len {
            return None;
        }
        // The first run starts at index 0, so `low` always qualifies; find
        // the last run that starts at or before `index`.
        let mut low = 0;
        let mut high = self.runs.len();
        while high - low > 1 {
            let middle = low + (high - low) / 2;
            if self.run(middle).first_index <= index {
                low = middle;
            } else {
                high = middle;
            }
        }
        Some(self.run(low).height)
    }

    /// The allocations the next [`push`](Self::push) at `height` makes, as
    /// [`ChunkedLog::push_allocations`]. None unless it starts a new run.
    #[must_use]
    pub fn push_allocations(&self, height: u64) -> [usize; 2] {
        if self.continues_last_run(height) {
            [0, 0]
        } else {
            self.runs.push_allocations()
        }
    }

    fn continues_last_run(&self, height: u64) -> bool {
        self.runs.last().is_some_and(|run| run.height == height)
    }

    fn run(&self, index: u32) -> HeightRun {
        self.runs
            .get(index)
            .expect("BlockHeights: run index is in range")
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use alloc::vec;

    /// Deterministic generator for the property tests (splitmix64).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, bound: u32) -> u32 {
            u32::try_from(self.next() % u64::from(bound)).unwrap()
        }

        fn bytes<const N: usize>(&mut self) -> [u8; N] {
            let mut out = [0u8; N];
            for chunk in out.chunks_mut(8) {
                chunk.copy_from_slice(&self.next().to_le_bytes()[..chunk.len()]);
            }
            out
        }
    }

    fn index(position: usize) -> u32 {
        u32::try_from(position).unwrap()
    }

    /// Checks every way of reading `log` against the `Vec` it should equal.
    fn assert_matches<T: Copy + PartialEq + core::fmt::Debug>(
        log: &ChunkedLog<T>,
        model: &[T],
        rng: &mut Rng,
    ) {
        assert_eq!(log.len(), index(model.len()));
        assert_eq!(log.is_empty(), model.is_empty());
        assert_eq!(log.last(), model.last().copied());
        for (position, value) in model.iter().enumerate() {
            assert_eq!(log.get(index(position)), Some(*value), "index {position}");
        }
        assert_eq!(log.get(index(model.len())), None);
        assert_eq!(log.get(u32::MAX), None);
        for _ in 0..64 {
            let start = rng.below(index(model.len()) + 2);
            let limit = match rng.below(4) {
                0 => 0,
                1 => u32::MAX,
                2 => ChunkedLog::<T>::PER_CHUNK + 1,
                _ => rng.below(300),
            };
            let end = (start as usize)
                .saturating_add(limit as usize)
                .min(model.len());
            let expected = model.get(start as usize..end).unwrap_or(&[]);
            assert_eq!(log.page(start, limit), expected, "page({start}, {limit})");
        }
    }

    /// Pushes `count` random entries into a log and a `Vec`, comparing them
    /// as it goes, and checks that every push allocates what
    /// `push_allocations` said it would.
    fn check_against_vec<T, F>(count: u32, seed: u64, mut entry: F)
    where
        T: Copy + PartialEq + core::fmt::Debug,
        F: FnMut(&mut Rng) -> T,
    {
        let mut rng = Rng(seed);
        let mut log = ChunkedLog::new();
        let mut model = Vec::new();
        for step in 0..count {
            let [chunk, table] = log.push_allocations();
            let chunks_before = log.chunks.len();
            let table_before = log.chunks.capacity();
            let value = entry(&mut rng);
            assert_eq!(log.push(value), step);
            model.push(value);

            let new_chunk = log.chunks.len() > chunks_before;
            assert_eq!(new_chunk, chunk > 0, "step {step}");
            if new_chunk {
                let last = log.chunks.last().unwrap();
                assert_eq!(last.capacity() * size_of::<T>(), chunk);
                assert!(chunk <= CHUNK_BYTES);
            }
            let new_table = log.chunks.capacity() != table_before;
            assert_eq!(new_table, table > 0, "step {step}");
            if new_table {
                assert_eq!(log.chunks.capacity() * size_of::<Vec<T>>(), table);
            }
            // No chunk ever grows past what it was created with.
            assert!(log
                .chunks
                .iter()
                .all(|c| c.capacity() == ChunkedLog::<T>::PER_CHUNK as usize));
            if step % 997 == 0 {
                assert_matches(&log, &model, &mut rng);
            }
        }
        assert_matches(&log, &model, &mut rng);
    }

    #[test]
    fn per_chunk_fills_64_kib() {
        assert_eq!(ChunkedLog::<[u8; 32]>::PER_CHUNK, 2048);
        assert_eq!(ChunkedLog::<[u8; 20]>::PER_CHUNK, 3276);
        assert_eq!(ChunkedLog::<u64>::PER_CHUNK, 8192);
        assert_eq!(ChunkedLog::<[u8; CHUNK_BYTES]>::PER_CHUNK, 1);
    }

    #[test]
    fn an_empty_log_allocates_nothing_until_the_first_push() {
        let log = ChunkedLog::<[u8; 32]>::new();
        assert_eq!(log.chunks.capacity(), 0);
        assert_eq!(log.page(0, 10), Vec::<[u8; 32]>::new());
        assert_eq!(
            log.push_allocations(),
            [CHUNK_BYTES, 4 * size_of::<Vec<[u8; 32]>>()]
        );
    }

    #[test]
    fn message_ids_match_a_vec_across_chunks() {
        check_against_vec(3 * 2048 + 17, 1, Rng::bytes::<32>);
    }

    #[test]
    fn addresses_match_a_vec_across_chunks() {
        // 3276 per chunk leaves 16 bytes of every chunk unused.
        check_against_vec(2 * 3276 + 5, 2, Rng::bytes::<20>);
    }

    #[test]
    fn large_entries_match_a_vec_across_many_chunks() {
        // Four 16 KiB entries per chunk: the chunk table grows many times.
        check_against_vec(4 * 70 + 3, 3, |rng| {
            let mut entry = [0u64; 2048];
            entry[0] = rng.next();
            entry[2047] = rng.next();
            entry
        });
    }

    #[test]
    fn random_lengths_match_a_vec() {
        // 2730 entries of 24 bytes to a chunk, so up to four chunks.
        for seed in 0..40 {
            let count = Rng(1000 + seed).below(4 * 2730);
            check_against_vec(count, seed, Rng::bytes::<24>);
        }
    }

    #[test]
    fn page_stops_at_the_end_and_spans_chunks() {
        let mut log = ChunkedLog::<u64>::new();
        for value in 0..10_000u64 {
            log.push(value);
        }
        let per_chunk = ChunkedLog::<u64>::PER_CHUNK;
        let page = log.page(per_chunk - 3, 6);
        assert_eq!(
            page,
            (u64::from(per_chunk) - 3..u64::from(per_chunk) + 3).collect::<Vec<_>>()
        );
        assert_eq!(log.page(9_998, 256), vec![9_998, 9_999]);
        assert_eq!(log.page(10_000, 256), Vec::<u64>::new());
        assert_eq!(log.page(20_000, 256), Vec::<u64>::new());
        assert_eq!(log.page(5, 0), Vec::<u64>::new());
    }

    /// Pushes heights into `BlockHeights` and a `Vec` and checks every index.
    fn check_heights(heights: &[u64]) {
        let mut record = BlockHeights::new();
        let mut runs = 0u32;
        for (position, &height) in heights.iter().enumerate() {
            let starts_run = position == 0 || heights[position - 1] != height;
            let [chunk, _] = record.push_allocations(height);
            assert_eq!(
                chunk > 0,
                starts_run && runs.is_multiple_of(ChunkedLog::<HeightRun>::PER_CHUNK),
                "index {position}"
            );
            assert_eq!(record.push(height), index(position));
            runs += u32::from(starts_run);
        }
        assert_eq!(record.len(), index(heights.len()));
        assert_eq!(record.runs(), runs);
        for (position, &height) in heights.iter().enumerate() {
            assert_eq!(
                record.get(index(position)),
                Some(height),
                "index {position}"
            );
        }
        assert_eq!(record.get(index(heights.len())), None);
        assert_eq!(record.get(u32::MAX), None);
    }

    #[test]
    fn heights_match_a_vec() {
        check_heights(&[]);
        check_heights(&[7]);
        check_heights(&[7, 7, 7]);
        check_heights(&[0, 1, 2, 3]);
        check_heights(&[u64::MAX, u64::MAX, 0]);
        let mut rng = Rng(42);
        for _ in 0..200 {
            let count = rng.below(2_000);
            let mut height = rng.next() >> 8;
            let mut heights = Vec::new();
            for _ in 0..count {
                match rng.below(10) {
                    0..=5 => {}
                    6..=8 => height += 1 + u64::from(rng.below(1_000)),
                    // Heights never go down on a chain, but nothing here
                    // depends on it.
                    _ => height = rng.next() >> 8,
                }
                heights.push(height);
            }
            check_heights(&heights);
        }
    }

    #[test]
    fn heights_cross_chunks_of_runs() {
        // One entry per block: one run per entry, three chunks of runs.
        let per_chunk = u64::from(ChunkedLog::<HeightRun>::PER_CHUNK);
        let heights: Vec<u64> = (1_000..1_000 + 2 * per_chunk + 9).collect();
        check_heights(&heights);
        // Many entries per block: few runs.
        let heights: Vec<u64> = (0..20_000u64).map(|i| 5 + i / 1_000).collect();
        check_heights(&heights);
    }

    #[test]
    fn a_run_takes_16_bytes() {
        assert_eq!(size_of::<HeightRun>(), 16);
        assert_eq!(ChunkedLog::<HeightRun>::PER_CHUNK, 4096);
    }
}
