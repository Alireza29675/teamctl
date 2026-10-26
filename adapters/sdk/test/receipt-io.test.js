import test from 'node:test';
import assert from 'node:assert/strict';
import { open, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fixture, send } from './helpers.js';

for (const stage of ['write', 'file-sync', 'directory-sync']) {
  test(`known provider success survives ${stage} failure without a second send`, async (t) => {
    const f = await fixture(t);
    const probe = await open(join(f.directory, 'probe'), 'wx', 0o600);
    const prototype = Object.getPrototypeOf(probe);
    await probe.close();
    const method = stage === 'write' ? 'writeFile' : 'sync';
    const original = prototype[method];
    let fail = true;
    t.mock.method(prototype, method, async function (...args) {
      const directory = (await this.stat()).isDirectory();
      if (fail && (stage === 'directory-sync' ? directory : !directory)) {
        throw Object.assign(new Error('injected storage failure'), { code: 'EIO' });
      }
      return original.apply(this, args);
    });
    await f.session.handle(send());
    assert.equal(f.sends(), 1);
    assert.equal(f.output.frames().filter((frame) => frame.type === 'sent').length, 0);
    await f.session.handle(send());
    assert.equal(f.sends(), 1, 'retry must retry storage, rather than the provider');
    await f.session.handle(send({ request_id: 'delivery:2' }));
    assert.equal(f.sends(), 1, 'new sends must wait for the known receipt to become durable');
    await f.session.handle(send({ message: { text: 'changed', kind: 'text' } }));
    assert.equal(f.sends(), 1);
    assert.equal(f.output.frames().at(-1).code, 'request_id_reused');
    fail = false;
    await f.session.handle(send());
    assert.equal(f.sends(), 1);
    assert.deepEqual(f.output.frames().at(-1), {
      type: 'sent', request_id: 'delivery:1', external_id: 'provider:1',
    });
    const state = JSON.parse(await readFile(join(f.directory, 'adapter-state.json'), 'utf8'));
    assert.equal(state.receipts.length, 1);
    assert.equal(state.receipts[0].external_id, 'provider:1');
    await f.session.handle(send({ request_id: 'delivery:2' }));
    assert.equal(f.sends(), 2, 'new sends resume after storage recovers');
    const recovered = JSON.parse(await readFile(join(f.directory, 'adapter-state.json'), 'utf8'));
    assert.equal(recovered.receipts.length, 2);
  });
}
