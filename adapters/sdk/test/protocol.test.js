import test from 'node:test';
import assert from 'node:assert/strict';
import { Readable, Writable } from 'node:stream';
import { readFrames, hostFrame, encoded, inbound, MAX_FRAME_BYTES, MAX_TEXT_BYTES } from '../src/protocol.js';
import { ProtocolOutput } from '../src/output.js';

async function parse(chunks) {
  const frames = [];
  for await (const frame of readFrames(Readable.from(chunks))) frames.push(frame);
  return frames;
}

test('UTF-8 frames remain valid across arbitrary byte boundaries', async () => {
  const frame = { type: 'send', request_id: '42', peer_id: 'human', message: { text: 'سلام é 📎\nsecond line' } };
  const bytes = Buffer.from(encoded(frame));
  const actual = await parse([...bytes].map((byte) => Buffer.from([byte])));
  assert.deepEqual(actual, [hostFrame(frame)]);
});

test('malformed, unsupported, privileged and unknown fields fail closed', async () => {
  for (const body of ['{\n', '\n', JSON.stringify({ type: 'initialize', version: 2, instance_id: 'a',
    state_dir: '/private/a', mode: 'run', config: {} }) + '\n',
  JSON.stringify({ type: 'send', request_id: 'a', peer_id: 'p', message: { text: 'x', kind: 'system' } }) + '\n',
  JSON.stringify({ type: 'shutdown', command: 'rm' }) + '\n']) {
    await assert.rejects(parse([body]), { code: 'protocol_error' });
  }
  assert.throws(() => inbound({ external_id: '1', peer_id: 'p', sender_id: 's', text: 'x',
    kind: 'media_pending' }), { code: 'protocol_error' });
});

test('oversized frames, text, invalid UTF-8 and incomplete final lines are rejected', async () => {
  await assert.rejects(parse([Buffer.alloc(MAX_FRAME_BYTES, 65)]), { code: 'protocol_error' });
  await assert.rejects(parse([Buffer.from([123, 34, 0xff, 34, 125, 10])]), { code: 'protocol_error' });
  await assert.rejects(parse(['{"type":"shutdown"}']), { code: 'protocol_error' });
  assert.throws(() => hostFrame({ type: 'send', request_id: 'r', peer_id: 'p',
    message: { text: 'x'.repeat(MAX_TEXT_BYTES + 1) } }), { code: 'protocol_error' });
  await assert.rejects(parse(['{"type":"send","request_id":"r","peer_id":"p","message":{"text":"\\ud800"}}\n']),
    { code: 'protocol_error' });
});

test('identities cannot carry terminal controls or surrounding whitespace', () => {
  for (const peer of [' p', 'p\n', '\x1bp', '']) {
    assert.throws(() => hostFrame({ type: 'accepted', peer_id: peer, external_id: '1' }), { code: 'protocol_error' });
  }
});

test('typing is an outgoing capability and controls cannot enter agent inboxes', () => {
  const typing = hostFrame({ type: 'send', request_id: 'r', peer_id: 'p',
    message: { text: '', kind: 'typing', payload: { expires_at: 42 } } });
  assert.equal(typing.message.kind, 'typing');
  for (const kind of ['typing', 'reaction']) {
    assert.throws(() => inbound({ external_id: '1', peer_id: 'p', sender_id: 's',
      text: '', kind }), { code: 'protocol_error' });
  }
});

test('output queue is bounded and preserves write order under backpressure', async () => {
  let release;
  const seen = [];
  const output = new Writable({ write(chunk, _encoding, done) { seen.push(JSON.parse(chunk)); release = done; } });
  const writer = new ProtocolOutput(output, { maxFrames: 2 });
  const first = writer.send({ type: 'connection', state: 'connecting' });
  const second = writer.send({ type: 'connection', state: 'connected' });
  await assert.rejects(writer.send({ type: 'connection', state: 'failed' }), { code: 'output_backpressure' });
  await new Promise(setImmediate);
  release();
  await first;
  await new Promise(setImmediate);
  release();
  await second;
  await writer.close();
  assert.deepEqual(seen.map((frame) => frame.state), ['connecting', 'connected']);
});
