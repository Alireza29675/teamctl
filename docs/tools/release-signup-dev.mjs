import { readFile } from 'node:fs/promises';
import { parseEnv } from 'node:util';
import { handleReleaseSignup, MAX_SIGNUP_BODY_BYTES } from '../server/release-signup.mjs';

const ENV_FILE = new URL('../.env', import.meta.url);
const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]']);

// Local Vite is one process. Production uses the shared atomic D1 store instead.
export function createLocalReplayStore(maxEntries = 4096) {
  const claims = new Map();
  return (tokenHash, expiresAt, now) => {
    for (const [hash, expiry] of claims) {
      if (expiry <= now) claims.delete(hash);
    }
    if (claims.has(tokenHash)) return false;
    if (claims.size >= maxEntries) throw new Error('Local signup replay store is full.');
    claims.set(tokenHash, expiresAt);
    return true;
  };
}

async function signupEnv() {
  let local = {};
  try {
    local = parseEnv(await readFile(ENV_FILE, 'utf8'));
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  return {
    RESEND_API_KEY: process.env.RESEND_API_KEY ?? local.RESEND_API_KEY,
    RESEND_SEGMENT_ID: process.env.RESEND_SEGMENT_ID ?? local.RESEND_SEGMENT_ID,
    TURNSTILE_SECRET: process.env.TURNSTILE_SECRET ?? local.TURNSTILE_SECRET,
  };
}

function readNodeBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let length = 0;
    const timer = setTimeout(() => fail(408), 5000);

    function cleanup() {
      clearTimeout(timer);
      request.off('data', onData);
      request.off('end', onEnd);
      request.off('error', onError);
      request.off('aborted', onError);
    }
    function fail(status) {
      cleanup();
      request.once('error', () => {});
      request.resume();
      reject(Object.assign(new Error('Invalid signup request.'), { status }));
    }
    function onData(chunk) {
      length += chunk.length;
      if (length > MAX_SIGNUP_BODY_BYTES) return fail(413);
      chunks.push(chunk);
    }
    function onEnd() {
      cleanup();
      resolve(Buffer.concat(chunks, length));
    }
    function onError() {
      fail(400);
    }

    if (Number(request.headers['content-length']) > MAX_SIGNUP_BODY_BYTES) return fail(413);
    request.on('data', onData);
    request.on('end', onEnd);
    request.on('error', onError);
    request.on('aborted', onError);
  });
}

function safeError(status) {
  return new Response(JSON.stringify({ success: false, error: 'Could not save your signup. Please try again.' }), {
    status,
    headers: { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' },
  });
}

export default function releaseSignupDevPlugin() {
  const claimToken = createLocalReplayStore();
  return {
    name: 'teamctl-release-signup',
    configureServer(server) {
      server.middlewares.use(async (request, response, next) => {
        if (request.url?.split('?')[0] !== '/api/release-signup') return next();

        let result;
        try {
          const headers = new Headers();
          for (const [name, value] of Object.entries(request.headers)) {
            if (value !== undefined) headers.set(name, Array.isArray(value) ? value.join(', ') : value);
          }
          const protocol = request.socket.encrypted ? 'https' : 'http';
          const url = new URL(request.url, `${protocol}://${request.headers.host}`);
          if (!LOOPBACK_HOSTS.has(url.hostname)) {
            result = safeError(403);
          } else {
            const body = request.method === 'POST' ? await readNodeBody(request) : undefined;
            const webRequest = new Request(url, { method: request.method, headers, body });
            result = await handleReleaseSignup(webRequest, request.method === 'POST' ? await signupEnv() : {}, { claimToken });
          }
        } catch (error) {
          result = safeError(error.status ?? 503);
        }

        if (response.destroyed || response.writableEnded) return;
        response.statusCode = result.status;
        result.headers.forEach((value, name) => response.setHeader(name, value));
        response.end(await result.text());
      });
    },
  };
}
