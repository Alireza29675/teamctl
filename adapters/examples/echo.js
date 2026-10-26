#!/usr/bin/env node
import { createHash } from 'node:crypto';
import { runAdapter } from '../sdk/src/index.js';

process.umask(0o077);
process.exitCode = await runAdapter({ displayName: 'Echo example', capabilities: ['text'],
  factory: async (initialization, context) => ({
    async start() {
      await context.ready();
      if (initialization.config.greeting != null) {
        await context.receive({ external_id: 'echo:greeting', peer_id: initialization.config.peer_id,
          sender_id: 'echo-user', text: initialization.config.greeting });
      }
    },
    async send(frame) {
      const external = `echo:${createHash('sha256').update(frame.request_id).digest('hex')}`;
      await context.receive({ external_id: external, peer_id: frame.peer_id,
        sender_id: 'echo-user', text: frame.message.text });
      return external;
    },
    async close() {},
  }),
});
