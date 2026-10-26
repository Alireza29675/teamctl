#!/usr/bin/env node
import { runAdapter } from '@teamctl/adapter-sdk';
import { createWhatsAppTransport } from './transport.js';
import { isolatedIO } from './stdio.js';

process.umask(0o077);
const { output, diagnostics } = isolatedIO();

try {
  process.exitCode = await runAdapter({ factory: createWhatsAppTransport, output, diagnostics,
    displayName: 'WhatsApp', capabilities: ['text', 'quoted_reply'] });
} catch {
  diagnostics.write('adapter: startup_failed\n');
  process.exitCode = 1;
}
