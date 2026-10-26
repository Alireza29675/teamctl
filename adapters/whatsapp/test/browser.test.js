import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { closeClient } from '../src/browser.js';

test('stalled browser destroy terminates and reaps its owned detached process', async (t) => {
  if (process.platform === 'win32') { t.skip('Unix detached browser process group'); return; }
  const child = spawn(process.execPath, ['-e', 'process.stdout.write("ready\\n"); setInterval(() => {}, 1000)'],
    { detached: true, stdio: ['ignore', 'pipe', 'ignore'] });
  t.after(() => { if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL'); });
  await once(child.stdout, 'data');
  const client = { pupBrowser: { process: () => child }, removeAllListeners: () => {}, destroy: () => new Promise(() => {}) };
  await closeClient(client, { destroyTimeoutMs: 10, reapTimeoutMs: 100 });
  assert.notEqual(child.signalCode, null);
  assert.throws(() => process.kill(child.pid, 0), { code: 'ESRCH' });
});

test('normal browser destroy performs no logout and needs no forced signal', async () => {
  let destroyed = 0;
  let closed = 0;
  const client = { pupBrowser: { close: async () => { closed += 1; } },
    removeAllListeners: () => {}, destroy: async () => { destroyed += 1; } };
  await closeClient(client);
  assert.equal(destroyed, 1);
  assert.equal(closed, 1);
});
