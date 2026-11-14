import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import test from 'node:test';
import { RuesClient } from '../dist/index.js';

for (const [label, path, query, fragment] of [
  ['origin', '', '', ''],
  ['path prefix', '/rpc', '', ''],
  ['trailing slash', '/rpc/', '', ''],
  ['query', '/rpc', '?token=fixture%2Fvalue&network=local', ''],
  ['fragment', '/rpc', '', '#client-label'],
  ['query and fragment', '/rpc/', '?token=fixture%2Fvalue&network=local', '#client-label'],
  ['query ending in slash', '/rpc', '?token=fixture/', ''],
  ['encoded prefix', '/tenant%2Fname/rpc/', '?token=fixture', ''],
  ['browser relative', '/rpc', '?token=fixture', ''],
]) {
  test(`RUES routes retain ${label} URL components correctly`, async () => {
    const requests = [];
    const server = createServer(async (request, response) => {
      for await (const chunk of request) {}
      requests.push(request.url);
      response.end(Buffer.from([7]));
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    try {
      const origin = `http://127.0.0.1:${server.address().port}`;
      let client;
      if (label === 'browser relative') {
        const location = globalThis.location;
        try {
          globalThis.location = { href: `${origin}/page` };
          client = new RuesClient(`${path}${query}${fragment}`);
        } finally {
          if (location === undefined) delete globalThis.location;
          else globalThis.location = location;
        }
      } else {
        client = new RuesClient(`${origin}${path}${query}${fragment}`);
      }
      assert.deepEqual(await client.contractQuery(new Uint8Array(32), 'nonce', new Uint8Array()), new Uint8Array([7]));
      await client.propagateTx(new Uint8Array([1]));
      const prefix = path.replace(/\/+$/, '');
      assert.deepEqual(requests, [
        `${prefix}/on/contracts:${'00'.repeat(32)}/nonce${query}`,
        `${prefix}/on/transactions/propagate${query}`,
      ]);
    } finally {
      server.closeAllConnections();
      await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
    }
  });
}
