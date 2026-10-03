import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import test from 'node:test';
import { RuesClient } from '../dist/index.js';

const contract = new Uint8Array(32);
const query = (client, options) => client.contractQuery(contract, 'nonce', new Uint8Array(), options);
async function withServer(handler, run) {
  const server = createServer(async (request, response) => {
    for await (const _chunk of request) {};
    handler(request, response);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try { await run(`http://127.0.0.1:${server.address().port}`); }
  finally {
    server.closeAllConnections();
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
}
async function bounded(promise) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('fixture deadline: SDK did not settle')), 1000);
    })]);
  } finally { clearTimeout(timer); }
}

for (const chunked of [false, true]) {
  for (const operation of ['query', 'query error', 'propagation error']) {
    test(`${operation} bounds ${chunked ? 'chunked' : 'declared-length'} responses and recovers`, async () => {
      let size = 64;
      await withServer((_request, response) => {
        response.writeHead(operation === 'query' ? 200 : 503,
          chunked ? {} : { 'Content-Length': String(size) });
        response.write(Buffer.alloc(size - 1, 65));
        response.end(Buffer.from([65]));
      }, async url => {
        const client = new RuesClient(url, { maxResponseBytes: 64, maxErrorResponseBytes: 64 });
        const call = () => operation.startsWith('propagation')
          ? client.propagateTx(new Uint8Array()) : query(client);
        if (operation === 'query') assert.equal((await call()).length, 64);
        else await assert.rejects(call(), /failed \(503\)/);
        size = 65;
        await assert.rejects(call(), /exceeds.*64.*bytes/i);
        size = 3;
        if (operation === 'query') assert.equal((await call()).length, 3);
        else await assert.rejects(call(), /failed \(503\): AAA/);
      });
    });
  }
}

test('default query response limit is four MiB', async () => {
  await withServer((_request, response) => response.end(Buffer.alloc(4 * 1024 * 1024 + 1)),
    async url => assert.rejects(query(new RuesClient(url)), /exceeds.*4194304.*bytes/i));
});

for (const operation of ['query headers', 'query body', 'propagation headers', 'propagation error body']) {
  test(`${operation} respects the configured deadline and recovers`, async () => {
    let stall = true;
    await withServer((_request, response) => {
      if (!stall) { response.end(Buffer.from([1, 2, 3])); return; }
      if (operation.includes('body')) {
        response.writeHead(operation.startsWith('propagation') ? 503 : 200);
        response.write(Buffer.from([1]));
      }
    }, async url => {
      const client = new RuesClient(url, { timeoutMs: 100 });
      const call = () => operation.startsWith('propagation')
        ? client.propagateTx(new Uint8Array()) : query(client);
      await assert.rejects(bounded(call()), error => error.name === 'TimeoutError');
      stall = false;
      await bounded(call());
    });
  });
}

test('caller cancellation aborts a pending body and does not poison later calls', async () => {
  let stall = true;
  let start;
  const started = new Promise(resolve => { start = resolve; });
  await withServer((_request, response) => {
    response.write(Buffer.from([1]));
    start();
    if (!stall) response.end();
  }, async url => {
    const client = new RuesClient(url);
    const controller = new AbortController();
    const pending = query(client, { signal: controller.signal });
    await started;
    controller.abort();
    await assert.rejects(bounded(pending), error => error.name === 'AbortError');
    stall = false;
    assert.equal((await query(client)).length, 1);
    await assert.rejects(query(client, { signal: controller.signal }), error => error.name === 'AbortError');
  });
});

test('successful propagation acknowledges headers and cancels an unused body', async () => {
  await withServer((_request, response) => { response.writeHead(202); response.write('accepted'); },
    async url => new RuesClient(url, { timeoutMs: 100 }).propagateTx(new Uint8Array()));
});

test('invalid limits fail before any request', () => {
  for (const value of [0, -1, NaN, Infinity, 1.5]) {
    for (const key of ['timeoutMs', 'maxResponseBytes', 'maxErrorResponseBytes']) {
      assert.throws(() => new RuesClient('http://127.0.0.1:1', { [key]: value }), /positive.*integer/i);
    }
  }
  assert.throws(() => new RuesClient('http://127.0.0.1:1', { timeoutMs: 2147483648 }), /timeoutMs/i);
});
