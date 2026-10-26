import { setTimeout as delay } from 'node:timers/promises';

function alive(child) {
  return child && child.exitCode === null && child.signalCode === null;
}

function signalOwned(child, signal) {
  if (!alive(child) || !Number.isSafeInteger(child.pid) || child.pid <= 0 || child.pid === process.pid) return;
  // Puppeteer's Unix browser launcher gives Chromium its own process group.
  // Signal that group so browser descendants cannot survive a stalled destroy.
  if (process.platform !== 'win32') {
    try { process.kill(-child.pid, signal); } catch (error) { if (error.code !== 'ESRCH') throw error; }
  }
  if (alive(child)) child.kill(signal);
}

export async function closeClient(client, { destroyTimeoutMs = 2_000, reapTimeoutMs = 500 } = {}) {
  if (!client) return;
  const browser = client.pupBrowser;
  const child = browser?.process?.();
  let timer;
  client.removeAllListeners();
  try {
    await Promise.race([Promise.resolve().then(async () => {
      // WhatsApp1.34.7 checks the removed Browser.isConnected method in destroy.
      // Closing through the public Puppeteer25 API first preserves graceful cleanup.
      try { await browser?.close?.(); } finally { await client.destroy(); }
    }), new Promise((resolve) => {
      timer = setTimeout(resolve, destroyTimeoutMs);
    })]);
  } catch { /* Browser termination below also handles a rejected library shutdown. */ }
  finally { clearTimeout(timer); }
  if (!alive(child)) return;
  const exited = new Promise((resolve) => child.once('exit', resolve));
  signalOwned(child, 'SIGTERM');
  await Promise.race([exited, delay(reapTimeoutMs)]);
  if (alive(child)) {
    signalOwned(child, 'SIGKILL');
    await Promise.race([exited, delay(reapTimeoutMs)]);
  }
}
