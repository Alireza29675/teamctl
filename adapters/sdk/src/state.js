import { constants } from 'node:fs';
import { mkdir, open, lstat, chmod, realpath, rename, unlink, rmdir } from 'node:fs/promises';
import { isAbsolute, join, relative, sep } from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { AdapterError, inbound, identity } from './protocol.js';

const MAX_STATE_BYTES = 24 * 1024 * 1024;
const MAX_INBOUND = 256;
const MAX_RECEIPTS = 10_000;

function stateError(code = 'invalid_state') {
  return new AdapterError(code, 'Adapter state is unavailable or unsafe');
}

function owned(stat) {
  return typeof process.getuid !== 'function' || stat.uid === process.getuid();
}

async function readPrivate(path, maximum) {
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || !owned(stat) || (stat.mode & 0o077) !== 0 || stat.size > maximum) throw stateError();
    const buffer = Buffer.alloc(stat.size + 1);
    let size = 0;
    while (size < buffer.length) {
      const { bytesRead } = await handle.read(buffer, size, buffer.length - size, null);
      if (bytesRead === 0) break;
      size += bytesRead;
    }
    if (size > stat.size) throw stateError();
    return new TextDecoder('utf-8', { fatal: true }).decode(buffer.subarray(0, size));
  } finally { await handle.close(); }
}

function inside(root, path) {
  const child = relative(root, path);
  return child === '' || (child !== '..' && !child.startsWith(`..${sep}`) && !isAbsolute(child));
}

export function key(peer, external) {
  return createHash('sha256').update(JSON.stringify([peer, external])).digest('hex');
}

export function fingerprint(peer, message) {
  return createHash('sha256').update(JSON.stringify([peer, message])).digest('hex');
}

export class DurableState {
  #path;
  #lock;
  #token;
  #inbound = new Map();
  #receipts = new Map();
  #receiptsDirty = false;

  static async open(directory, instanceId = 'default') {
    if (!isAbsolute(directory)) throw stateError();
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const stat = await lstat(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink() || !owned(stat)) throw stateError();
    const canonical = await realpath(directory);
    if (inside(await realpath(process.cwd()), canonical)) throw stateError();
    await chmod(canonical, 0o700);
    const state = new DurableState(canonical, identity(instanceId));
    await state.#acquire();
    try { if (!await state.#load()) await state.#persist(); }
    catch (error) { await state.close(); throw error; }
    return state;
  }

  constructor(directory, instanceId) {
    this.directory = directory;
    this.instanceId = instanceId;
    this.#path = join(directory, 'adapter-state.json');
    this.#lock = join(directory, '.adapter-lock');
    this.#token = randomUUID();
  }

  async #acquire() {
    for (let attempt = 0; attempt < 3; attempt += 1) {
      try {
        await mkdir(this.#lock, { mode: 0o700 });
        const handle = await open(join(this.#lock, 'owner.json'), 'wx', 0o600);
        try {
          await handle.writeFile(JSON.stringify({ pid: process.pid, token: this.#token }));
          await handle.sync();
        } finally { await handle.close(); }
        return;
      } catch (error) {
        if (error.code !== 'EEXIST') throw stateError();
      }
      const stat = await lstat(this.#lock);
      if (!stat.isDirectory() || stat.isSymbolicLink() || !owned(stat)) throw stateError();
      let owner;
      try { owner = JSON.parse(await readPrivate(join(this.#lock, 'owner.json'), 4096)); }
      catch { throw stateError('state_locked'); }
      if (!Number.isSafeInteger(owner.pid) || owner.pid <= 0 || typeof owner.token !== 'string') {
        throw stateError('state_locked');
      }
      try { process.kill(owner.pid, 0); throw stateError('state_locked'); }
      catch (error) { if (error.code !== 'ESRCH') throw stateError('state_locked'); }
      let claim;
      try { claim = await open(join(this.#lock, 'reclaim'), 'wx', 0o600); }
      catch { throw stateError('state_locked'); }
      await claim.close();
      const current = JSON.parse(await readPrivate(join(this.#lock, 'owner.json'), 4096));
      if (current.token !== owner.token) throw stateError('state_locked');
      await unlink(join(this.#lock, 'owner.json'));
      await unlink(join(this.#lock, 'reclaim'));
      await rmdir(this.#lock);
    }
    throw stateError('state_locked');
  }

  async #load() {
    let body;
    try { body = await readPrivate(this.#path, MAX_STATE_BYTES); }
    catch (error) { if (error.code === 'ENOENT') return false; throw stateError(); }
    try {
      const value = JSON.parse(body);
      if (value.version !== 1 || value.instance_id !== this.instanceId
        || !Array.isArray(value.inbound) || !Array.isArray(value.receipts)
        || value.inbound.length > MAX_INBOUND || value.receipts.length > MAX_RECEIPTS) throw stateError();
      for (const event of value.inbound) {
        const normalized = inbound(event);
        this.#inbound.set(key(normalized.peer_id, normalized.external_id), normalized);
      }
      for (const receipt of value.receipts) {
        identity(receipt.request_id);
        if (receipt.external_id != null) identity(receipt.external_id);
        if (typeof receipt.fingerprint !== 'string' || !/^[a-f0-9]{64}$/u.test(receipt.fingerprint)) {
          throw stateError();
        }
        this.#receipts.set(receipt.request_id, receipt);
      }
      return true;
    } catch { throw stateError(); }
  }

  async #persist() {
    const body = JSON.stringify({ version: 1, instance_id: this.instanceId, inbound: [...this.#inbound.values()],
      receipts: [...this.#receipts.values()] });
    if (Buffer.byteLength(body) > MAX_STATE_BYTES) throw stateError('state_full');
    const temporary = join(this.directory, `.state-${randomUUID()}.tmp`);
    const handle = await open(temporary, 'wx', 0o600);
    try {
      await handle.writeFile(body);
      await handle.sync();
      await handle.close();
      await rename(temporary, this.#path);
      const directory = await open(this.directory, constants.O_RDONLY);
      try { await directory.sync(); } finally { await directory.close(); }
    } finally {
      await handle.close().catch(() => {});
      await unlink(temporary).catch((error) => { if (error.code !== 'ENOENT') throw error; });
    }
    this.#receiptsDirty = false;
  }

  pending() { return [...this.#inbound.values()]; }

  async receive(event) {
    const id = key(event.peer_id, event.external_id);
    const previous = this.#inbound.get(id);
    if (previous) {
      if (JSON.stringify(previous) !== JSON.stringify(event)) throw stateError('source_id_reused');
      return previous;
    }
    if (this.#inbound.size >= MAX_INBOUND) throw stateError('inbound_full');
    this.#inbound.set(id, event);
    try { await this.#persist(); }
    catch (error) { this.#inbound.delete(id); throw error; }
    return event;
  }

  async acknowledge(peer, external) {
    const id = key(peer, external);
    const previous = this.#inbound.get(id);
    if (!previous) return;
    this.#inbound.delete(id);
    try { await this.#persist(); }
    catch (error) { this.#inbound.set(id, previous); throw error; }
  }

  async receipt(request, expected) {
    const previous = this.#receipts.get(request);
    if (previous && previous.fingerprint !== expected) {
      throw new AdapterError('request_id_reused', 'Request ID was reused with different content');
    }
    // Provider success remains known even when the local write or directory
    // sync fails. Retry durability before acknowledging, without a second send.
    if (this.#receiptsDirty) await this.#persist();
    return previous;
  }

  ensureReceiptCapacity() {
    if (this.#receipts.size >= MAX_RECEIPTS) throw stateError('receipts_full');
  }

  async recordReceipt(request, expected, external) {
    this.#receipts.set(request, { request_id: request, fingerprint: expected, external_id: external });
    this.#receiptsDirty = true;
    await this.#persist();
  }

  async close() {
    try {
      const owner = JSON.parse(await readPrivate(join(this.#lock, 'owner.json'), 4096));
      if (owner.token !== this.#token) return;
      await unlink(join(this.#lock, 'owner.json'));
      await rmdir(this.#lock);
    } catch (error) { if (error.code !== 'ENOENT') throw stateError(); }
  }
}
