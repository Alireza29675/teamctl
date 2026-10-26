import { mkdir, lstat, chmod } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { AdapterError } from '@teamctl/adapter-sdk';
import { closeClient } from './browser.js';

function whatsappPeer(value, { group = true } = {}) {
  return typeof value === 'string' && /^[a-zA-Z0-9_-]+@(c\.us|lid|g\.us)$/u.test(value)
    && (group || !value.endsWith('@g.us'));
}

export async function normalizeIncoming(message, { selfChat = false, selfIds = new Set() } = {}) {
  const own = Boolean(message.fromMe || message.id?.fromMe);
  if (own && (!selfChat || !['android', 'ios'].includes(message.deviceType)
    || !selfIds.has(message.from) || !selfIds.has(message.to))) return null;
  const peer = own ? message.to : message.from;
  if (message.isStatus || message.broadcast
    || message.type !== 'chat' || message.hasMedia || !message.body?.trim()
    || !whatsappPeer(peer) || (own && !whatsappPeer(peer, { group: false }))) return null;
  const group = peer.endsWith('@g.us');
  const sender = group ? message.author : message.from;
  if (!whatsappPeer(sender, { group: false }) || typeof message.id?._serialized !== 'string') return null;
  let reply = null;
  if (message.hasQuotedMsg) {
    try { reply = (await message.getQuotedMessage())?.id?._serialized ?? null; }
    catch { /* Text remains usable when WhatsApp no longer has the quoted message. */ }
  }
  return { external_id: message.id._serialized, peer_id: peer, sender_id: sender,
    text: message.body, kind: 'text', payload: null, reply_to_external_id: reply };
}

async function defaultClient(options, authPath) {
  const { default: WhatsApp } = await import('whatsapp-web.js');
  return new WhatsApp.Client({ ...options,
    authStrategy: new WhatsApp.LocalAuth({ clientId: 'teamctl', dataPath: authPath }) });
}

function deadline(operation, milliseconds) {
  let timer;
  return Promise.race([operation, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new AdapterError('startup_timeout', 'WhatsApp startup timed out', true)), milliseconds);
  })]).finally(() => clearTimeout(timer));
}

async function ownAccountIds(client) {
  const wid = client.info?.wid?._serialized;
  const ids = new Set();
  if (!whatsappPeer(wid, { group: false })) return ids;
  ids.add(wid);
  try {
    // Accept aliases only when the provider ties them to the current account ID.
    const aliases = await deadline(Promise.resolve().then(() => client.getContactLidAndPhone([wid])), 5_000);
    const pair = Array.isArray(aliases) ? aliases[0] : null;
    if (pair?.pn === wid || pair?.lid === wid) {
      for (const id of [pair.pn, pair.lid]) {
        if (whatsappPeer(id, { group: false })) ids.add(id);
      }
    }
  } catch { /* The verified wid remains usable if alias lookup is unavailable. */ }
  return ids;
}

export async function createWhatsAppTransport(initialization, context, {
  clientFactory = defaultClient, setTimer = setTimeout, clearTimer = clearTimeout,
  reconnectBaseMs = 1_000, startupTimeoutMs = initialization.mode === 'pair' ? 120_000 : 45_000,
} = {}) {
  const config = initialization.config;
  if (config.self_chat !== undefined && typeof config.self_chat !== 'boolean') {
    throw new AdapterError('invalid_config', 'self_chat must be a boolean');
  }
  const reconnectLimit = config.reconnect_limit ?? 3;
  if (!Number.isSafeInteger(reconnectLimit) || reconnectLimit < 0 || reconnectLimit > 10) {
    throw new AdapterError('invalid_config', 'reconnect_limit must be an integer from 0 to 10');
  }
  if (config.browser_executable_path != null
    && (typeof config.browser_executable_path !== 'string' || !isAbsolute(config.browser_executable_path))) {
    throw new AdapterError('invalid_config', 'browser_executable_path must be an absolute path');
  }
  const authPath = join(initialization.state_dir, 'whatsapp-auth');
  const cachePath = join(initialization.state_dir, 'whatsapp-cache');
  await mkdir(authPath, { recursive: true, mode: 0o700 });
  await mkdir(cachePath, { recursive: true, mode: 0o700 });
  for (const path of [authPath, cachePath]) {
    const stat = await lstat(path);
    if (!stat.isDirectory() || stat.isSymbolicLink()) {
      throw new AdapterError('invalid_state', 'WhatsApp profile directory is unsafe');
    }
    await chmod(path, 0o700);
  }
  const controller = new AbortController();
  const options = {
    puppeteer: { headless: true, dumpio: false, args: [], timeout: 30_000, signal: controller.signal,
      handleSIGINT: true, handleSIGTERM: true, handleSIGHUP: true },
    webVersionCache: { type: 'local', path: cachePath },
    takeoverOnConflict: false,
    qrMaxRetries: 5,
    authTimeoutMs: 30_000,
    deviceName: 'teamctl',
  };
  if (config.browser_executable_path) options.puppeteer.executablePath = config.browser_executable_path;
  let client = null;
  let connected = false;
  let closing = false;
  let generation = 0;
  let attempts = 0;
  let reconnectTimer = null;
  let launchPromise = null;
  let pendingEvents = 0;
  let selfIds = new Set();
  const outgoingIds = new Set();
  const pendingSends = new Set();

  const fail = async (code) => {
    if (!closing) await context.fail(code);
  };

  const listen = (current, event, handler, epoch) => {
    current.on(event, (...args) => {
      if (closing || epoch !== generation) return;
      if (pendingEvents >= 64) { fail('ingress_backpressure').catch(() => {}); return; }
      pendingEvents += 1;
      Promise.resolve().then(() => handler(...args)).catch(() => fail('whatsapp_event_failed'))
        .finally(() => { pendingEvents -= 1; }).catch(() => {});
    });
  };

  const reconnect = async () => {
    if (closing || reconnectTimer !== null) return;
    if (attempts >= reconnectLimit) { await fail('reconnect_exhausted'); return; }
    const delay = Math.min(30_000, reconnectBaseMs * 2 ** attempts);
    attempts += 1;
    reconnectTimer = setTimer(() => {
      reconnectTimer = null;
      launchPromise = launch().catch(reconnect);
    }, delay);
  };

  const launch = async () => {
    const epoch = ++generation;
    connected = false;
    selfIds = new Set();
    if (client) {
      await closeClient(client);
    }
    if (closing) return;
    client = await clientFactory(options, authPath);
    const current = client;
    if (closing) { await closeClient(current); return; }
    listen(current, 'qr', (qr) => context.pairing(qr), epoch);
    listen(current, 'authenticated', () => context.connection('connecting'), epoch);
    listen(current, 'auth_failure', async () => {
      connected = false;
      await context.connection('auth_failed');
      await fail('authentication_failed');
    }, epoch);
    listen(current, 'ready', async () => {
      const ids = config.self_chat && initialization.mode === 'run' ? await ownAccountIds(current) : new Set();
      if (closing || epoch !== generation) return;
      selfIds = ids;
      connected = true;
      await context.connection('connected');
      await context.ready();
    }, epoch);
    listen(current, 'disconnected', async (reason) => {
      connected = false;
      await context.connection('disconnected');
      if (reason === 'LOGOUT') {
        await context.connection('pairing_required');
        await fail('pairing_required');
      } else if (reason === 'CONFLICT') {
        await fail('session_conflict');
      } else await reconnect();
    }, epoch);
    listen(current, 'message', async (message) => {
      if (initialization.mode !== 'run') return;
      const normalized = await normalizeIncoming(message);
      if (normalized) await context.receive(normalized);
    }, epoch);
    listen(current, 'message_create', async (message) => {
      if (initialization.mode !== 'run' || !config.self_chat
        || !(message.fromMe || message.id?.fromMe)) return;
      if (outgoingIds.has(message.id?._serialized) || [...pendingSends].some((send) =>
        send.text === message.body && (send.peer === message.to
          || (selfIds.has(send.peer) && selfIds.has(message.to))))) return;
      const normalized = await normalizeIncoming(message, { selfChat: true, selfIds });
      if (normalized) await context.receive(normalized);
    }, epoch);
    await context.connection('connecting');
    try { await deadline(Promise.resolve(current.initialize()), startupTimeoutMs); }
    finally { if (closing || epoch !== generation) await closeClient(current); }
  };

  return {
    async start() {
      launchPromise = launch().catch(reconnect);
      await launchPromise;
    },
    async send({ peer_id: peer, message }) {
      if (!connected || closing) throw new AdapterError('disconnected', 'WhatsApp is disconnected', true);
      if (initialization.mode !== 'run') throw new AdapterError('pair_mode', 'Messages cannot be sent while pairing');
      if (!whatsappPeer(peer)) throw new AdapterError('invalid_peer', 'Unsupported WhatsApp peer ID');
      if (message.kind !== 'text' || message.payload != null) {
        throw new AdapterError('unsupported_kind', 'WhatsApp supports text and quoted text replies');
      }
      const sendOptions = { linkPreview: false, sendSeen: false, waitUntilMsgSent: true,
        ignoreQuoteErrors: false };
      if (message.reply_to_external_id) {
        const quoted = await client.getMessageById(message.reply_to_external_id);
        const quotePeer = quoted?.fromMe ? quoted.to : quoted?.from;
        if (quotePeer !== peer) throw new AdapterError('invalid_quote', 'Quoted message does not belong to this peer');
        sendOptions.quotedMessageId = message.reply_to_external_id;
      }
      const pending = { peer, text: message.text };
      pendingSends.add(pending);
      try {
        const sent = await client.sendMessage(peer, message.text, sendOptions);
        if (!sent?.id?._serialized) throw new AdapterError('send_failed', 'WhatsApp did not accept the message', true);
        outgoingIds.add(sent.id._serialized);
        // Match the SDK receipt capacity without growing a long-lived echo cache.
        if (outgoingIds.size > 10_000) outgoingIds.delete(outgoingIds.values().next().value);
        return sent.id._serialized;
      } finally { pendingSends.delete(pending); }
    },
    async close() {
      closing = true;
      connected = false;
      generation += 1;
      controller.abort();
      if (reconnectTimer !== null) clearTimer(reconnectTimer);
      reconnectTimer = null;
      await closeClient(client);
    },
  };
}
