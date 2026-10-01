# Changelog

## Unreleased

### Changed

- `DuskWarpDrc20.totalSupply()` returns `bigint` to preserve every u64 value ([#11]).
- `DuskMailbox.deliveredAt()` returns `bigint` to preserve every block-height bit ([#11]).
- Decoded token-message JSON represents amounts as decimal strings ([#11]).
- WASM u64 encoder arguments require an in-range `bigint` ([#11]).
- WASM u8/u32 encoder arguments reject fractional, non-finite, and out-of-range numbers ([#11]).
- Address encoders require exactly 32 bytes ([#11]).

### Fixed

- Contract queries use binary RUES method endpoints ([#11]).
- Token metadata queries decode the contract's rkyv String and u8 representations ([#11]).
- Typed-array HTTP bodies preserve the selected byte range ([#11]).
- SDK binding declarations match the generated WASM bigint and optional-return types ([#11]).

[#11]: https://github.com/dusk-network/hyperlane-dusk/pull/11
