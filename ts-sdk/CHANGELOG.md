# Changelog

## Unreleased

### Added

- `DuskMailbox.quoteDispatchForSender()` quotes the dispatch fee for an explicit sender as a read-only query, and the `rkyv_serialize_quote_dispatch_for_sender` binding encodes its arguments.
- `DuskIgp` for the InterchainGasPaymaster (`state_version` 4): `quotePayForGas()` quotes a top-up, `payForGasCall()` encodes `pay_for_gas` with the deposit a wallet signs it with, and `quoteGasPayment()`, `gasPaymentCount()` and `stateVersion()` read the rest. The `rkyv_serialize_pay_for_gas` and `rkyv_serialize_quote_pay_for_gas` bindings encode the arguments.

### Removed

- `DuskMailbox.deliveredAt()` and `DuskMailbox.processedCount()`. The Mailbox (`state_version` 5) no longer stores delivery heights or a list of deliveries; index its `process_id` events instead.

### Changed

- `DomainGasConfig` fields use `bigint` ([#11]).
- Data-driver u64 fields use decimal JSON strings, including nested records, events and decoded input amounts ([#11]).
- Data-driver quote and withdrawal encoders accept decimal strings alongside legacy unsigned JSON integers ([#11]).

- RUES requests have configurable deadlines and response-size limits, defaulting to 30 seconds, 4 MiB query bodies and 64 KiB error bodies ([#11]).
- Direct RUES requests accept an AbortSignal for cancellation ([#11]).

- `DuskWarpDrc20.totalSupply()` returns `bigint` to preserve every u64 value ([#11]).
- Decoded token-message JSON represents amounts as decimal strings ([#11]).
- WASM u64 encoder arguments require an in-range `bigint` ([#11]).
- WASM u8/u32 encoder arguments reject fractional, non-finite, and out-of-range numbers ([#11]).
- Address encoders require exactly 32 bytes ([#11]).

### Fixed

- `RuesClient` refuses a URL with a username or password, which `fetch` would reject with an error quoting the whole URL. Its errors never include the URL: a URL that doesn't parse gives `Invalid RUES URL`, and a network failure gives `<request> failed: network error (<code>)`.
- RUES routes preserve base-path prefixes and query parameters while omitting URL fragments ([#11]).

- Mailbox views reject malformed dispatched messages and nonce/domain mismatches ([#11]).

- Mailbox views derive the latest ID at their observed nonce, even during dispatch ([#11]).
- Invalid archives and truncated message accessors return recoverable errors without exhausting the WASM instance ([#11]).

- Contract queries use binary RUES method endpoints ([#11]).
- Token metadata queries decode the contract's rkyv String and u8 representations ([#11]).
- Typed-array HTTP bodies preserve the selected byte range ([#11]).
- SDK binding declarations match the generated WASM bigint and optional-return types ([#11]).

[#11]: https://github.com/dusk-network/hyperlane-dusk/pull/11
