import { ProtocolOutput } from './output.js';
import { DurableState, fingerprint } from './state.js';
import { AdapterError, inbound, identity, text, readFrames, connectionState, invalid } from './protocol.js';

export { AdapterError, MAX_FRAME_BYTES, MAX_TEXT_BYTES } from './protocol.js';

function timeout(operation, milliseconds) {
  let timer;
  return Promise.race([operation, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new AdapterError('transport_timeout', 'Transport operation timed out', true)),
      milliseconds);
  })]).finally(() => clearTimeout(timer));
}

export class AdapterSession {
  #factory;
  #output;
  #diagnostics;
  #state = null;
  #transport = null;
  #initialization = null;
  #ready = false;
  #closed = false;
  #closePromise;
  #tail = Promise.resolve();
  #queued = 0;
  #diagnosticCount = 0;
  #input = null;
  #controller = new AbortController();

  constructor({ factory, output = process.stdout, diagnostics = process.stderr,
    displayName, capabilities = ['text'], operationTimeoutMs = 30_000, shutdownTimeoutMs = 5_000 }) {
    identity(displayName);
    if (!Array.isArray(capabilities) || capabilities.length > 32) throw invalid();
    capabilities.forEach(identity);
    this.#factory = factory;
    this.#output = new ProtocolOutput(output);
    this.#diagnostics = diagnostics;
    this.displayName = displayName;
    this.capabilities = [...capabilities];
    this.operationTimeoutMs = operationTimeoutMs;
    this.shutdownTimeoutMs = shutdownTimeoutMs;
    this.exitCode = 0;
  }

  #enqueue(operation) {
    if (this.#closed) return Promise.reject(new AdapterError('adapter_closed', 'Adapter is closed'));
    if (this.#queued >= 64) return Promise.reject(new AdapterError('adapter_busy', 'Adapter queue is full', true));
    this.#queued += 1;
    const result = this.#tail.then(operation).finally(() => { this.#queued -= 1; });
    this.#tail = result.catch(() => {});
    return result;
  }

  #diagnose(code) {
    // Diagnostics contain only controlled codes, never transport errors or payloads.
    const safe = /^[a-z_]{1,64}$/u.test(code) ? code : 'transport_failed';
    if (this.#diagnosticCount < 128) {
      this.#diagnosticCount += 1;
      this.#diagnostics.write(`adapter: ${safe}\n`);
    }
  }

  async #error(error, request) {
    const known = error instanceof AdapterError;
    const frame = { type: 'error', code: known ? error.code : 'transport_failed',
      message: known ? error.message : 'Transport operation failed', retryable: known ? error.retryable : true };
    if (request) frame.request_id = request;
    await this.#output.send(frame);
    this.#diagnose(frame.code);
  }

  #context() {
    return {
      signal: this.#controller.signal,
      ready: () => this.#enqueue(async () => {
        if (this.#ready) return;
        this.#ready = true;
        await this.#output.send({ type: 'ready', version: 1,
          capabilities: this.capabilities, display_name: this.displayName });
        if (this.#initialization.mode === 'run') {
          for (const event of this.#state.pending()) await this.#output.send(event);
        }
      }),
      receive: (value) => this.#enqueue(async () => {
        if (this.#initialization.mode !== 'run') return;
        const event = await this.#state.receive(inbound(value));
        if (this.#ready) await this.#output.send(event);
      }),
      connection: (state) => this.#output.send({ type: 'connection', state: connectionState(state) }),
      pairing: (qr) => {
        if (this.#initialization.mode !== 'pair') {
          return this.#output.send({ type: 'connection', state: 'pairing_required' });
        }
        text(qr);
        return this.#output.send({ type: 'pairing', qr });
      },
      fail: async (code) => {
        this.exitCode = 1;
        await this.#error(new AdapterError(code, 'Transport cannot continue'));
        await this.close();
      },
    };
  }

  async handle(frame) {
    if (frame.type === 'shutdown') { await this.close(); return; }
    if (frame.type === 'initialize') {
      if (this.#initialization) throw new AdapterError('protocol_error', 'Adapter is already initialized');
      this.#initialization = frame;
      this.#state = await DurableState.open(frame.state_dir, frame.instance_id);
      const creating = Promise.resolve(this.#factory(frame, this.#context()));
      let disposed = false;
      const disposeLate = async (transport) => {
        if (this.#closed && !disposed && typeof transport?.close === 'function') {
          disposed = true;
          await timeout(Promise.resolve(transport.close()), this.shutdownTimeoutMs);
        }
      };
      creating.then(disposeLate).catch(() => {});
      this.#transport = await timeout(creating, this.operationTimeoutMs);
      if (this.#closed) { await disposeLate(this.#transport); return; }
      if (!this.#transport || typeof this.#transport.send !== 'function'
        || typeof this.#transport.close !== 'function') throw new AdapterError('invalid_transport', 'Invalid transport implementation');
      if (this.#transport.start) {
        Promise.resolve().then(() => this.#transport.start()).catch(async () => {
          if (!this.#closed) await this.#context().fail('transport_start_failed');
        }).catch(() => {});
      }
      return;
    }
    if (!this.#state) throw new AdapterError('protocol_error', 'Adapter must be initialized first');
    if (frame.type === 'accepted' || frame.type === 'rejected') {
      await this.#enqueue(() => this.#state.acknowledge(frame.peer_id, frame.external_id));
      return;
    }
    if (frame.type === 'send') {
      try {
        const expected = fingerprint(frame.peer_id, frame.message);
        const previous = await this.#enqueue(async () => {
          if (this.#initialization.mode !== 'run') {
            throw new AdapterError('pair_mode', 'Messages cannot be sent while pairing');
          }
          const previous = await this.#state.receipt(frame.request_id, expected);
          if (previous) return previous;
          if (!this.#ready) throw new AdapterError('not_ready', 'Transport is not connected', true);
          if (!this.capabilities.includes(frame.message.kind)) {
            throw new AdapterError('unsupported_kind', 'This transport does not support the requested message kind');
          }
          this.#state.ensureReceiptCapacity();
          return null;
        });
        if (previous) {
          await this.#output.send({ type: 'sent', request_id: frame.request_id, external_id: previous.external_id });
          return;
        }
        // Provider calls run outside the state queue: a transport may receive a new
        // inbound event while an outgoing request is in flight.
        const delivered = await timeout(Promise.resolve(this.#transport.send(frame)), this.operationTimeoutMs);
        const external = delivered == null ? null : identity(delivered);
        await this.#enqueue(async () => {
          // Save success before informing the host, so host replay after restart is idempotent.
          await this.#state.recordReceipt(frame.request_id, expected, external);
          await this.#output.send({ type: 'sent', request_id: frame.request_id, external_id: external });
        });
      } catch (error) {
        await this.#error(error, frame.request_id);
        if (error.code === 'transport_timeout') { this.exitCode = 1; await this.close(); }
      }
    }
  }

  async run(input = process.stdin) {
    this.#input = input;
    try {
      for await (const frame of readFrames(input)) {
        if (this.#closed) break;
        await this.handle(frame);
        if (this.#closed) break;
      }
    } catch (error) {
      if (!this.#closed) {
        this.exitCode = 1;
        await this.#error(error).catch(() => {});
      }
    } finally {
      await this.close();
    }
    return this.exitCode;
  }

  close() {
    if (this.#closePromise) return this.#closePromise;
    this.#closed = true;
    this.#controller.abort();
    this.#input?.destroy();
    this.#closePromise = (async () => {
      if (this.#transport) {
        try { await timeout(Promise.resolve(this.#transport.close()), this.shutdownTimeoutMs); }
        catch { this.#diagnose('shutdown_timeout'); this.exitCode = 1; }
      }
      // Pending writes finish before releasing the single-owner state lock.
      let drained = true;
      try { await timeout(this.#tail, this.shutdownTimeoutMs); }
      catch { drained = false; this.#diagnose('shutdown_timeout'); this.exitCode = 1; }
      if (this.#state && drained) await this.#state.close();
      await timeout(this.#output.close(), this.shutdownTimeoutMs).catch(() => { this.exitCode = 1; });
    })();
    return this.#closePromise;
  }
}

export async function runAdapter(options) {
  const session = new AdapterSession(options);
  const stop = () => {
    session.close().catch(() => {});
    (options.input ?? process.stdin).destroy();
  };
  process.once('SIGTERM', stop);
  process.once('SIGINT', stop);
  try { return await session.run(options.input ?? process.stdin); }
  finally { process.removeListener('SIGTERM', stop); process.removeListener('SIGINT', stop); }
}
