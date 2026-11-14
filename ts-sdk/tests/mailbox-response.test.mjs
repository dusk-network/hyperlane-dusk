import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import test from 'node:test';
import { DuskMailbox, RuesClient } from '../dist/index.js';

const wasm = createRequire(import.meta.url)('../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js');
const address = new Uint8Array(32).fill(0x11);
const message = (version = 3, nonce = 0, origin = 4242) =>
  wasm.encode_message(version, nonce, origin, address, 42, address, new Uint8Array());
const validMessage = message();
const expectedId = Buffer.from(wasm.message_id(validMessage)).toString('hex');
function archiveBytes(bytes) {
  const root = (bytes.length + 3) & ~3;
  const archive = Buffer.alloc(root + 8);
  archive.set(bytes);
  archive.writeInt32LE(-root, root);
  archive.writeUInt32LE(bytes.length, root + 4);
  return archive;
}

for (const [label, invalidMessage] of [
  ['empty', new Uint8Array()],
  ['truncated', validMessage.slice(0, 76)],
  ['unsupported version', message(4)],
  ['wrong nonce', message(3, 1)],
  ['wrong origin', message(3, 0, 4243)],
]) {
  test(`mailbox state rejects a ${label} message response and recovers`, async () => {
    let servedMessage = validMessage;
    const server = createServer(async (request, response) => {
      for await (const _chunk of request) {};
      const method = request.url.split('/').at(-1);
      if (method === 'local_domain') response.end(wasm.rkyv_serialize_u32(4242));
      else if (method === 'nonce') response.end(wasm.rkyv_serialize_u32(1));
      else if (method === 'dispatched_message') response.end(archiveBytes(servedMessage));
      else response.end(address);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    try {
      const mailbox = new DuskMailbox(new RuesClient(`http://127.0.0.1:${server.address().port}`), address, wasm);
      assert.equal((await mailbox.getState()).latestDispatchedId, expectedId);
      servedMessage = invalidMessage;
      await assert.rejects(mailbox.getState(), /dispatched message/i);
      servedMessage = validMessage;
      assert.equal((await mailbox.getState()).latestDispatchedId, expectedId);
    } finally {
      server.closeAllConnections();
      await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    }
  });
}
