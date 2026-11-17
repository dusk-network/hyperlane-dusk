# Fuzz targets for the shared types

Coverage-guided fuzzing ([cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz))
of the `types` crate as the agents build it (software keccak256, no Rusk
dependency), each target checking properties against an independent
reference. The directory is its own workspace with its own lockfile.

| Target | Checks |
|---|---|
| `message_codec` | `message::decode` of any bytes loses nothing and accepts exactly ≥77 bytes of version 3; accessors; `encode` of any fields is `Message.sol`'s packed layout; IDs against the `sha3` crate |
| `keccak` | the software keccak256 against `sha3`, across the 136-byte rate |
| `token_message` | `token_message::decode` reads the full uint256 and refuses, never truncates, amounts of 2^64 and above; `encode` is `abi.encodePacked(bytes32, uint256, bytes)`; hook metadata gas limit |
| `digests_signatures` | checkpoint and announcement digests against `CheckpointLib`/`ValidatorAnnounce` computed with `sha3`; `signature::is_canonical` against `k256`; an honest `k256` signature over a checkpoint is canonical and recovers to its signer, its high-s twin is refused |
| `ism_metadata` | the MessageIdMultisig ISM's metadata rules (restated from `contracts/ism-multisig`, which only builds for wasm) against Solidity's: never more permissive, and only stricter about trailing bytes |
| `chunked_log`, `block_heights` | `storage::ChunkedLog` and `BlockHeights` against a `Vec`, and every push allocates exactly what `push_allocations` predicted (an allocator records it) |
| `merkle` | `merkle::IncrementalMerkle` against a naive depth-32 tree: roots, checkpoints and branch slots, steered to each side of powers of two |
| `rkyv_types` | rkyv validation of the event and record types the agents decode: any bytes are refused or round-trip |

```bash
cd fuzz
cargo run --example seeds          # optional: valid seed inputs into corpus/
cargo fuzz run message_codec -- -max_total_time=600
```

The toolchain is the repository's pinned nightly. `corpus/`, `artifacts/`
and `target/` are ignored.
