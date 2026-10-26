import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, stat, mkdtemp, rm, mkdir, symlink, writeFile, chmod } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { AdapterError, AdapterSession } from '../src/index.js';
import { DurableState } from '../src/state.js';
import { fixture, incoming, send } from './helpers.js';
import { execFileSync } from 'node:child_process';

test('incoming events persist before output and acknowledgements are scoped by peer', async (t) => {
  const f = await fixture(t);
  await f.context.receive(incoming());
  await f.context.receive(incoming({ peer_id: 'peer:2' }));
  const persisted = JSON.parse(await readFile(join(f.directory, 'adapter-state.json'), 'utf8'));
  assert.equal(persisted.inbound.length, 2);
  await f.session.handle({ type: 'accepted', peer_id: 'peer:2', external_id: 'source:1' });
  await f.session.handle({ type: 'accepted', peer_id: 'foreign', external_id: 'source:1' });
  const remaining = JSON.parse(await readFile(join(f.directory, 'adapter-state.json'), 'utf8'));
  assert.deepEqual(remaining.inbound.map((event) => event.peer_id), ['peer:1']);
  await f.session.handle({ type: 'rejected', peer_id: 'peer:1', external_id: 'source:1', code: 'forbidden_sender' });
  assert.equal(JSON.parse(await readFile(join(f.directory, 'adapter-state.json'), 'utf8')).inbound.length, 0);
});

test('pending inbound replays after restart, while accepted records do not', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-replay-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const first = await fixture(t, { directory });
  await first.context.receive(incoming());
  await first.session.close();
  const second = await fixture(t, { directory });
  assert.equal(second.output.frames().filter((frame) => frame.type === 'message').length, 1);
  await second.session.handle({ type: 'accepted', peer_id: 'peer:1', external_id: 'source:1' });
  await second.session.close();
  const third = await fixture(t, { directory });
  assert.equal(third.output.frames().filter((frame) => frame.type === 'message').length, 0);
});

test('provider receipts suppress outgoing replay across restart and detect changed content', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-receipt-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const first = await fixture(t, { directory });
  await first.session.handle(send());
  await first.session.handle(send());
  assert.equal(first.sends(), 1);
  await first.session.close();
  const second = await fixture(t, { directory });
  await second.session.handle(send());
  assert.equal(second.sends(), 0);
  const sent = second.output.frames().find((frame) => frame.type === 'sent');
  assert.equal(sent.external_id, 'provider:1');
  await second.session.handle(send({ peer_id: 'other' }));
  assert.equal(second.output.frames().at(-1).code, 'request_id_reused');
});

test('incoming receive can complete during a send without a state queue deadlock', async (t) => {
  const f = await fixture(t, { send: async (_frame, context) => {
    await context.receive(incoming());
    return 'provider:echo';
  } });
  await f.session.handle(send());
  assert.equal(f.output.frames().at(-1).type, 'sent');
  assert.equal(f.output.frames().filter((frame) => frame.type === 'message').length, 1);
});

test('provider errors expose controlled codes and never raw secrets', async (t) => {
  const f = await fixture(t, { send: async () => { throw new Error('QR_SECRET credentials'); } });
  await f.session.handle(send());
  assert.equal(f.output.frames().at(-1).code, 'transport_failed');
  assert.equal(f.output.text().includes('QR_SECRET'), false);
  assert.equal(f.diagnostics.text().includes('credentials'), false);
  const controlled = await fixture(t, { send: async () => { throw new AdapterError('disconnected', 'Transport disconnected', true); } });
  await controlled.session.handle(send());
  assert.equal(controlled.output.frames().at(-1).retryable, true);
});

test('unsupported send kinds return an actionable permanent error', async (t) => {
  const f = await fixture(t);
  await f.session.handle(send({ message: { text: '', kind: 'image', payload: {} } }));
  assert.equal(f.output.frames().at(-1).code, 'unsupported_kind');
  assert.equal(f.sends(), 0);
});

test('QRs are ephemeral and only emitted in explicit pair mode', async (t) => {
  const run = await fixture(t);
  await run.context.pairing('EPHEMERAL_QR');
  assert.equal(run.output.frames().at(-1).state, 'pairing_required');
  assert.equal(run.output.text().includes('EPHEMERAL_QR'), false);
  const pair = await fixture(t, { mode: 'pair' });
  await pair.context.pairing('EPHEMERAL_QR');
  await pair.context.receive(incoming());
  await pair.session.handle(send());
  assert.deepEqual(pair.output.frames().find((frame) => frame.type === 'pairing'), { type: 'pairing', qr: 'EPHEMERAL_QR' });
  assert.equal(pair.output.frames().at(-1).code, 'pair_mode');
  assert.equal(pair.output.frames().some((frame) => frame.type === 'message'), false);
  assert.equal(pair.diagnostics.text().includes('EPHEMERAL_QR'), false);
  const privateState = await readFile(join(pair.directory, 'adapter-state.json'), 'utf8');
  assert.equal(privateState.includes('EPHEMERAL_QR'), false);
  assert.equal(JSON.parse(privateState).inbound.length, 0);
});

test('state permissions are private and a second owner cannot take over', async (t) => {
  const f = await fixture(t);
  await f.context.receive(incoming());
  assert.equal((await stat(f.directory)).mode & 0o777, 0o700);
  assert.equal((await stat(join(f.directory, 'adapter-state.json'))).mode & 0o777, 0o600);
  assert.equal((await stat(join(f.directory, '.adapter-lock', 'owner.json'))).mode & 0o777, 0o600);
  await assert.rejects(DurableState.open(f.directory), { code: 'state_locked' });
});

test('workspace and symlink state directories fail closed', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-unsafe-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(join(directory, 'real'));
  await symlink(join(directory, 'real'), join(directory, 'alias'));
  await assert.rejects(DurableState.open(join(directory, 'alias')), { code: 'invalid_state' });
  await assert.rejects(DurableState.open(process.cwd()), { code: 'invalid_state' });
});

test('EOF and explicit shutdown close owned transport exactly once', async (t) => {
  for (const input of [[], ['{"type":"shutdown"}\n']]) {
    const f = await fixture(t);
    assert.equal(await f.session.run(Readable.from(input)), 0);
    await f.session.close();
    assert.equal(f.closes(), 1);
  }
});

test('a hanging send is bounded and closes the transport', async (t) => {
  const f = await fixture(t, { operationTimeoutMs: 10, send: () => new Promise(() => {}) });
  await f.session.handle(send());
  assert.equal(f.output.frames().at(-1).code, 'transport_timeout');
  assert.equal(f.closes(), 1);
  assert.equal(f.session.exitCode, 1);
});

test('providers without message IDs still get durable replay receipts', async (t) => {
  const f = await fixture(t, { send: async () => null });
  await f.session.handle(send());
  await f.session.handle(send());
  assert.equal(f.sends(), 1);
  assert.equal(f.output.frames().at(-1).external_id, null);
});

test('source ID reuse and a foreign instance cannot change pending history', async (t) => {
  const f = await fixture(t);
  await f.context.receive(incoming());
  await assert.rejects(f.context.receive(incoming({ text: 'changed' })), { code: 'source_id_reused' });
  await f.session.close();
  await assert.rejects(DurableState.open(f.directory, 'foreign-instance'), { code: 'invalid_state' });
  assert.equal(JSON.parse(await readFile(join(f.directory, 'adapter-state.json'), 'utf8')).inbound[0].text,
    incoming().text);
});

test('private state rejects permissive files and FIFOs without blocking', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-state-unsafe-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const path = join(directory, 'adapter-state.json');
  await writeFile(path, '{}', { mode: 0o600 });
  await chmod(path, 0o644);
  await assert.rejects(DurableState.open(directory), { code: 'invalid_state' });
  await rm(path);
  if (process.platform !== 'win32') {
    execFileSync('mkfifo', [path]);
    await assert.rejects(DurableState.open(directory), { code: 'invalid_state' });
  }
});

test('full inbound and receipt stores fail closed instead of expiring history', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-state-full-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, 'adapter-state.json'), JSON.stringify({ version: 1, instance_id: 'full',
    inbound: Array.from({ length: 256 }, (_, index) => incoming({ external_id: `source:${index}` })),
    receipts: Array.from({ length: 10_000 }, (_, index) => ({ request_id: `delivery:${index}`,
      external_id: null, fingerprint: 'a'.repeat(64) })) }), { mode: 0o600 });
  const state = await DurableState.open(directory, 'full');
  t.after(() => state.close());
  assert.throws(() => state.ensureReceiptCapacity(), { code: 'receipts_full' });
  await assert.rejects(state.receive(incoming({ external_id: 'new' })), { code: 'inbound_full' });
  assert.equal(state.pending().length, 256);
});

test('Ready declarations match the shared bounded capability contract', () => {
  const create = (capabilities) => new AdapterSession({ factory: async () => ({}),
    displayName: 'Fixture', capabilities });
  const capabilities = Array.from({ length: 32 }, (_, i) => `cap${i}`);
  const session = create(capabilities);
  capabilities.push('extra');
  assert.equal(session.capabilities.length, 32);
  assert.throws(() => create(capabilities), { code: 'protocol_error' });
  assert.throws(() => create(['text\n']), { code: 'protocol_error' });
});
