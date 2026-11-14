import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import test from 'node:test';
import { DuskMailbox, RuesClient } from '../dist/index.js';

const wasm = createRequire(import.meta.url)('../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js');
const address = new Uint8Array(32).fill(0x11);
function archiveBytes(bytes) {
  const root = (bytes.length + 3) & ~3;
  const archive = Buffer.alloc(root + 8);
  archive.set(bytes);
  archive.writeInt32LE(-root, root);
  archive.writeUInt32LE(bytes.length, root + 4);
  return archive;
}

for (const initialNonce of [0, 1]) {
  test(`mailbox view keeps the nonce/ID pair coherent across dispatch at nonce ${initialNonce}`, async () => {
    const messages = [0, 1].map(nonce => wasm.encode_message(3, nonce, 4242, address, 42, address, Uint8Array.of(nonce)));
    let nonceObserved;
    const nonceRead = new Promise(resolve => { nonceObserved = resolve; });
    const server = createServer(async (request, response) => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const args = Buffer.concat(chunks);
      const method = request.url.split('/').at(-1);
      if (method === 'local_domain') response.end(wasm.rkyv_serialize_u32(4242));
      else if (method === 'nonce') {
        response.end(wasm.rkyv_serialize_u32(initialNonce));
        // One ordinary dispatch commits after this response's state was read.
        nonceObserved();
      } else if (method === 'latest_dispatched_id') {
        await nonceRead;
        response.end(wasm.message_id(messages[initialNonce]));
      } else if (method === 'dispatched_message') {
        assert.equal(args.readUInt32LE(), initialNonce - 1);
        response.end(archiveBytes(messages[initialNonce - 1]));
      } else response.end(address);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    try {
      const mailbox = new DuskMailbox(new RuesClient(`http://127.0.0.1:${server.address().port}`), address, wasm);
      const view = await mailbox.getState();
      assert.equal(view.nonce, initialNonce);
      assert.equal(view.latestDispatchedId, initialNonce === 0 ? '00'.repeat(32) : Buffer.from(wasm.message_id(messages[initialNonce - 1])).toString('hex'));
    } finally {
      await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    }
  });
}
