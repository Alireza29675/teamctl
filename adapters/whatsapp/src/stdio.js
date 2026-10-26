import { Writable } from 'node:stream';

export function isolatedIO() {
  const protocolWrite = process.stdout.write.bind(process.stdout);
  const diagnosticWrite = process.stderr.write.bind(process.stderr);
  const output = new Writable({ write(chunk, encoding, callback) { protocolWrite(chunk, encoding, callback); } });
  const diagnostics = new Writable({ write(chunk, encoding, callback) { diagnosticWrite(chunk, encoding, callback); } });
  // Browser libraries sometimes print raw errors. A QR is an authentication secret;
  // only controlled SDK output may leave this process.
  const discard = (_chunk, encoding, callback) => {
    const done = typeof encoding === 'function' ? encoding : callback;
    if (done) queueMicrotask(done);
    return true;
  };
  process.stdout.write = discard;
  process.stderr.write = discard;
  for (const name of ['log', 'info', 'warn', 'error', 'debug', 'trace']) console[name] = () => {};
  return { output, diagnostics };
}
