import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, rm, access, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { incoming } from './helpers.js';

function launch(directory, config = {}) {
  const child = spawn(process.execPath, [fileURLToPath(new URL('child-fixture.js', import.meta.url))],
    { stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '';
  let errors = '';
  const waiters = [];
  const frames = [];
  child.stdout.on('data', (chunk) => {
    output += chunk.toString();
    let end;
    while ((end = output.indexOf('\n')) !== -1) {
      frames.push(JSON.parse(output.slice(0, end)));
      output = output.slice(end + 1);
    }
    for (const waiter of waiters) waiter();
  });
  child.stderr.on('data', (chunk) => { errors += chunk.toString(); });
  const send = (frame) => child.stdin.write(`${JSON.stringify(frame)}\n`);
  send({ type: 'initialize', version: 1, instance_id: 'child', state_dir: directory,
    mode: 'run', config });
  return { child, frames, errors: () => errors, send,
    wait: async (condition) => {
      if (condition()) return;
      await new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('Fixture timed out')), 3_000);
        waiters.push(() => { if (condition()) { clearTimeout(timer); resolve(); } });
      });
    } };
}

test('process crash recovery reclaims only dead owners and replays durable state', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'teamctl-crash-'));
  const children = [];
  t.after(async () => {
    for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await rm(directory, { recursive: true, force: true });
  });
  const first = launch(directory, { initial_message: incoming() });
  children.push(first.child);
  await first.wait(() => first.frames.some((frame) => frame.type === 'message'));
  first.send({ type: 'send', request_id: 'delivery:1', peer_id: 'peer:1', message: { text: 'hello' } });
  await first.wait(() => first.frames.some((frame) => frame.type === 'sent'));
  const died = once(first.child, 'exit');
  first.child.kill('SIGKILL');
  await died;
  const second = launch(directory);
  children.push(second.child);
  await second.wait(() => second.frames.some((frame) => frame.type === 'message'));
  second.send({ type: 'send', request_id: 'delivery:1', peer_id: 'peer:1', message: { text: 'hello' } });
  await second.wait(() => second.frames.some((frame) => frame.type === 'sent'));
  assert.equal(second.frames.find((frame) => frame.type === 'sent').external_id, 'fixture:delivery:1');
  second.send({ type: 'accepted', peer_id: 'peer:1', external_id: 'source:1' });
  const exited = once(second.child, 'exit');
  second.send({ type: 'shutdown' });
  second.child.stdin.end();
  assert.equal((await exited)[0], 0);
  await assert.rejects(access(join(directory, '.adapter-lock')), { code: 'ENOENT' });
  assert.equal(JSON.parse(await readFile(join(directory, 'adapter-state.json'), 'utf8')).inbound.length, 0);
});

test('stdin EOF and SIGTERM release state and invoke owned cleanup', async (t) => {
  for (const stop of ['eof', 'signal']) {
    const directory = await mkdtemp(join(tmpdir(), 'teamctl-stop-'));
    const fixture = launch(directory);
    t.after(async () => {
      if (fixture.child.exitCode === null && fixture.child.signalCode === null) fixture.child.kill('SIGKILL');
      await rm(directory, { recursive: true, force: true });
    });
    await fixture.wait(() => fixture.frames.some((frame) => frame.type === 'ready'));
    const exited = once(fixture.child, 'exit');
    if (stop === 'eof') fixture.child.stdin.end(); else fixture.child.kill('SIGTERM');
    assert.equal((await exited)[0], 0);
    assert.equal(await readFile(join(directory, 'closed'), 'utf8'), 'yes');
    await assert.rejects(access(join(directory, '.adapter-lock')), { code: 'ENOENT' });
  }
});
