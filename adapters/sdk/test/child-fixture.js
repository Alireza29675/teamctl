import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { runAdapter } from '../src/index.js';

process.umask(0o077);
process.exitCode = await runAdapter({ displayName: 'Child fixture', capabilities: ['text'],
  factory: async (initialization, context) => ({
    async start() {
      await context.ready();
      if (initialization.config.initial_message) await context.receive(initialization.config.initial_message);
    },
    async send(frame) { return `fixture:${frame.request_id}`; },
    async close() { await writeFile(join(initialization.state_dir, 'closed'), 'yes', { mode: 0o600 }); },
  }),
});
