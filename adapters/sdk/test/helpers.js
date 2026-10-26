import { Writable } from 'node:stream';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setImmediate as tick } from 'node:timers/promises';
import { AdapterSession } from '../src/index.js';
import { hostFrame } from '../src/protocol.js';

export function capture() {
  let body = '';
  const stream = new Writable({ write(chunk, _encoding, done) { body += chunk.toString(); done(); } });
  return { stream, text: () => body,
    frames: () => body.split('\n').filter(Boolean).map((line) => JSON.parse(line)) };
}

export async function until(condition) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (condition()) return;
    await tick();
  }
  throw new Error('Fixture did not reach expected state');
}

export async function fixture(test, options = {}) {
  const directory = options.directory ?? await mkdtemp(join(tmpdir(), 'teamctl-sdk-'));
  const output = capture();
  const diagnostics = capture();
  let context;
  let sends = 0;
  let closes = 0;
  const session = new AdapterSession({ displayName: 'Fixture', output: output.stream,
    diagnostics: diagnostics.stream, ...options,
    factory: async (initialization, providedContext) => {
      context = providedContext;
      return { start: async () => context.ready(),
        send: async (frame) => {
          sends += 1;
          if (options.send) return options.send(frame, context);
          return `provider:${sends}`;
        },
        close: async () => { closes += 1; if (options.close) await options.close(); },
      };
    } });
  await session.handle(hostFrame({ type: 'initialize', version: 1, instance_id: 'fixture',
    state_dir: directory, mode: options.mode ?? 'run', config: {} }));
  await until(() => output.frames().some((frame) => frame.type === 'ready'));
  test.after(async () => { await session.close(); if (!options.directory) await rm(directory, { recursive: true, force: true }); });
  return { session, directory, context, output, diagnostics, sends: () => sends, closes: () => closes };
}

export const incoming = (extra = {}) => ({ external_id: 'source:1', peer_id: 'peer:1',
  sender_id: 'human:1', text: 'سلام é 📎', ...extra });
export const send = (extra = {}) => hostFrame({ type: 'send', request_id: 'delivery:1',
  peer_id: 'peer:1', message: { text: 'Hello', kind: 'text', payload: null, reply_to_external_id: null }, ...extra });
