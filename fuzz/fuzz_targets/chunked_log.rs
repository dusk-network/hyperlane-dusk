// SPDX-License-Identifier: MIT OR Apache-2.0

//! `types::storage::ChunkedLog` against a `Vec`, for the entry types the
//! contracts store (message IDs, validator addresses, gas payment records)
//! and sizes that leave part of a chunk unused.
//!
//! Every push is run under an allocator that records what it asks for, and
//! must allocate exactly what `push_allocations` said it would: the
//! Mailbox's and the IGP's 3 GiB limits count those sizes before they push.

#![no_main]

use core::fmt::Debug;

use hyperlane_dusk_fuzz::{tracked, Reader, TrackingAllocator};
use hyperlane_dusk_types::storage::{ChunkedLog, CHUNK_BYTES};
use hyperlane_dusk_types::{EthAddress, GasPaymentRecord};
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

/// Entries pushed per input, at most: enough to cross several chunks and
/// table doublings for every entry size below.
const MAX_ENTRIES: u32 = 40_000;
/// For the 64 KiB entries, each its own chunk.
const MAX_HUGE_ENTRIES: u32 = 300;

trait Entry: Copy + PartialEq + Debug {
    fn read(reader: &mut Reader) -> Self;
    /// A distinct value for the `n`th generated entry.
    fn generated(n: u32) -> Self;
}

impl<const N: usize> Entry for [u8; N] {
    fn read(reader: &mut Reader) -> Self {
        reader.array()
    }
    fn generated(n: u32) -> Self {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = (n as usize).wrapping_mul(31).wrapping_add(i) as u8;
        }
        out[..N.min(4)].copy_from_slice(&n.to_le_bytes()[..N.min(4)]);
        out
    }
}

impl Entry for EthAddress {
    fn read(reader: &mut Reader) -> Self {
        EthAddress(reader.array())
    }
    fn generated(n: u32) -> Self {
        EthAddress(<[u8; 20]>::generated(n))
    }
}

impl Entry for GasPaymentRecord {
    fn read(reader: &mut Reader) -> Self {
        GasPaymentRecord {
            message_id: reader.array(),
            destination: reader.u32(),
            gas_limit: reader.u64(),
            payment: reader.u64(),
            block_height: reader.u64(),
        }
    }
    fn generated(n: u32) -> Self {
        GasPaymentRecord {
            message_id: <[u8; 32]>::generated(n),
            destination: n,
            gas_limit: u64::from(n) << 7,
            payment: u64::from(n) * 3,
            block_height: u64::from(n / 5),
        }
    }
}

fn push_checked<T: Entry>(log: &mut ChunkedLog<T>, model: &mut Vec<T>, value: T) {
    let [chunk, table] = log.push_allocations();
    assert!(chunk <= CHUNK_BYTES);
    assert!(chunk == 0 || chunk == ChunkedLog::<T>::PER_CHUNK as usize * size_of::<T>());
    assert_eq!(
        chunk > 0,
        model.len() % ChunkedLog::<T>::PER_CHUNK as usize == 0
    );
    assert!(
        table == 0 || chunk > 0,
        "the table only grows with a new chunk"
    );
    let (index, sizes) = tracked(|| log.push(value));
    // The table is reserved before the chunk is allocated.
    let expected: Vec<usize> = [table, chunk]
        .into_iter()
        .filter(|size| *size > 0)
        .collect();
    assert_eq!(
        sizes,
        expected,
        "push {} allocated other than predicted",
        model.len()
    );
    assert_eq!(index as usize, model.len());
    model.push(value);
}

fn check_reads<T: Entry>(log: &ChunkedLog<T>, model: &[T], reader: &mut Reader) {
    let len = model.len() as u32;
    assert_eq!(log.len(), len);
    assert_eq!(log.is_empty(), model.is_empty());
    assert_eq!(log.last(), model.last().copied());
    let pick = |reader: &mut Reader| -> u32 {
        match reader.u8() % 6 {
            0 => len,
            1 => len.saturating_sub(1),
            2 => u32::MAX,
            3 => ChunkedLog::<T>::PER_CHUNK.saturating_mul(u32::from(reader.u8() % 8)),
            4 => (ChunkedLog::<T>::PER_CHUNK.saturating_mul(u32::from(reader.u8() % 8)))
                .saturating_sub(1),
            _ => reader.u32() % len.saturating_add(2),
        }
    };
    for _ in 0..4 {
        let index = pick(reader);
        assert_eq!(
            log.get(index),
            model.get(index as usize).copied(),
            "get({index})"
        );
    }
    let start = pick(reader);
    let limit = match reader.u8() % 5 {
        0 => 0,
        1 => u32::MAX,
        2 => 256,
        3 => ChunkedLog::<T>::PER_CHUNK + 1,
        _ => u32::from(reader.u16()),
    };
    let end = (start as usize)
        .saturating_add(limit as usize)
        .min(model.len());
    let expected = model.get(start as usize..end).unwrap_or(&[]);
    assert_eq!(log.page(start, limit), expected, "page({start}, {limit})");
}

fn run<T: Entry>(reader: &mut Reader, max_entries: u32) {
    let mut log = ChunkedLog::<T>::new();
    let mut model = Vec::new();
    assert_eq!(log.page(0, 10), Vec::<T>::new());
    let mut generated = 0u32;
    while !reader.is_empty() {
        match reader.u8() % 4 {
            0 => {
                if (model.len() as u32) < max_entries {
                    let value = T::read(reader);
                    push_checked(&mut log, &mut model, value);
                }
            }
            1 => {
                let wanted = u32::from(reader.u16()) % (2 * ChunkedLog::<T>::PER_CHUNK + 3);
                let room = max_entries.saturating_sub(model.len() as u32);
                for _ in 0..wanted.min(room) {
                    push_checked(&mut log, &mut model, T::generated(generated));
                    generated = generated.wrapping_add(1);
                }
            }
            _ => check_reads(&log, &model, reader),
        }
    }
    // A full comparison at the end.
    for (index, value) in model.iter().enumerate() {
        assert_eq!(log.get(index as u32), Some(*value));
    }
    assert_eq!(log.page(0, u32::MAX), model);
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader::new(data);
    match reader.u8() % 7 {
        0 => run::<[u8; 32]>(&mut reader, MAX_ENTRIES),
        1 => run::<EthAddress>(&mut reader, MAX_ENTRIES),
        2 => run::<GasPaymentRecord>(&mut reader, MAX_ENTRIES),
        3 => run::<[u8; 24]>(&mut reader, MAX_ENTRIES),
        4 => run::<[u8; 7]>(&mut reader, MAX_ENTRIES),
        5 => run::<[u8; CHUNK_BYTES / 3 + 1]>(&mut reader, MAX_HUGE_ENTRIES),
        _ => run::<[u8; CHUNK_BYTES]>(&mut reader, MAX_HUGE_ENTRIES),
    }
});
