import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, mkdirSync, statSync, symlinkSync, copyFileSync, writeFileSync, readFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { Store, parseConfig, outcome, HttpApi, binding } from './adapter.mjs';
import { wireSdk } from './worker.mjs';

const root = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const sdk = require('@larksuiteoapi/node-sdk');
const settings = changes => ({ appId: 'cli_0123456789abcdef', ownerOpenId: 'ou_owner', chatId: 'oc_selected',
  receiveEnabled: true, deliveryEnabled: true, appSecret: 'offline_fixture_only', ...changes });
const cfg = changes => parseConfig(settings(changes));
const event = () => ({ schema: '2.0', header: { event_type: 'im.message.receive_v1', event_id: 'ev_fixture' },
  event: { sender: { sender_type: 'user', sender_id: { open_id: 'ou_owner' } }, message: {
    chat_id: 'oc_selected', chat_type: 'p2p', message_id: 'om_fixture', message_type: 'text',
    content: JSON.stringify({ text: '$(touch /tmp/never-execute)' }) } } });
function fixture(fn) {
  return async () => {
    const dir = mkdtempSync(join(tmpdir(), 'feishu-node-offline-'));
    const config = cfg(), state = join(dir, 'state'), store = new Store(state, config);
    try { await fn({ store, config, state, dir }); } finally { try { store.close(); } catch {} rmSync(dir, { recursive: true, force: true }); }
  };
}
const fakeApi = (result = outcome('sent', 'confirmed', 'om_receipt')) => ({ calls: 0,
  async send(config, text, uuid) { this.calls++; assert.equal(uuid.length, 48); return result; } });

test('closed configuration; secret not serialized', () => {
  assert.ok(!JSON.stringify(cfg()).includes('offline_fixture_only'));
  for (const changes of [{ chatId: '' }, { ownerOpenId: '' }, { deliveryEnabled: 'yes' }, { appSecret: '' }, { command: 'exec' }]) assert.throws(() => cfg(changes));
});
test('owner AND selected chat only; text remains inert', fixture(({ store, config }) => {
  const [status, item] = store.receive(config, event()); assert.equal(status, 'accepted'); assert.match(item.text, /touch/);
  for (const mutate of [v => v.event.sender.sender_id.open_id = 'ou_other', v => v.event.message.chat_id = 'oc_other',
    v => v.event.sender.sender_type = 'app', v => v.event.message.message_type = 'image']) {
    const value = event(); mutate(value); assert.deepEqual(store.receive(config, value), ['filtered', null]);
  }
  assert.deepEqual(store.receive(cfg({ receiveEnabled: false }), event()), ['filtered', null]);
}));
test('malformed payload fails closed', fixture(({ store, config }) => {
  for (const value of [null, {}, [], { header: [] }]) assert.deepEqual(store.receive(config, value), ['filtered', null]);
  for (const content of ['null', '[]', '1', 'bad', '{"text":2}', JSON.stringify({ text: '中'.repeat(6000) })]) {
    const value = event(); value.event.message.content = content; assert.deepEqual(store.receive(config, value), ['filtered', null]);
  }
}));
test('durable inbound restart, event and message dedup, ACK clears text', fixture(({ store, config, state }) => {
  const [, item] = store.receive(config, event());
  for (const mutate of [v => v.header.event_id = 'ev_other', v => v.event.message.message_id = 'om_other']) {
    const value = event(); mutate(value); assert.equal(store.receive(config, value)[0], 'duplicate');
  }
  const reopened = new Store(state, config);
  try { assert.deepEqual(reopened.pending(), [item]); assert.ok(reopened.ack(item.ticket)); assert.deepEqual(store.pending(), []);
    assert.equal(store.receive(config, event())[0], 'duplicate'); assert.equal(store.db.prepare('SELECT payload FROM inbox').get().payload, null); }
  finally { reopened.close(); }
}));
test('unknown is durable across restart and never auto retried', fixture(async ({ store, config, state }) => {
  const api = fakeApi(); api.send = async () => { api.calls++; throw Error('timeout'); };
  const first = await store.send(config, 'request_01', 'hello', api); assert.equal(first.status, 'unknown');
  const reopened = new Store(state, config);
  try { assert.deepEqual(await reopened.send(config, 'request_01', 'hello', api), first);
    assert.equal((await reopened.send(config, 'request_01', 'changed', api)).status, 'blocked'); assert.equal(api.calls, 1); }
  finally { reopened.close(); }
}));
test('reservation is committed before transport; second connection cannot duplicate', fixture(async ({ store, config, state }) => {
  const other = new Store(state, config), api = fakeApi(); let release;
  api.send = async () => { api.calls++; await new Promise(resolve => { release = resolve; }); return outcome('sent', 'ok', 'om_ok'); };
  const first = store.send(config, 'request_02', 'hello', api);
  try { const second = await other.send(config, 'request_02', 'hello', api); assert.equal(second.status, 'unknown'); assert.equal(api.calls, 1);
    release(); assert.equal((await first).status, 'sent'); }
  finally { other.close(); }
}));
test('disabled / invalid blocked; missing message receipt stays unknown', fixture(async ({ store, config }) => {
  const api = fakeApi(); assert.equal((await store.send(cfg({ deliveryEnabled: false }), 'request_03', 'hi', api)).status, 'blocked');
  for (const [id, text] of [['bad', 'hi'], ['request_03', ''], ['request_03', 'x\0'], ['request_03', '中'.repeat(6000)]]) assert.equal((await store.send(config, id, text, api)).status, 'blocked');
  assert.equal(api.calls, 0); api.send = async () => outcome('sent', 'bad');
  assert.equal((await store.send(config, 'request_03', 'hello', api)).status, 'unknown');
}));
test('private binding paths; symlinks and public state rejected', fixture(({ store, state, dir, config }) => {
  assert.notEqual(binding(config), binding(cfg({ chatId: 'oc_other' })));
  assert.equal(statSync(state).mode & 0o777, 0o700);
  assert.equal(statSync(join(state, `${binding(config)}.node.sqlite3`)).mode & 0o777, 0o600);
  const link = join(dir, 'link'); symlinkSync(state, link); assert.throws(() => new Store(link, config));
  const publicDir = join(dir, 'public'); mkdirSync(publicDir, { mode: 0o755 }); assert.throws(() => new Store(publicDir, config));
}));
test('backlog bounded; existing duplicates still deduplicate', fixture(({ store, config }) => {
  store.receive(config, event()); store.transaction(() => {
    const insert = store.db.prepare('INSERT INTO inbox(ticket,message_hash,payload) VALUES(?,?,?)');
    for (let i = 0; i < 999; i++) insert.run(`t_${i}`, `m_${i}`, '{}');
  });
  assert.equal(store.receive(config, event())[0], 'duplicate');
  const fresh = event(); fresh.header.event_id = 'ev_new'; fresh.event.message.message_id = 'om_new';
  assert.throws(() => store.receive(config, fresh), /backlog/);
}));
test('HTTP endpoint, selected target and authentication preflight', async () => {
  const api = new HttpApi(), calls = [];
  const responses = [{ code: 0, tenant_access_token: 'offline_token', expire: 7200 }, { code: 0, data: { message_id: 'om_ok' } }];
  api.post = async (...args) => { calls.push(args); return responses.shift(); };
  assert.equal((await api.send(cfg(), 'hello', 'fixture_uuid')).status, 'sent');
  assert.equal(calls[1][0], 'im/v1/messages?receive_id_type=chat_id'); assert.equal(calls[1][1].receive_id, 'oc_selected');
  const authFail = new HttpApi(); authFail.post = async () => { throw Error('no network'); };
  assert.equal((await authFail.send(cfg(), 'hi', 'fixture_uuid')).status, 'blocked');
});
test('HTTP uncertain result unknown; API rejects failed; no transport retries', async () => {
  for (const result of [null, { code: 0, data: {} }, { code: 123 }]) {
    const api = new HttpApi(); let calls = 0;
    api.post = async () => { calls++; if (calls === 1) return { code: 0, tenant_access_token: 'fixture', expire: 7200 }; if (result === null) throw Error('timeout'); return result; };
    assert.equal((await api.send(cfg(), 'hi', 'fixture_uuid')).status, result?.code === 123 ? 'failed' : 'unknown'); assert.equal(calls, 2);
  }
});
test('real installed SDK exact version and dispatcher flattening stay scoped', fixture(async ({ store, config }) => {
  assert.equal(require('@larksuiteoapi/node-sdk/package.json').version, '1.76.0');
  const statuses = [], wire = { counts: { reconnects: 0 }, status: (...value) => statuses.push(value) };
  const { client, dispatcher } = wireSdk(sdk, config, wire, payload => store.receive(config, payload));
  try { await dispatcher.invoke(event(), { needCheck: false }); assert.equal(store.pending().length, 1);
    const other = event(); other.event.sender.sender_id.open_id = 'ou_other'; await dispatcher.invoke(other, { needCheck: false }); assert.equal(store.pending().length, 1); }
  finally { client.close({ force: true }); }
}));
test('real SDK protocol ACK follows successful SQLite commit, failure gets 500', fixture(async ({ store, config }) => {
  const wire = { counts: { reconnects: 0 }, status() {} };
  const { client, dispatcher } = wireSdk(sdk, config, wire, payload => store.receive(config, payload));
  client.eventDispatcher = dispatcher;
  const replies = []; client.sendMessage = frame => { replies.push(JSON.parse(Buffer.from(frame.payload).toString())); };
  const frame = value => ({ headers: [{ key: 'message_id', value: 'ws_fixture_' + value.header.event_id }, { key: 'sum', value: '1' },
    { key: 'seq', value: '0' }, { key: 'type', value: 'event' }, { key: 'trace_id', value: 'trace_fixture' }], payload: Buffer.from(JSON.stringify(value)) });
  try { await client.handleEventData(frame(event())); assert.equal(replies[0].code, 200); assert.equal(store.pending().length, 1);
    store.close(); const fresh = event(); fresh.header.event_id = 'ev_failure'; fresh.event.message.message_id = 'om_failure';
    await client.handleEventData(frame(fresh)); assert.equal(replies[1].code, 500); }
  finally { client.close({ force: true }); }
}));
test('lifecycle uses public SDK callbacks and no private method overrides', () => {
  let options; const states = [];
  class Client { constructor(value) { options = value; } }
  class Dispatcher { register(value) { this.handlers = value; return this; } }
  const fake = { WSClient: Client, EventDispatcher: Dispatcher, LoggerLevel: { fatal: 0 }, Domain: { Feishu: 0 } };
  const wire = { counts: { reconnects: 0 }, status: state => states.push(state) };
  wireSdk(fake, cfg(), wire, () => {}); options.onReady(); options.onReconnecting(); options.onReconnected(); options.onError();
  assert.deepEqual(states, ['connected', 'disconnected', 'reconnecting', 'connected', 'failed']); assert.equal(wire.counts.reconnects, 1);
});
function workerInput(lines, script = join(root, 'worker.mjs')) {
  const dir = mkdtempSync(join(tmpdir(), 'feishu-node-pipe-'));
  try { return spawnSync(process.execPath, ['--disable-warning=ExperimentalWarning', script, '--state-dir', dir],
    { input: lines.map(value => typeof value === 'string' ? value : JSON.stringify(value)).join('\n') + '\n', encoding: 'utf8', timeout: 5000 }); }
  finally { rmSync(dir, { recursive: true, force: true }); }
}
test('real Node worker private pipe: status, disabled delivery, unsupported command, stop', () => {
  const result = workerInput([{ op: 'init', config: settings({ receiveEnabled: false, deliveryEnabled: false }) }, { op: 'status' },
    { op: 'send', requestId: 'request_fixture', text: 'hello' }, { op: 'execute', command: 'bad' }, { op: 'stop' }]);
  assert.equal(result.status, 0, result.stderr); assert.equal(result.stderr, '');
  const values = result.stdout.trim().split('\n').map(line => JSON.parse(line)); assert.equal(values.at(-1).state, 'stopped');
  assert.ok(values.some(value => value.outcome?.status === 'blocked')); assert.ok(values.some(value => value.reason === 'unsupported_command'));
  assert.ok(!result.stdout.includes('offline_fixture_only'));
});
test('protocol oversized or malformed input fails safely', () => {
  for (const value of ['x'.repeat(65537), '[]', '{}']) {
    const result = workerInput([value]); assert.equal(result.status, 2); assert.ok(!result.stderr); assert.match(result.stdout, /stopped/);
  }
});
test('missing SDK runtime is explicit, never auto installs or falls back', () => {
  const dir = mkdtempSync(join(tmpdir(), 'feishu-node-no-sdk-'));
  try { for (const file of ['worker.mjs', 'adapter.mjs']) copyFileSync(join(root, file), join(dir, file));
    const result = workerInput([{ op: 'init', config: settings() }], join(dir, 'worker.mjs'));
    assert.equal(result.status, 2); assert.match(result.stdout, /sdk_unavailable/); assert.ok(!result.stdout.includes('offline_fixture_only')); }
  finally { rmSync(dir, { recursive: true, force: true }); }
});
test('real worker SIGTERM closes without network and emits stop states', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'feishu-node-signal-'));
  const child = spawn(process.execPath, ['--disable-warning=ExperimentalWarning', join(root, 'worker.mjs'), '--state-dir', dir]);
  let output = '', errors = '', sent = false;
  child.stdout.on('data', chunk => { output += chunk; if (!sent && output.includes('receive_disabled')) { sent = true; child.kill('SIGTERM'); } });
  child.stderr.on('data', chunk => errors += chunk);
  child.stdin.write(JSON.stringify({ op: 'init', config: settings({ receiveEnabled: false }) }) + '\n');
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  const code = await new Promise(resolve => child.on('exit', resolve)); clearTimeout(timer);
  assert.equal(code, 0); assert.match(output, /stopping/); assert.match(output, /stopped/); assert.equal(errors, '');
  rmSync(dir, { recursive: true, force: true });
});
