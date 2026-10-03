import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';

const wasm = createRequire(import.meta.url)('../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js');
const address = new Uint8Array(32).fill(0x11);

test('rejected archives and truncated message accessors remain recoverable', () => {
  const decoders = ['u8', 'u32', 'u64', 'bool', 'string', 'bytes32', 'bytes'];
  for (const decoder of decoders) {
    assert.throws(() => wasm[`rkyv_deserialize_${decoder}`](new Uint8Array()), error => !(error instanceof WebAssembly.RuntimeError));
  }
  const valid = wasm.encode_message(3, 42, 4242, address, 1234, address, new Uint8Array());
  for (let length = 0; length < 77; length++) {
    assert.throws(() => wasm.message_nonce(valid.subarray(0, length)), error => !(error instanceof WebAssembly.RuntimeError));
    assert.throws(() => wasm.message_destination(valid.subarray(0, length)), error => !(error instanceof WebAssembly.RuntimeError));
  }
  assert.equal(wasm.message_nonce(valid), 42);
  assert.equal(wasm.message_destination(valid), 1234);
  assert.throws(() => wasm.rkyv_deserialize_string(Uint8Array.of(255, 0, 0, 0, 0, 0, 0, 1)));
  assert.throws(() => wasm.rkyv_deserialize_bytes(new Uint8Array(8).fill(255)));
  // The old aborting panic path corrupted the instance after 5,956 iterations.
  // Keep checking real subsequent operations on the same compiled instance.
  for (let iteration = 0; iteration < 20_000; iteration++) {
    assert.throws(() => wasm.rkyv_deserialize_bool(Uint8Array.of(2)), error => !(error instanceof WebAssembly.RuntimeError));
    assert.equal(wasm.rkyv_deserialize_u32(wasm.rkyv_serialize_u32(42)), 42);
  }
});
