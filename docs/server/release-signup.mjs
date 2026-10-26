export const MAX_SIGNUP_BODY_BYTES = 4096;
export const RELEASE_CONSENT_VERSION = 'teamctl-next-release-v1';

const DEFAULT_SEGMENT_ID = 'e9a7afc4-5258-447c-ab1b-797c63e29073';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SIGNUP_ERROR = 'Could not save your signup. Please try again.';
const TURNSTILE_SITEVERIFY_URL = 'https://challenges.cloudflare.com/turnstile/v0/siteverify';
const CHALLENGE_LIFETIME_MS = 300_000;
const CHALLENGE_CLOCK_TOLERANCE_MS = 30_000;

function challengeTime(value) {
  if (typeof value !== 'string'
    || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(value)) return null;
  const timestamp = Date.parse(value);
  const age = Date.now() - timestamp;
  return Number.isFinite(timestamp) && age >= -CHALLENGE_CLOCK_TOLERANCE_MS && age <= CHALLENGE_LIFETIME_MS
    ? timestamp : null;
}

async function verifyTurnstile(token, secret, hostname, signal, fetchImpl) {
  try {
    const response = await fetchImpl(TURNSTILE_SITEVERIFY_URL, {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ secret, response: token }).toString(),
      // Never forward the secret or token to a redirect destination.
      redirect: 'manual',
      signal,
    });
    if (!response.ok) return null;
    const verification = await response.json();
    if (verification?.success !== true || verification.action !== 'release-signup' || verification.hostname !== hostname) return null;
    const timestamp = challengeTime(verification.challenge_ts);
    return timestamp === null ? null : { timestamp };
  } catch {
    return null;
  }
}

async function claimInDatabase(database, tokenHash, expiresAt, now) {
  // The unique key is the claim: concurrent submissions cannot both insert it.
  // D1 batches are transactions, so cleanup and claim commit together.
  const results = await database.batch([
    database.prepare('DELETE FROM release_signup_tokens WHERE expires_at <= ?').bind(now),
    database.prepare('INSERT INTO release_signup_tokens (token_hash, expires_at) VALUES (?, ?) ON CONFLICT(token_hash) DO NOTHING RETURNING token_hash')
      .bind(tokenHash, expiresAt),
  ]);
  if (!Array.isArray(results) || results.length !== 2 || results.some((result) => result?.success !== true)
    || !Array.isArray(results[1].results)) throw new Error('Signup replay store unavailable.');
  const rows = results[1].results;
  if (rows.length === 0) return false;
  if (rows.length !== 1 || rows[0]?.token_hash !== tokenHash) throw new Error('Signup replay store unavailable.');
  return true;
}

function withinDeadline(operation, signal) {
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      signal.removeEventListener('abort', onAbort);
      reject(new DOMException('Signup deadline exceeded.', 'TimeoutError'));
    };
    if (signal.aborted) return onAbort();
    signal.addEventListener('abort', onAbort, { once: true });
    Promise.resolve().then(() => {
      signal.throwIfAborted();
      return operation();
    }).then((value) => {
      signal.removeEventListener('abort', onAbort);
      if (signal.aborted) return onAbort();
      resolve(value);
    }, (error) => {
      signal.removeEventListener('abort', onAbort);
      reject(error);
    });
  });
}

function json(status, body, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      'Content-Type': 'application/json; charset=utf-8',
      'Cache-Control': 'no-store',
      ...headers,
    },
  });
}

function failure(status, error = SIGNUP_ERROR, headers) {
  return json(status, { success: false, error }, headers);
}

function upstreamFailure(response) {
  if (response.status === 429) {
    const retryAfter = Number(response.headers.get('retry-after'));
    const seconds = Number.isFinite(retryAfter) && retryAfter > 0 ? Math.min(Math.ceil(retryAfter), 3600) : 60;
    return failure(429, 'Too many attempts. Please try again shortly.', { 'Retry-After': String(seconds) });
  }
  return failure(502);
}

function validContact(contact, email) {
  return contact?.object === 'contact' && typeof contact.id === 'string' && UUID.test(contact.id)
    && typeof contact.email === 'string' && contact.email.toLowerCase() === email.toLowerCase()
    && typeof contact.unsubscribed === 'boolean';
}

function validEmail(email) {
  if (email.length > 254) return false;
  const parts = email.split('@');
  if (parts.length !== 2) return false;
  const [local, domain] = parts;
  if (!local || local.length > 64 || local.startsWith('.') || local.endsWith('.') || local.includes('..')) return false;
  if (!/^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+$/.test(local)) return false;
  const labels = domain.split('.');
  return labels.length > 1 && labels.every((label) => /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(label));
}

async function readBody(request) {
  const declaredLength = request.headers.get('content-length');
  if (declaredLength && Number(declaredLength) > MAX_SIGNUP_BODY_BYTES) return null;
  if (!request.body) return '';

  const reader = request.body.getReader();
  const chunks = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > MAX_SIGNUP_BODY_BYTES) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  const body = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(body);
}

export async function handleReleaseSignup(request, env = {}, { fetchImpl = fetch, claimToken } = {}) {
  if (request.method !== 'POST') return failure(405, SIGNUP_ERROR, { Allow: 'POST' });
  if (request.headers.get('origin') !== new URL(request.url).origin) return failure(403);
  if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') return failure(415);

  let body;
  try {
    const text = await readBody(request);
    if (text === null) return failure(413);
    body = JSON.parse(text);
  } catch {
    return failure(400);
  }

  if (!body || typeof body !== 'object' || Array.isArray(body)) return failure(400);
  if (body.website !== undefined && body.website !== '') return failure(400);
  if (body.consentVersion !== RELEASE_CONSENT_VERSION) return failure(400);
  const email = typeof body.email === 'string' ? body.email.trim() : '';
  if (!validEmail(email)) return failure(400, 'Enter a valid email address.');

  const apiKey = typeof env.RESEND_API_KEY === 'string' ? env.RESEND_API_KEY.trim() : '';
  const segmentId = env.RESEND_SEGMENT_ID ?? DEFAULT_SEGMENT_ID;
  const turnstileSecret = typeof env.TURNSTILE_SECRET === 'string' ? env.TURNSTILE_SECRET.trim() : '';
  if (!apiKey || typeof segmentId !== 'string' || !UUID.test(segmentId) || !turnstileSecret) return failure(503);
  const turnstileToken = typeof body.turnstileToken === 'string' ? body.turnstileToken.trim() : '';
  if (!turnstileToken || turnstileToken.length > 2048) return failure(403);
  const replayDatabase = env.SIGNUP_REPLAY_DB;
  if (typeof claimToken !== 'function'
    && (typeof replayDatabase?.prepare !== 'function' || typeof replayDatabase?.batch !== 'function')) return failure(503);

  try {
    const signal = AbortSignal.timeout(8000);
    const verification = await verifyTurnstile(turnstileToken, turnstileSecret, new URL(request.url).hostname, signal, fetchImpl);
    if (!verification) return failure(403);
    let claimed;
    try {
      const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(turnstileToken));
      const tokenHash = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
      const now = Date.now();
      // Retain a future-dated challenge through its allowed clock tolerance.
      const expiresAt = Math.ceil((Math.max(now, verification.timestamp) + CHALLENGE_LIFETIME_MS) / 1000);
      const claim = claimToken ?? ((...args) => claimInDatabase(replayDatabase, ...args));
      claimed = await withinDeadline(() => claim(tokenHash, expiresAt, Math.floor(now / 1000)), signal);
      if (typeof claimed !== 'boolean') throw new Error('Signup replay store unavailable.');
    } catch {
      return failure(503);
    }
    if (!claimed) return failure(403);
    if (signal.aborted) return failure(503);
    const resend = (path, options = {}) => fetchImpl(`https://api.resend.com${path}`, {
      method: 'GET',
      ...options,
      headers: { Authorization: `Bearer ${apiKey}`, 'Content-Type': 'application/json' },
      // Workerd supports manual/follow; reject 3xx below without forwarding the key.
      redirect: 'manual',
      signal,
    });
    const lookupPath = `/contacts/${encodeURIComponent(email)}`;
    let response = await resend(lookupPath);
    let contact;
    if (response.status === 404) {
      response = await resend('/contacts', {
        method: 'POST',
        body: JSON.stringify({ email, segments: [{ id: segmentId }] }),
      });
      if (response.ok) {
        const created = await response.json();
        if (created?.object !== 'contact' || typeof created.id !== 'string' || !UUID.test(created.id)) return failure(502);
        response = await resend(`/contacts/${created.id}`);
      } else if (response.status === 409 || response.status === 422) {
        // A concurrent signup can create the same contact after the first lookup.
        response = await resend(lookupPath);
      } else {
        return upstreamFailure(response);
      }
    }
    if (!response.ok) return upstreamFailure(response);
    contact = await response.json();
    if (!validContact(contact, email)) return failure(502);
    if (contact.unsubscribed) return failure(409);

    let after;
    for (let page = 0; page < 5; page++) {
      const query = new URLSearchParams({ limit: '100', ...(after ? { after } : {}) });
      response = await resend(`/contacts/${contact.id}/segments?${query}`);
      if (!response.ok) return upstreamFailure(response);
      const segments = await response.json();
      if (segments?.object !== 'list' || !Array.isArray(segments.data) || typeof segments.has_more !== 'boolean'
        || !segments.data.every((segment) => typeof segment?.id === 'string' && UUID.test(segment.id))) return failure(502);
      if (segments.data.some((segment) => segment.id === segmentId)) return json(200, { success: true });
      if (!segments.has_more) break;
      const cursor = segments.data.at(-1)?.id;
      if (!cursor || cursor === after || page === 4) return failure(502);
      after = cursor;
    }

    response = await resend(`/contacts/${contact.id}/segments/${segmentId}`, { method: 'POST' });
    if (!response.ok) return upstreamFailure(response);
    const membership = await response.json();
    if (membership?.id !== segmentId) return failure(502);
    return json(200, { success: true });
  } catch {
    return failure(502);
  }
}
