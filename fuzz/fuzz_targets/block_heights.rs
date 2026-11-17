// SPDX-License-Identifier: MIT OR Apache-2.0

//! `types::storage::BlockHeights` against a `Vec<u64>` of one height per
//! entry: every lookup, the number of runs, and that every push allocates
//! exactly what `push_allocations(height)` predicted.

#![no_main]

use hyperlane_dusk_fuzz::{tracked, Reader, TrackingAllocator};
use hyperlane_dusk_types::storage::BlockHeights;
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

const MAX_ENTRIES: usize = 40_000;

fn push_checked(record: &mut BlockHeights, model: &mut Vec<u64>, runs: &mut u32, height: u64) {
    let starts_run = model.last() != Some(&height);
    let [chunk, table] = record.push_allocations(height);
    assert!(starts_run || (chunk == 0 && table == 0));
    let (index, sizes) = tracked(|| record.push(height));
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
    model.push(height);
    *runs += u32::from(starts_run);
    assert_eq!(record.runs(), *runs);
    assert_eq!(record.len() as usize, model.len());
}

fn check(record: &BlockHeights, model: &[u64], reader: &mut Reader) {
    let len = model.len() as u32;
    assert_eq!(record.is_empty(), model.is_empty());
    for _ in 0..4 {
        let index = match reader.u8() % 4 {
            0 => len,
            1 => len.saturating_sub(1),
            2 => u32::MAX,
            _ => reader.u32() % len.saturating_add(1),
        };
        assert_eq!(
            record.get(index),
            model.get(index as usize).copied(),
            "get({index})"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    let mut reader = Reader::new(data);
    let mut record = BlockHeights::new();
    let mut model: Vec<u64> = Vec::new();
    let mut runs = 0u32;
    let mut height = reader.u64() >> 1;
    while !reader.is_empty() {
        match reader.u8() % 6 {
            // Same block.
            0 => {
                if model.len() < MAX_ENTRIES {
                    push_checked(&mut record, &mut model, &mut runs, height);
                }
            }
            // A later block.
            1 => {
                height = height.saturating_add(1 + u64::from(reader.u8()));
                if model.len() < MAX_ENTRIES {
                    push_checked(&mut record, &mut model, &mut runs, height);
                }
            }
            // Any height at all, including lower ones and the extremes.
            2 => {
                height = match reader.u8() % 3 {
                    0 => 0,
                    1 => u64::MAX,
                    _ => reader.u64(),
                };
                if model.len() < MAX_ENTRIES {
                    push_checked(&mut record, &mut model, &mut runs, height);
                }
            }
            // Many entries, one per block or several per block.
            3 => {
                let wanted = usize::from(reader.u16()) % 9_000;
                let per_block = 1 + u64::from(reader.u8() % 4);
                for step in 0..wanted.min(MAX_ENTRIES - model.len()) {
                    if step as u64 % per_block == 0 {
                        height = height.wrapping_add(1);
                    }
                    push_checked(&mut record, &mut model, &mut runs, height);
                }
            }
            _ => check(&record, &model, &mut reader),
        }
    }
    for (index, expected) in model.iter().enumerate() {
        assert_eq!(record.get(index as u32), Some(*expected), "index {index}");
    }
    assert_eq!(record.get(model.len() as u32), None);
});
