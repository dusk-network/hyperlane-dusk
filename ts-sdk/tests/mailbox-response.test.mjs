import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import test from 'node:test';
import { DuskMailbox, RuesClient } from '../dist/index.js';

const wasm = createRequire(import.meta.url)('../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js');
const address = new Uint8Array(32).fill(0x11);
const validId = wasm.message_id(
  wasm.encode_message(3, 0, 4242, address, 42, address, new Uint8Array()),
);
const expectedId = Buffer.from(validId).toString('hex');

for (const [label, invalidId] of [
  ['empty', new Uint8Array()],
  ['truncated', validId.slice(0, 31)],
]) {
  test(`mailbox state rejects a ${label} message ID response and recovers`, async () => {
    let servedId = validId;
    const server = createServer(async (request, response) => {
      for await (const _chunk of request) {};
      const method = request.url.split('/').at(-1);
      if (method === 'local_domain') response.end(wasm.rkyv_serialize_u32(4242));
      else if (method === 'nonce') response.end(wasm.rkyv_serialize_u32(1));
      else if (method === 'dispatched_id') response.end(servedId);
      else response.end(address);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    try {
      const mailbox = new DuskMailbox(new RuesClient(`http://127.0.0.1:${server.address().port}`), address, wasm);
      assert.equal((await mailbox.getState()).latestDispatchedId, expectedId);
      servedId = invalidId;
      await assert.rejects(mailbox.getState(), /H256/);
      servedId = validId;
      assert.equal((await mailbox.getState()).latestDispatchedId, expectedId);
    } finally {
      server.closeAllConnections();
      await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    }
  });
}
