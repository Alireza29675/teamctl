import { AdapterError, encoded } from './protocol.js';

export class ProtocolOutput {
  #output;
  #tail = Promise.resolve();
  #count = 0;
  #bytes = 0;
  #closed = false;
  #failure = null;

  constructor(output, { maxFrames = 64, maxBytes = 2 * 1024 * 1024 } = {}) {
    this.#output = output;
    this.maxFrames = maxFrames;
    this.maxBytes = maxBytes;
    output.on('error', () => { this.#failure = new AdapterError('output_closed', 'Host output closed'); });
  }

  send(frame) {
    const body = encoded(frame);
    const bytes = Buffer.byteLength(body);
    if (this.#closed || this.#failure) return Promise.reject(this.#failure
      ?? new AdapterError('output_closed', 'Host output closed'));
    if (this.#count >= this.maxFrames || this.#bytes + bytes > this.maxBytes) {
      return Promise.reject(new AdapterError('output_backpressure', 'Host output queue is full', true));
    }
    this.#count += 1;
    this.#bytes += bytes;
    const operation = this.#tail.then(() => new Promise((resolve, reject) => {
      if (this.#failure) { reject(this.#failure); return; }
      this.#output.write(body, (error) => {
        if (error) reject(new AdapterError('output_closed', 'Host output closed'));
        else resolve();
      });
    })).finally(() => { this.#count -= 1; this.#bytes -= bytes; });
    this.#tail = operation.catch(() => {});
    return operation;
  }

  async close() {
    this.#closed = true;
    await this.#tail;
  }
}
