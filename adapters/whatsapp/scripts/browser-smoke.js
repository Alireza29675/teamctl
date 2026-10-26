import { access, mkdtemp, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { join, isAbsolute } from 'node:path';
import { tmpdir } from 'node:os';
import { createRequire } from 'node:module';
import { closeClient } from '../src/browser.js';
import { createWhatsAppTransport } from '../src/transport.js';
import { isolatedIO } from '../src/stdio.js';

process.umask(0o077);
const { output } = isolatedIO();
const args = process.argv.slice(2);
const executable = args[args.indexOf('--executable') + 1];
const unpaired = args.includes('--unpaired-qr');
const result = { browserStarted: false, blankPage: false, qrGenerated: false,
  linkedAccount: false, messagesSent: 0, cleanup: false, outcome: 'not_started' };
let directory;
let client;
let transport;
let timer;

try {
  if (!args.includes('--executable') || !isAbsolute(executable)) throw new Error('invalid_arguments');
  await access(executable, constants.X_OK);
  directory = await mkdtemp(join(tmpdir(), 'teamctl-browser-smoke-'));
  const require = createRequire(import.meta.url);
  const puppeteer = require(require.resolve('puppeteer', { paths: [require.resolve('whatsapp-web.js')] }));
  const browser = await puppeteer.launch({ executablePath: executable, userDataDir: join(directory, 'blank-profile'),
    headless: true, dumpio: false, args: [], timeout: 30_000 });
  client = { pupBrowser: browser, removeAllListeners() {}, destroy: () => browser.close() };
  result.browserStarted = true;
  const page = await browser.newPage();
  await page.goto('about:blank');
  // These are the public browser/page APIs used by WhatsApp's text/QR path.
  for (const method of ['evaluate', 'exposeFunction', 'evaluateOnNewDocument', 'goto', 'on',
    'setUserAgent', 'setBypassCSP', 'setRequestInterception', 'authenticate']) {
    if (typeof page[method] !== 'function') throw new Error('missing_public_method');
  }
  await page.exposeFunction('teamctlFixture', () => 'ok');
  if (await page.evaluate(() => window.teamctlFixture()) !== 'ok') throw new Error('binding_failed');
  result.blankPage = page.url() === 'about:blank';
  await closeClient(client);
  client = null;
  result.outcome = 'blank_passed';
  if (unpaired) {
    let settle;
    const completed = new Promise((resolve) => { settle = resolve; });
    transport = await createWhatsAppTransport({ instance_id: 'smoke', mode: 'pair',
      state_dir: join(directory, 'unpaired'), config: { browser_executable_path: executable, reconnect_limit: 0 } },
    { pairing: async () => { result.qrGenerated = true; settle(); },
      ready: async () => { result.linkedAccount = true; settle(); },
      receive: async () => { throw new Error('unexpected_ingress'); },
      connection: async () => {}, fail: async () => settle() }, { startupTimeoutMs: 60_000 });
    const starting = transport.start().catch(() => settle());
    await Promise.race([completed, new Promise((resolve) => { timer = setTimeout(resolve, 60_000); })]);
    clearTimeout(timer);
    await transport.close();
    await Promise.race([starting, new Promise((resolve) => { timer = setTimeout(resolve, 3_000); })]);
    clearTimeout(timer);
    transport = null;
    result.outcome = result.qrGenerated && !result.linkedAccount ? 'unpaired_qr_passed' : 'unpaired_qr_unverified';
  }
} catch {
  result.outcome = 'browser_smoke_failed';
} finally {
  clearTimeout(timer);
  if (transport) await transport.close().catch(() => {});
  if (client) await closeClient(client).catch(() => {});
  if (directory) {
    await rm(directory, { recursive: true, force: true });
    result.cleanup = true;
  }
  output.write(`${JSON.stringify(result)}\n`);
  if (!['blank_passed', 'unpaired_qr_passed'].includes(result.outcome)) process.exitCode = 1;
}
