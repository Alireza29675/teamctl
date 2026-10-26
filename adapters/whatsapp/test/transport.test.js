import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtemp, rm, stat, symlink, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createWhatsAppTransport, normalizeIncoming } from '../src/transport.js';
import { until } from '../../sdk/test/helpers.js';

class FakeClient extends EventEmitter {
  calls = [];
  destroyed = 0;
  quote = null;
  info = { wid: { _serialized: '123@c.us' } };
  aliases = [{ pn: '123@c.us', lid: '456@lid' }];
  aliasRequests = [];
  async initialize() { this.emit('ready'); }
  async destroy() { this.destroyed += 1; }
  async sendMessage(...args) { this.calls.push(args); return { id: { _serialized: 'wa:sent:1' } }; }
  async getMessageById() { return this.quote; }
  async getContactLidAndPhone(ids) { this.aliasRequests.push(ids); return this.aliases; }
}

const source = (extra = {}) => ({ from: '123@c.us', body: 'سلام é 📎', type: 'chat',
  id: { _serialized: 'wa:source:1' }, ...extra });
const outbound = (extra = {}) => ({ peer_id: '123@c.us',
  message: { text: 'hello', kind: 'text', payload: null, reply_to_external_id: null }, ...extra });
const selfNote = (extra = {}) => source({ fromMe: true, to: '123@c.us', deviceType: 'android', ...extra });

async function fixture(t, options = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-wa-'));
  const clients = [];
  const events = [];
  const messages = [];
  const failures = [];
  let clientOptions;
  let profile;
  const context = {
    ready: async () => events.push('ready'), connection: async (state) => events.push(state),
    receive: async (event) => messages.push(event), pairing: async () => events.push('pairing'),
    fail: async (code) => failures.push(code),
  };
  const transport = await createWhatsAppTransport({ instance_id: 'whatsapp', state_dir: directory,
    mode: options.mode ?? 'run', config: options.config ?? {} }, context,
  { ...options, clientFactory: async (providedOptions, authPath) => {
    clientOptions = providedOptions;
    profile = authPath;
    const client = new FakeClient();
    options.configureClient?.(client);
    clients.push(client);
    return client;
  } });
  t.after(async () => { await transport.close(); await rm(directory, { recursive: true, force: true }); });
  await transport.start();
  await until(() => events.includes('ready'));
  return { transport, directory, clients, events, messages, failures,
    options: () => clientOptions, profile: () => profile };
}

test('direct and group identities remain opaque and preserve the participant', async () => {
  assert.deepEqual(await normalizeIncoming(source()), { external_id: 'wa:source:1', peer_id: '123@c.us',
    sender_id: '123@c.us', text: 'سلام é 📎', kind: 'text', payload: null, reply_to_external_id: null });
  const group = await normalizeIncoming(source({ from: '123-456@g.us', author: '987@lid' }));
  assert.equal(group.peer_id, '123-456@g.us');
  assert.equal(group.sender_id, '987@lid');
  assert.equal(await normalizeIncoming(source({ from: '123-456@g.us' })), null);
  assert.equal(await normalizeIncoming(source({ from: '123-456@g.us', author: 'other@g.us' })), null);
});

test('own, status, broadcast, channel and unsupported media events are dropped', async () => {
  for (const extra of [{ fromMe: true }, { id: { fromMe: true, _serialized: 'own' } },
    { isStatus: true }, { broadcast: true }, { from: 'status@broadcast' }, { from: '123@newsletter' },
    { type: 'image', hasMedia: true }, { body: '' }]) {
    assert.equal(await normalizeIncoming(source(extra)), null);
  }
});

test('incoming quoted text preserves reference, even if quote lookup later fails', async () => {
  const quoted = await normalizeIncoming(source({ hasQuotedMsg: true,
    getQuotedMessage: async () => ({ id: { _serialized: 'wa:quote' } }) }));
  assert.equal(quoted.reply_to_external_id, 'wa:quote');
  const expired = await normalizeIncoming(source({ hasQuotedMsg: true,
    getQuotedMessage: async () => { throw new Error('expired'); } }));
  assert.equal(expired.reply_to_external_id, null);
});

test('phone self-notes require opt-in and verified own sender and destination IDs', async (t) => {
  const ordinary = await fixture(t);
  ordinary.clients[0].emit('message_create', selfNote());
  await new Promise(setImmediate);
  assert.equal(ordinary.messages.length, 0);
  assert.equal(ordinary.clients[0].aliasRequests.length, 0);

  const enabled = await fixture(t, { config: { self_chat: true } });
  enabled.clients[0].emit('message_create', selfNote({ hasQuotedMsg: true,
    getQuotedMessage: async () => ({ id: { _serialized: 'wa:quote' } }) }));
  enabled.clients[0].emit('message_create', selfNote({ from: '456@lid', to: '456@lid', deviceType: 'ios',
    id: { _serialized: 'wa:ios:1', fromMe: true } }));
  await until(() => enabled.messages.length === 2);
  const android = enabled.messages.find((message) => message.external_id === 'wa:source:1');
  const ios = enabled.messages.find((message) => message.external_id === 'wa:ios:1');
  assert.equal(android.peer_id, '123@c.us');
  assert.equal(android.sender_id, '123@c.us');
  assert.equal(android.reply_to_external_id, 'wa:quote');
  assert.equal(ios.peer_id, '456@lid');
  assert.equal(ios.sender_id, '456@lid');
  assert.deepEqual(enabled.clients[0].aliasRequests, [['123@c.us']]);
});

test('self-chat drops web or unknown origins and own messages to another peer', async () => {
  const options = { selfChat: true, selfIds: new Set(['123@c.us', '456@lid']) };
  for (const extra of [{ deviceType: 'web' }, { deviceType: undefined }, { deviceType: 'desktop' },
    { to: 'other@c.us' }, { from: 'other@lid' }, { to: '123-456@g.us' }, { to: undefined },
    { from: 'status@broadcast', isStatus: true }, { hasMedia: true }, { broadcast: true }]) {
    assert.equal(await normalizeIncoming(selfNote(extra), options), null);
  }
  const aliases = await normalizeIncoming(selfNote({ from: '456@lid' }), options);
  assert.equal(aliases.peer_id, '123@c.us');
  assert.equal(aliases.sender_id, '456@lid');
});

test('unavailable or unrelated alias lookups never guess own account identities', async (t) => {
  for (const configureClient of [
    (client) => { client.aliases = [{ pn: '999@c.us', lid: '456@lid' }]; },
    (client) => { client.getContactLidAndPhone = async () => { throw new Error('unavailable'); }; },
  ]) {
    const f = await fixture(t, { config: { self_chat: true }, configureClient });
    f.clients[0].emit('message_create', selfNote({ to: '456@lid' }));
    f.clients[0].emit('message_create', selfNote());
    await until(() => f.messages.length === 1);
    assert.equal(f.messages[0].peer_id, '123@c.us');
  }
  const missing = await fixture(t, { config: { self_chat: true },
    configureClient: (client) => { client.info = {}; } });
  missing.clients[0].emit('message_create', selfNote());
  await new Promise(setImmediate);
  assert.equal(missing.messages.length, 0);
});

test('adapter echoes are filtered both before send resolves and by returned source ID', async (t) => {
  const f = await fixture(t, { config: { self_chat: true } });
  f.clients[0].sendMessage = async () => {
    f.clients[0].emit('message_create', selfNote({ body: 'hello', from: '456@lid', to: '456@lid' }));
    await new Promise(setImmediate);
    return { id: { _serialized: 'wa:sent:1' } };
  };
  await f.transport.send(outbound());
  f.clients[0].emit('message_create', selfNote({ body: 'late echo', id: { _serialized: 'wa:sent:1' } }));
  f.clients[0].emit('message_create', selfNote({ deviceType: 'web' }));
  await new Promise(setImmediate);
  assert.equal(f.messages.length, 0);
});

test('self-chat preserves ordinary contacts and groups without duplicate message_create ingress', async (t) => {
  const f = await fixture(t, { config: { self_chat: true } });
  for (const message of [source(), source({ from: '123-456@g.us', author: '987@lid',
    id: { _serialized: 'wa:group:1' } })]) {
    f.clients[0].emit('message_create', message);
    f.clients[0].emit('message', message);
  }
  await until(() => f.messages.length === 2);
  assert.equal(f.messages[0].peer_id, '123@c.us');
  assert.equal(f.messages[1].peer_id, '123-456@g.us');
  assert.equal(f.messages[1].sender_id, '987@lid');
});

test('browser settings keep sandbox and conflict policy, and profile/cache stay private', async (t) => {
  const f = await fixture(t, { config: { browser_executable_path: '/Applications/Browser' } });
  assert.equal(f.options().takeoverOnConflict, false);
  assert.equal(f.options().puppeteer.dumpio, false);
  assert.deepEqual(f.options().puppeteer.args, []);
  assert.equal(f.options().puppeteer.executablePath, '/Applications/Browser');
  assert.equal(f.options().puppeteer.signal.aborted, false);
  assert.equal((await stat(f.profile())).mode & 0o777, 0o700);
  assert.equal((await stat(f.options().webVersionCache.path)).mode & 0o777, 0o700);
  await f.transport.close();
  assert.equal(f.options().puppeteer.signal.aborted, true);
  assert.equal(f.clients[0].destroyed, 1);
});

test('text sends use provider acceptance without previews/read receipt side effects', async (t) => {
  const f = await fixture(t);
  assert.equal(await f.transport.send(outbound()), 'wa:sent:1');
  assert.deepEqual(f.clients[0].calls[0], ['123@c.us', 'hello', { linkPreview: false,
    sendSeen: false, waitUntilMsgSent: true, ignoreQuoteErrors: false }]);
});

test('quoted replies require the same peer and never silently remove the quote', async (t) => {
  const f = await fixture(t);
  f.clients[0].quote = { from: '123@c.us', fromMe: false };
  const frame = outbound({ message: { ...outbound().message, reply_to_external_id: 'wa:source:1' } });
  await f.transport.send(frame);
  assert.equal(f.clients[0].calls[0][2].quotedMessageId, 'wa:source:1');
  f.clients[0].quote = { fromMe: true, to: 'other@c.us' };
  await assert.rejects(f.transport.send(frame), { code: 'invalid_quote' });
  assert.equal(f.clients[0].calls.length, 1);
});

test('unsupported outgoing media and invalid peers fail before calling the provider', async (t) => {
  const f = await fixture(t);
  await assert.rejects(f.transport.send(outbound({ message: { text: 'caption', kind: 'image', payload: {} } })), { code: 'unsupported_kind' });
  await assert.rejects(f.transport.send(outbound({ peer_id: 'status@broadcast' })), { code: 'invalid_peer' });
  assert.equal(f.clients[0].calls.length, 0);
});

test('normalized receive events and QR callbacks reach the SDK, never raw objects', async (t) => {
  const f = await fixture(t);
  f.clients[0].emit('message', source());
  f.clients[0].emit('qr', 'FAKE_QR');
  await until(() => f.messages.length === 1 && f.events.includes('pairing'));
  assert.equal(f.messages[0].text, 'سلام é 📎');
  assert.equal('rawData' in f.messages[0], false);
});

test('pair mode has no message ingress or outgoing sends', async (t) => {
  const f = await fixture(t, { mode: 'pair' });
  f.clients[0].emit('message', source());
  await new Promise(setImmediate);
  assert.equal(f.messages.length, 0);
  await assert.rejects(f.transport.send(outbound()), { code: 'pair_mode' });
});

test('disconnection retries are bounded and discard callbacks from old clients', async (t) => {
  const timers = [];
  const f = await fixture(t, { config: { reconnect_limit: 1 },
    setTimer: (callback, delay) => { timers.push({ callback, delay }); return timers.length; }, clearTimer: () => {} });
  f.clients[0].emit('disconnected', 'TIMEOUT');
  await until(() => timers.length === 1);
  await assert.rejects(f.transport.send(outbound()), { code: 'disconnected' });
  assert.equal(timers[0].delay, 1_000);
  timers[0].callback();
  await until(() => f.clients.length === 2 && f.events.filter((event) => event === 'ready').length === 2);
  assert.equal(f.clients[0].destroyed, 1);
  f.clients[0].emit('message', source());
  f.clients[1].emit('disconnected', 'TIMEOUT');
  await until(() => f.failures.includes('reconnect_exhausted'));
  assert.equal(f.messages.length, 0);
  assert.equal(timers.length, 1);
});

test('logout, auth failure and conflicts require explicit recovery without takeover', async (t) => {
  for (const [event, reason, code] of [['disconnected', 'LOGOUT', 'pairing_required'],
    ['disconnected', 'CONFLICT', 'session_conflict'], ['auth_failure', 'PRIVATE_REASON', 'authentication_failed']]) {
    const f = await fixture(t);
    f.clients[0].emit(event, reason);
    await until(() => f.failures.includes(code));
    assert.equal(f.failures.includes('PRIVATE_REASON'), false);
  }
});

test('invalid config and symlink profiles fail closed before a browser starts', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-wa-invalid-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  for (const config of [{ reconnect_limit: 11 }, { reconnect_limit: -1 }, { browser_executable_path: 'relative' },
    { self_chat: 'true' }, { self_chat: 1 }, { self_chat: null }]) {
    await assert.rejects(createWhatsAppTransport({ state_dir: directory, mode: 'run', config }, {}), { code: 'invalid_config' });
  }
  await mkdir(join(directory, 'other'));
  await symlink(join(directory, 'other'), join(directory, 'whatsapp-auth'));
  await assert.rejects(createWhatsAppTransport({ state_dir: directory, mode: 'run', config: {} }, {}), { code: 'invalid_state' });
});
