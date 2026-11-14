// Read-only SDK checks against the completed three-route agent E2E deployment.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { DuskMailbox, DuskWarpDrc20, DuskWarpNative, DuskWarpCollateral, RuesClient } from '../dist/index.js';

assert.ok(process.env.DUSK_RUES_URL, 'DUSK_RUES_URL is required');
assert.ok(process.env.BRIDGE_STATE_FILE, 'BRIDGE_STATE_FILE is required');
const state = JSON.parse(await readFile(process.env.BRIDGE_STATE_FILE, 'utf8'));
const wasm = createRequire(import.meta.url)('../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js');
const bytes = hex => Buffer.from(hex.replace(/^0x/, ''), 'hex');
const client = new RuesClient(process.env.DUSK_RUES_URL);
const mailbox = new DuskMailbox(client, bytes(state.dusk.mailbox), wasm);
const token = new DuskWarpDrc20(client, bytes(state.dusk.warp_drc20), wasm);
const native = new DuskWarpNative(client, bytes(state.dusk.warp_native), wasm);
const collateral = new DuskWarpCollateral(client, bytes(state.dusk.warp_drc20_collateral), wasm);
const view = await mailbox.getState();
assert.equal(view.localDomain, state.dusk_domain);
assert.equal(view.defaultIsm, state.dusk.default_ism);
assert.equal(view.defaultHook, state.dusk.igp);
assert.equal(view.requiredHook, state.dusk.aggregation_hook);
assert.ok(view.nonce >= 3, 'all three routes must have dispatched');
const lastMessage = await mailbox.dispatchedMessage(view.nonce - 1);
assert.equal(Buffer.from(wasm.message_id(lastMessage)).toString('hex'), view.latestDispatchedId);
assert.equal(JSON.parse(wasm.decode_message(lastMessage)).nonce, view.nonce - 1);
assert.ok(await mailbox.processedCount() >= 3, 'all three routes must have received');
const processedId = await client.contractQuery(bytes(state.dusk.mailbox), 'processed_at_index', wasm.rkyv_serialize_u32(0));
assert.equal(await mailbox.delivered(processedId), true);
assert.ok(await mailbox.deliveredAt(processedId) > 0n);
assert.equal(await token.symbol(), state.token_symbol);
assert.equal(await token.name(), process.env.TOKEN_NAME);
assert.equal(await token.decimals(), Number(process.env.TOKEN_DECIMALS));
assert.equal(await token.totalSupply(), BigInt(process.env.EXPECTED_DUSK_SUPPLY));
for (const [route, remote] of [[token, state.evm.token], [native, state.evm.native_token], [collateral, state.evm.collateral_token]]) {
  assert.equal(await route.mailbox(), state.dusk.mailbox);
  assert.equal(await route.hook(), '00'.repeat(32));
  assert.equal(await route.interchainSecurityModule(), '00'.repeat(32));
  assert.equal(await route.enrolledRouter(state.evm_domain), remote.replace(/^0x/, '').toLowerCase().padStart(64, '0'));
}
assert.equal(await native.isRegistered(bytes(state.account_h256)), true);
assert.equal(await native.isRegistered(new Uint8Array(32)), false);
assert.equal(await collateral.wrappedToken(), state.dusk.warp_drc20);
console.log(`SDK live checks passed: ${state.dusk_default_ism}, nonce=${view.nonce}, exact supply=${await token.totalSupply()}`);
