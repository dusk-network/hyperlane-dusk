import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import test from 'node:test';
import { DuskMailbox, DuskWarpDrc20, RuesClient } from '../dist/index.js';

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
    else if (request.url.endsWith('/total_supply') || request.url.endsWith('/delivered_at')) response.end(Buffer.alloc(8, 255));
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
    assert.equal(await mailbox.deliveredAt(address), maxU64);
    assert.equal(await mailbox.delivered(address), true);
    const backing = new Uint8Array([9, 1, 2, 9]);
    await client.propagateTx(backing.subarray(1, 3));
    assert.ok(requests.every(r => r.method === 'POST' && r.type === 'application/octet-stream'));
    assert.equal(requests[0].url, `/on/contracts:${Buffer.from(address).toString('hex')}/nonce`);
    assert.equal(requests[0].body.length, 0);
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
