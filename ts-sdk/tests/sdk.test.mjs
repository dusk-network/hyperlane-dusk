import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import test from 'node:test';
import { DuskIgp, DuskMailbox, DuskWarpDrc20, RuesClient } from '../dist/index.js';

const wasm = createRequire(import.meta.url)('../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js');
const address = new Uint8Array(32).fill(0x11);
const maxU64 = (1n << 64n) - 1n;

test('token wire format and JSON preserve all 64 amount bits', () => {
  const encoded = wasm.encode_token_message(address, maxU64);
  assert.deepEqual(encoded.slice(0, 32), address);
  assert.deepEqual(encoded.slice(32, 56), new Uint8Array(24));
  assert.deepEqual(encoded.slice(56), new Uint8Array(8).fill(255));
  assert.equal(JSON.parse(wasm.decode_token_message(encoded)).amount, maxU64.toString());
  encoded[32] = 1;
  assert.equal(wasm.decode_token_message(encoded), undefined, 'out-of-range u256 must be rejected');
});

test('numeric encoders reject coercion, truncation, and unsigned wraparound', () => {
  for (const invalid of [-1n, 1n << 64n, (1n << 64n) + 1n, 1, 1.5, '1', null, undefined]) {
    assert.throws(() => wasm.encode_token_message(address, invalid), `token amount ${String(invalid)}`);
    assert.throws(() => wasm.rkyv_serialize_u64(invalid), `u64 ${String(invalid)}`);
  }
  for (const valid of [0n, 1n, maxU64]) {
    assert.equal(wasm.rkyv_deserialize_u64(wasm.rkyv_serialize_u64(valid)), valid);
    assert.equal(JSON.parse(wasm.decode_token_message(wasm.encode_token_message(address, valid))).amount, valid.toString());
  }
  for (const invalid of [-1, 0.5, 2 ** 32, NaN, Infinity, -Infinity, '1', null, undefined]) {
    assert.throws(() => wasm.rkyv_serialize_u32(invalid), `u32 ${String(invalid)}`);
    for (const field of [1, 2, 4]) {
      const args = [3, 0, 1, address, 2, address, new Uint8Array()];
      args[field] = invalid;
      assert.throws(() => wasm.encode_message(...args), `message field ${field}: ${String(invalid)}`);
    }
  }
  for (const invalid of [-1, 0.5, 256, NaN, Infinity, '3', null, undefined]) {
    assert.throws(() => wasm.encode_message(invalid, 0, 1, address, 2, address, new Uint8Array()));
  }
  for (const valid of [0, 1, 2 ** 32 - 1]) {
    assert.equal(wasm.rkyv_deserialize_u32(wasm.rkyv_serialize_u32(valid)), valid);
    const message = JSON.parse(wasm.decode_message(wasm.encode_message(3, valid, valid, address, valid, address, new Uint8Array())));
    assert.equal(message.version, 3);
    assert.equal(message.nonce, valid);
    assert.equal(message.origin, valid);
    assert.equal(message.destination, valid);
  }
  assert.equal(wasm.encode_message(255, 0, 1, address, 2, address, new Uint8Array())[0], 255);
});

test('address codecs reject truncation and padding', () => {
  for (const length of [0, 20, 31, 33, 64]) {
    const invalid = new Uint8Array(length);
    assert.throws(() => wasm.rkyv_serialize_bytes32(invalid));
    assert.throws(() => wasm.encode_token_message(invalid, 1n));
    assert.throws(() => wasm.encode_message(3, 0, 1, invalid, 2, address, new Uint8Array()));
    assert.throws(() => wasm.encode_message(3, 0, 1, address, 2, invalid, new Uint8Array()));
  }
  assert.deepEqual(wasm.rkyv_serialize_bytes32(address), address);
});

test('RUES uses binary method endpoints and keeps large query results exact', async () => {
  const requests = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = Buffer.concat(chunks);
    requests.push({url: request.url, method: request.method, type: request.headers['content-type'], body});
    response.setHeader('Content-Type', 'application/octet-stream');
    if (request.url.endsWith('/nonce')) response.end(Buffer.from([42, 0, 0, 0]));
    else if (request.url.endsWith('/decimals')) response.end(Buffer.from([18]));
    else if (request.url.endsWith('/total_supply') || request.url.endsWith('/quote_dispatch_for_sender')) response.end(Buffer.alloc(8, 255));
    else if (request.url.endsWith('/delivered')) response.end(Buffer.from([1]));
    else if (request.url === '/on/transactions/propagate') response.end();
    else { response.statusCode = 404; response.end('unexpected method'); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const client = new RuesClient(`http://127.0.0.1:${server.address().port}/`);
    const mailbox = new DuskMailbox(client, address, wasm);
    const token = new DuskWarpDrc20(client, address, wasm);
    assert.equal(await mailbox.nonce(), 42);
    assert.equal(await token.decimals(), 18);
    assert.equal(await token.totalSupply(), maxU64);
    assert.equal(await mailbox.quoteDispatchForSender(address, 7, address, new Uint8Array([5])), maxU64);
    assert.equal(await mailbox.delivered(address), true);
    const backing = new Uint8Array([9, 1, 2, 9]);
    await client.propagateTx(backing.subarray(1, 3));
    assert.ok(requests.every(r => r.method === 'POST' && r.type === 'application/octet-stream'));
    assert.equal(requests[0].url, `/on/contracts:${Buffer.from(address).toString('hex')}/nonce`);
    assert.equal(requests[0].body.length, 0);
    assert.deepEqual(requests[3].body, Buffer.from(wasm.rkyv_serialize_quote_dispatch_for_sender(address, 7, address, new Uint8Array([5]), new Uint8Array(), new Uint8Array(32))));
    assert.deepEqual(requests[4].body, Buffer.from(address));
    assert.deepEqual(requests[5].body, Buffer.from([1, 2]));
    const count = requests.length;
    await assert.rejects(client.contractQuery(new Uint8Array(31), 'nonce', new Uint8Array()));
    await assert.rejects(client.contractQuery(address, '../nonce', new Uint8Array()));
    assert.equal(requests.length, count, 'invalid requests must fail before network access');
  } finally {
    await new Promise((resolve, reject) => server.close(e => e ? reject(e) : resolve()));
  }
});

// The IGP's pay_for_gas(message_id, destination, gas_amount, deposit) as rkyv
// archives it, which is the order rustc lays the tuple out in, the same in
// the contract: the ID, the gas amount, the destination padded to 8 bytes,
// the deposit, little-endian. quote_pay_for_gas takes (destination,
// gas_amount) as the destination padded to 8 bytes and the gas amount. The
// data driver's tests pin the same bytes on the Rust side.
function payForGasBytes(id, destination, gasAmount, deposit) {
  const bytes = new Uint8Array(56);
  const view = new DataView(bytes.buffer);
  bytes.set(id, 0);
  view.setBigUint64(32, gasAmount, true);
  view.setUint32(40, destination, true);
  view.setBigUint64(48, deposit, true);
  return bytes;
}

test('pay_for_gas and its quote encode as the IGP takes them, and reject what it cannot', () => {
  assert.equal(Buffer.from(wasm.rkyv_serialize_pay_for_gas(address, 0x01020304, 0x0a0b0c0d0e0f1011n, 0x2122232425262728n)).toString('hex'),
    '11'.repeat(32) + '11100f0e0d0c0b0a' + '0403020100000000' + '2827262524232221');
  assert.equal(Buffer.from(wasm.rkyv_serialize_quote_pay_for_gas(0x01020304, 0x0a0b0c0d0e0f1011n)).toString('hex'),
    '0403020100000000' + '11100f0e0d0c0b0a');
  for (const [destination, gas, deposit] of [[0, 1n, 0n], [11155111, 100000n, 3750000000n], [2 ** 32 - 1, maxU64, maxU64]]) {
    assert.deepEqual(wasm.rkyv_serialize_pay_for_gas(address, destination, gas, deposit),
      payForGasBytes(address, destination, gas, deposit));
    const quote = new Uint8Array(16);
    new DataView(quote.buffer).setUint32(0, destination, true);
    new DataView(quote.buffer).setBigUint64(8, gas, true);
    assert.deepEqual(wasm.rkyv_serialize_quote_pay_for_gas(destination, gas), quote);
  }
  for (const length of [0, 20, 31, 33]) {
    assert.throws(() => wasm.rkyv_serialize_pay_for_gas(new Uint8Array(length), 1, 1n, 1n));
  }
  for (const invalid of [-1, 0.5, 2 ** 32, NaN, '1', null, undefined]) {
    assert.throws(() => wasm.rkyv_serialize_pay_for_gas(address, invalid, 1n, 1n), `destination ${String(invalid)}`);
    assert.throws(() => wasm.rkyv_serialize_quote_pay_for_gas(invalid, 1n), `destination ${String(invalid)}`);
  }
  for (const invalid of [-1n, 1n << 64n, 1, 1.5, '1', null, undefined]) {
    assert.throws(() => wasm.rkyv_serialize_pay_for_gas(address, 1, invalid, 1n), `gas ${String(invalid)}`);
    assert.throws(() => wasm.rkyv_serialize_pay_for_gas(address, 1, 1n, invalid), `deposit ${String(invalid)}`);
    assert.throws(() => wasm.rkyv_serialize_quote_pay_for_gas(1, invalid), `gas ${String(invalid)}`);
  }
});

test('DuskIgp quotes over RUES and builds the pay_for_gas call with its deposit', async () => {
  const requests = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    requests.push({url: request.url, body: Buffer.concat(chunks)});
    response.setHeader('Content-Type', 'application/octet-stream');
    if (request.url.endsWith('/quote_pay_for_gas') || request.url.endsWith('/quote_gas_payment')) response.end(Buffer.alloc(8, 255));
    else if (request.url.endsWith('/gas_payment_count')) response.end(Buffer.from([3, 0, 0, 0]));
    else if (request.url.endsWith('/state_version')) response.end(Buffer.from([4, 0, 0, 0]));
    else { response.statusCode = 404; response.end('unexpected method'); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const igpId = new Uint8Array(32).fill(0x0f);
    const igp = new DuskIgp(new RuesClient(`http://127.0.0.1:${server.address().port}/`), igpId, wasm);
    assert.equal(await igp.quotePayForGas(11155111, 100000n), maxU64);
    assert.equal(await igp.quoteGasPayment(11155111, 50000n), maxU64);
    assert.equal(await igp.gasPaymentCount(), 3);
    assert.equal(await igp.stateVersion(), 4);
    const base = `/on/contracts:${Buffer.from(igpId).toString('hex')}/`;
    assert.deepEqual(requests.map(r => r.url),
      ['quote_pay_for_gas', 'quote_gas_payment', 'gas_payment_count', 'state_version'].map(m => base + m));
    assert.deepEqual(requests[0].body, Buffer.from(wasm.rkyv_serialize_quote_pay_for_gas(11155111, 100000n)));
    assert.deepEqual(requests[1].body, Buffer.from(wasm.rkyv_serialize_quote_pay_for_gas(11155111, 50000n)));

    const call = igp.payForGasCall(address, 11155111, 100000n, 3750000000n);
    assert.deepEqual(call.contractId, igpId);
    assert.equal(call.fnName, 'pay_for_gas');
    assert.equal(call.deposit, 3750000000n);
    assert.deepEqual(call.args, payForGasBytes(address, 11155111, 100000n, 3750000000n));
    assert.equal(requests.length, 4, 'building the call sends nothing');
    assert.throws(() => igp.payForGasCall(new Uint8Array(31), 11155111, 1n, 1n));
  } finally {
    await new Promise((resolve, reject) => server.close(e => e ? reject(e) : resolve()));
  }
});
