import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import test from 'node:test';
import { inspect } from 'node:util';
import { RuesClient } from '../dist/index.js';

const SECRETS = ['rpc-user', 'rpc-password', 'rpc-path-sentinel', 'query-sentinel'];
const query = client => client.contractQuery(new Uint8Array(32), 'nonce', new Uint8Array());

// The message, and everything a logger would print: properties and causes.
function assertNamesNoUrl(error, expected) {
  assert.match(error.message, expected);
  const printed = `${error.message}\n${inspect(error, { depth: 8, showHidden: true })}`;
  for (const secret of SECRETS) {
    assert.ok(!printed.includes(secret), `error retained ${secret}: ${printed}`);
  }
  assert.ok(!/https?:\/\//.test(printed), `error retained a URL: ${printed}`);
  return true;
}

async function withServer(handler, run) {
  const server = createServer(handler);
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try { await run(server.address().port); }
  finally {
    server.closeAllConnections();
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
}

test('a URL with a username or password is refused without quoting it', () => {
  for (const url of [
    'http://rpc-user:rpc-password@127.0.0.1:8080/rpc-path-sentinel?key=query-sentinel',
    'http://rpc-user@127.0.0.1:8080/rpc-path-sentinel?key=query-sentinel',
    'https://:rpc-password@node.invalid/rpc-path-sentinel',
  ]) {
    assert.throws(() => new RuesClient(url),
      error => assertNamesNoUrl(error, /must not contain a username or password/));
  }
});

test('a URL that does not parse is refused without quoting it', () => {
  assert.throws(() => new RuesClient('http://rpc-user:rpc-password@[::1/rpc-path-sentinel?key=query-sentinel'),
    error => assertNamesNoUrl(error, /^Invalid RUES URL$/));
});

test('a refused connection reports a code, not the URL', async () => {
  let port;
  await withServer(() => {}, async freePort => { port = freePort; });
  const client = new RuesClient(`http://127.0.0.1:${port}/rpc-path-sentinel?key=query-sentinel`);
  await assert.rejects(query(client),
    error => assertNamesNoUrl(error, /^RUES query failed: network error \(ECONNREFUSED\)$/));
  await assert.rejects(client.propagateTx(new Uint8Array([1])),
    error => assertNamesNoUrl(error, /^TX propagation failed: network error \(ECONNREFUSED\)$/));
});

test('a connection dropped mid-body reports no part of the URL', async () => {
  await withServer((request, response) => {
    request.resume();
    response.writeHead(200, { 'Content-Length': '64' });
    response.write(Buffer.alloc(8), () => response.socket.destroy());
  }, async port => {
    const client = new RuesClient(`http://127.0.0.1:${port}/rpc-path-sentinel?key=query-sentinel`);
    await assert.rejects(query(client), error => assertNamesNoUrl(error, /^RUES query failed: network error/));
  });
});

test('HTTP errors keep the status and the node\'s message', async () => {
  await withServer((request, response) => {
    request.resume();
    response.writeHead(500);
    response.end('contract panicked');
  }, async port => {
    const client = new RuesClient(`http://127.0.0.1:${port}/rpc-path-sentinel?key=query-sentinel`);
    await assert.rejects(query(client),
      error => assertNamesNoUrl(error, /^RUES query failed \(500\): contract panicked$/));
  });
});
