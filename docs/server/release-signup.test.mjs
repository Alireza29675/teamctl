import assert from 'node:assert/strict';
import test from 'node:test';
import { Readable } from 'node:stream';
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { handleReleaseSignup, MAX_SIGNUP_BODY_BYTES, RELEASE_CONSENT_VERSION } from './release-signup.mjs';
import releaseSignupDevPlugin, { createLocalReplayStore } from '../tools/release-signup-dev.mjs';
import { onRequest } from '../../functions/api/release-signup.js';

const ORIGIN = 'http://127.0.0.1:4323';
const ENV = { RESEND_API_KEY: 'test-server-key', TURNSTILE_SECRET: 'test-turnstile-secret' };
const SITEVERIFY_URL = 'https://challenges.cloudflare.com/turnstile/v0/siteverify';
const CONTACT_ID = '479e3145-dd38-476b-932c-529ceb705947';
const DEFAULT_SEGMENT_ID = 'e9a7afc4-5258-447c-ab1b-797c63e29073';

function signupRequest(body = { email: 'person@example.com', website: '' }, options = {}) {
  const headers = new Headers({ Origin: ORIGIN, 'Content-Type': 'application/json', ...options.headers });
  if (options.omitOrigin) headers.delete('Origin');
  return new Request(`${ORIGIN}/api/release-signup`, {
    method: options.method ?? 'POST',
    headers,
    ...(options.method === 'GET' ? {} : {
      body: typeof body === 'string' ? body : JSON.stringify(body && !Array.isArray(body) ? { consentVersion: RELEASE_CONSENT_VERSION, turnstileToken: 'test-turnstile-token', ...body } : body),
    }),
  });
}

function neverFetch() {
  assert.fail('Invalid requests must not reach external services.');
}

// Test fixtures can supply an isolated successful claim without pretending to be D1.
function signupWithClaim(request, env, options = {}) {
  return handleReleaseSignup(request, env, { claimToken: () => true, ...options });
}

function turnstileVerified(hostname = '127.0.0.1') {
  return Response.json({ success: true, action: 'release-signup', hostname, challenge_ts: new Date().toISOString() });
}

// Keep the existing Resend behavior tests focused on their contact flows.
// The real Siteverify boundary is exercised independently below.
function verifiedSignup(request, env, { fetchImpl = fetch } = {}) {
  return signupWithClaim(request, env, {
    fetchImpl: (url, options) => {
      if (url === SITEVERIFY_URL && fetchImpl !== neverFetch) {
        return turnstileVerified(new URL(request.url).hostname);
      }
      return fetchImpl(url, options);
    },
  });
}

function confirmedContact() {
  return Response.json({ object: 'contact', id: CONTACT_ID }, { status: 201 });
}

function contactRecord(email = 'person@example.com', unsubscribed = false) {
  return Response.json({ object: 'contact', id: CONTACT_ID, email, unsubscribed });
}

function segmentList(ids = [DEFAULT_SEGMENT_ID], hasMore = false) {
  return Response.json({ object: 'list', data: ids.map((id) => ({ id })), has_more: hasMore });
}

test('only accepts POST requests from the same origin', async () => {
  const cases = [
    [signupRequest(undefined, { method: 'GET' }), 405],
    [signupRequest(undefined, { omitOrigin: true }), 403],
    [signupRequest(undefined, { headers: { Origin: 'https://another.example' } }), 403],
    [signupRequest(undefined, { headers: { Origin: 'null' } }), 403],
  ];
  for (const [request, status] of cases) {
    const response = await verifiedSignup(request, ENV, { fetchImpl: neverFetch });
    assert.equal(response.status, status);
    assert.equal((await response.json()).success, false);
    assert.equal(response.headers.get('Cache-Control'), 'no-store');
    assert.equal(response.headers.get('Access-Control-Allow-Origin'), null);
    if (status === 405) assert.equal(response.headers.get('Allow'), 'POST');
  }
});

test('requires a JSON object and rejects malformed or non-JSON bodies', async () => {
  const cases = [
    [signupRequest(undefined, { headers: { 'Content-Type': 'text/plain' } }), 415],
    [signupRequest('{'), 400],
    [signupRequest('null'), 400],
    [signupRequest([]), 400],
  ];
  for (const [request, status] of cases) {
    assert.equal((await verifiedSignup(request, ENV, { fetchImpl: neverFetch })).status, status);
  }
});

test('enforces the body limit from the actual stream as well as content length', async () => {
  const oversized = JSON.stringify({ email: 'person@example.com', padding: 'x'.repeat(MAX_SIGNUP_BODY_BYTES) });
  const actual = await verifiedSignup(signupRequest(oversized), ENV, { fetchImpl: neverFetch });
  assert.equal(actual.status, 413);

  const declared = signupRequest(undefined, { headers: { 'Content-Length': String(MAX_SIGNUP_BODY_BYTES + 1) } });
  assert.equal((await verifiedSignup(declared, ENV, { fetchImpl: neverFetch })).status, 413);

  const multibyte = JSON.stringify({ email: 'person@example.com', padding: 'é'.repeat(MAX_SIGNUP_BODY_BYTES / 2) });
  assert.ok(multibyte.length < MAX_SIGNUP_BODY_BYTES);
  assert.equal((await verifiedSignup(signupRequest(multibyte), ENV, { fetchImpl: neverFetch })).status, 413);
});

test('rejects a filled or invalid honeypot without reporting success', async () => {
  for (const website of ['https://spam.example', ' ', true, null]) {
    const response = await verifiedSignup(signupRequest({ email: 'person@example.com', website }), ENV, { fetchImpl: neverFetch });
    assert.equal(response.status, 400);
    assert.equal((await response.json()).success, false);
  }
});

test('requires the current release-only consent version', async () => {
  for (const consentVersion of [undefined, null, '', 'all-marketing', 'teamctl-next-release-v0']) {
    const response = await verifiedSignup(signupRequest(JSON.stringify({ email: 'person@example.com', consentVersion })), ENV, { fetchImpl: neverFetch });
    assert.equal(response.status, 400);
  }
});

test('rejects invalid emails before any external request', async () => {
  const emails = [
    '', 'person', 'person@localhost', '@example.com', 'person@@example.com',
    'two people@example.com', 'person@example..com', '.person@example.com',
    'person..name@example.com', 'person@-example.com', 'person@example-.com',
    'x'.repeat(65) + '@example.com',
    'p'.repeat(64) + '@' + 'd'.repeat(63) + '.' + 'd'.repeat(63) + '.' + 'd'.repeat(63),
    null, 123,
  ];
  for (const email of emails) {
    const response = await verifiedSignup(signupRequest({ email }), ENV, { fetchImpl: neverFetch });
    assert.equal(response.status, 400, `Expected invalid email to be rejected: ${String(email)}`);
  }
});

test('returns 503 when the private key or segment configuration is unavailable', async () => {
  for (const env of [{}, { RESEND_API_KEY: ' ' }, { ...ENV, RESEND_SEGMENT_ID: '' }, { ...ENV, RESEND_SEGMENT_ID: 'invalid' }]) {
    const response = await verifiedSignup(signupRequest(), env, { fetchImpl: neverFetch });
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('requires a server-only Turnstile secret before external requests', async () => {
  for (const secret of [undefined, null, '', ' ', 123]) {
    const response = await signupWithClaim(signupRequest(), { ...ENV, TURNSTILE_SECRET: secret }, { fetchImpl: neverFetch });
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('rejects missing, blank, non-string or oversized Turnstile tokens before verification', async () => {
  for (const turnstileToken of [undefined, null, '', ' ', 123, {}, [], 'x'.repeat(2049)]) {
    const response = await signupWithClaim(signupRequest({ email: 'person@example.com', turnstileToken }), ENV, { fetchImpl: neverFetch });
    assert.equal(response.status, 403);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('requires exact successful Siteverify action and hostname before contacting Resend', async () => {
  const results = [
    { success: false, 'error-codes': ['timeout-or-duplicate'] },
    { success: 'true', action: 'release-signup', hostname: '127.0.0.1' },
    { success: true, action: 'another-form', hostname: '127.0.0.1' },
    { success: true, action: 'release-signup', hostname: 'another.example' },
    { success: true, action: 'release-signup', hostname: '127.0.0.1.attacker.example' },
    { success: true, action: 'release-signup' },
    { success: true, hostname: '127.0.0.1' },
    null,
  ];
  for (const result of results) {
    let calls = 0;
    const response = await signupWithClaim(signupRequest(), ENV, {
      claimToken: () => assert.fail('Rejected verification metadata must never reach the replay store.'),
      fetchImpl: async (url) => {
        calls++;
        assert.equal(url, SITEVERIFY_URL, 'A rejected challenge must never reach Resend.');
        return Response.json(result);
      },
    });
    assert.equal(calls, 1);
    assert.equal(response.status, 403);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('fails closed for malformed Siteverify JSON, non-success statuses and redirects', async () => {
  const responses = [
    () => new Response('private upstream detail', { status: 200 }),
    ...[204, 301, 302, 307, 308, 400, 429, 500].map((status) => () => new Response(null, {
      status, headers: { Location: 'https://another.example/private' },
    })),
  ];
  for (const makeResponse of responses) {
    let calls = 0;
    const response = await signupWithClaim(signupRequest(), ENV, {
      fetchImpl: async (url, options) => {
        calls++;
        assert.equal(url, SITEVERIFY_URL);
        assert.equal(options.redirect, 'manual');
        return makeResponse();
      },
    });
    assert.equal(calls, 1);
    assert.equal(response.status, 403);
    assert.equal(response.headers.get('Location'), null);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('fails closed for Siteverify network errors and timeouts without leaking details', async () => {
  for (const error of [new Error('private verification detail'), new DOMException('Timed out', 'TimeoutError')]) {
    let calls = 0;
    const response = await signupWithClaim(signupRequest(), ENV, {
      fetchImpl: async (url) => {
        calls++;
        assert.equal(url, SITEVERIFY_URL);
        throw error;
      },
    });
    assert.equal(calls, 1);
    assert.equal(response.status, 403);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('validates the largest token before Resend using one shared eight-second deadline', async (t) => {
  const signal = new AbortController().signal;
  let deadlineCalls = 0;
  t.mock.method(AbortSignal, 'timeout', (milliseconds) => {
    deadlineCalls++;
    assert.equal(milliseconds, 8000);
    return signal;
  });
  const token = 'x'.repeat(2048);
  const calls = [];
  const response = await signupWithClaim(signupRequest({ email: 'person@example.com', turnstileToken: ` ${token} ` }), {
    ...ENV, TURNSTILE_SECRET: ' turnstile&secret=test ',
  }, {
    fetchImpl: async (url, options) => {
      calls.push(url);
      assert.equal(options.signal, signal);
      if (url === SITEVERIFY_URL) {
        assert.equal(calls.length, 1, 'Verification happens before every contact operation.');
        assert.equal(options.method, 'POST');
        assert.equal(options.redirect, 'manual');
        assert.deepEqual(options.headers, { 'Content-Type': 'application/x-www-form-urlencoded' });
        assert.deepEqual([...new URLSearchParams(options.body)], [
          ['secret', 'turnstile&secret=test'], ['response', token],
        ]);
        return turnstileVerified();
      }
      assert.equal(options.headers.Authorization, 'Bearer test-server-key');
      return url.includes('/segments?') ? segmentList() : contactRecord();
    },
  });
  assert.equal(response.status, 200);
  assert.equal(deadlineCalls, 1);
  assert.equal(calls.length, 3);
});

test('a replayed or expired challenge cannot repeat a previously verified contact flow', async () => {
  let verifications = 0;
  let contactCalls = 0;
  const fetchImpl = async (url) => {
    if (url === SITEVERIFY_URL) {
      verifications++;
      return verifications === 1 ? turnstileVerified() : Response.json({ success: false, 'error-codes': ['timeout-or-duplicate'] });
    }
    contactCalls++;
    return url.includes('/segments?') ? segmentList() : contactRecord();
  };
  assert.equal((await signupWithClaim(signupRequest(), ENV, { fetchImpl })).status, 200);
  assert.equal(contactCalls, 2);
  assert.equal((await signupWithClaim(signupRequest(), ENV, { fetchImpl })).status, 403);
  assert.equal(verifications, 2);
  assert.equal(contactCalls, 2);
});

test('requires shared replay storage by default and never falls back to isolate memory', async () => {
  for (const database of [undefined, null, {}, { prepare() {} }, { batch() {} }]) {
    const response = await handleReleaseSignup(signupRequest(), { ...ENV, SIGNUP_REPLAY_DB: database }, { fetchImpl: neverFetch });
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('rejects missing, malformed, expired or excessively future-dated challenges before claiming', async () => {
  const now = Date.now();
  const timestamps = [undefined, null, '', 123, 'today', new Date(now - 301_000).toISOString(), new Date(now + 31_000).toISOString()];
  for (const challenge_ts of timestamps) {
    let calls = 0;
    const response = await signupWithClaim(signupRequest(), ENV, {
      claimToken: () => assert.fail('Unverified or stale challenges must never be claimed.'),
      fetchImpl: async (url) => {
        calls++;
        assert.equal(url, SITEVERIFY_URL);
        return Response.json({ success: true, action: 'release-signup', hostname: '127.0.0.1', challenge_ts });
      },
    });
    assert.equal(calls, 1);
    assert.equal(response.status, 403);
  }
});

test('claims only a SHA-256 hash after successful verification and before Resend', async () => {
  const order = [];
  const now = Date.now();
  const token = 'test-turnstile-token';
  const response = await signupWithClaim(signupRequest(), ENV, {
    claimToken: (hash, expiresAt, acceptedAt) => {
      order.push('claim');
      assert.equal(hash, createHash('sha256').update(token).digest('hex'));
      assert.ok(Number.isInteger(acceptedAt));
      assert.ok(acceptedAt >= Math.floor(now / 1000));
      assert.ok(expiresAt >= acceptedAt + 300 && expiresAt <= acceptedAt + 301);
      return true;
    },
    fetchImpl: async (url) => {
      if (url === SITEVERIFY_URL) {
        order.push('verify');
        return turnstileVerified();
      }
      order.push('resend');
      return url.includes('/segments?') ? segmentList() : contactRecord();
    },
  });
  assert.equal(response.status, 200);
  assert.deepEqual(order, ['verify', 'claim', 'resend', 'resend']);
});

test('an accepted future challenge remains claimed through the entire clock tolerance window', async () => {
  const future = Date.now() + 29_000;
  const response = await signupWithClaim(signupRequest(), ENV, {
    claimToken: (_hash, expiresAt) => {
      assert.ok(expiresAt * 1000 >= future + 300_000);
      return true;
    },
    fetchImpl: async (url) => {
      if (url === SITEVERIFY_URL) {
        return Response.json({ success: true, action: 'release-signup', hostname: '127.0.0.1', challenge_ts: new Date(future).toISOString() });
      }
      return url.includes('/segments?') ? segmentList() : contactRecord();
    },
  });
  assert.equal(response.status, 200);
});

test('concurrent submissions with a repeatedly valid provider token allow only one contact flow', async () => {
  const claimed = new Set();
  let verifications = 0;
  let contactCalls = 0;
  const options = {
    claimToken: (hash) => {
      if (claimed.has(hash)) return false;
      claimed.add(hash);
      return true;
    },
    fetchImpl: async (url) => {
      if (url === SITEVERIFY_URL) {
        verifications++;
        return turnstileVerified();
      }
      contactCalls++;
      return url.includes('/segments?') ? segmentList() : contactRecord();
    },
  };
  const responses = await Promise.all([
    signupWithClaim(signupRequest(), ENV, options),
    signupWithClaim(signupRequest(), ENV, options),
  ]);
  assert.deepEqual(responses.map((response) => response.status).sort(), [200, 403]);
  assert.equal(verifications, 2);
  assert.equal(contactCalls, 2);
});

test('replay storage errors or invalid claim results fail closed before Resend', async () => {
  for (const claimToken of [() => { throw new Error('private store detail'); }, () => undefined, () => 'true']) {
    let verifications = 0;
    const response = await signupWithClaim(signupRequest(), ENV, {
      claimToken,
      fetchImpl: async (url) => {
        verifications++;
        assert.equal(url, SITEVERIFY_URL);
        return turnstileVerified();
      },
    });
    assert.equal(verifications, 1);
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('a stalled replay claim respects the shared deadline and cannot later reach Resend', async (t) => {
  const deadline = new AbortController();
  t.mock.method(AbortSignal, 'timeout', () => deadline.signal);
  let releaseClaim;
  const deferredClaim = new Promise((resolve) => { releaseClaim = resolve; });
  let notifyStarted;
  const claimStarted = new Promise((resolve) => { notifyStarted = resolve; });
  let contactCalls = 0;
  const pending = signupWithClaim(signupRequest(), ENV, {
    claimToken: () => { notifyStarted(); return deferredClaim; },
    fetchImpl: async (url) => {
      if (url === SITEVERIFY_URL) return turnstileVerified();
      contactCalls++;
      return contactRecord();
    },
  });
  await claimStarted;
  deadline.abort(new DOMException('Timed out', 'TimeoutError'));
  let timeout;
  const response = await Promise.race([
    pending,
    new Promise((_resolve, reject) => { timeout = setTimeout(() => reject(new Error('Replay claim ignored the deadline.')), 1000); }),
  ]).finally(() => { clearTimeout(timeout); releaseClaim(true); });
  assert.equal(response.status, 503);
  assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(contactCalls, 0);
});

test('a deadline reached during verification cannot claim a token or contact Resend', async (t) => {
  const deadline = new AbortController();
  t.mock.method(AbortSignal, 'timeout', () => deadline.signal);
  const response = await signupWithClaim(signupRequest(), ENV, {
    claimToken: () => assert.fail('An expired deadline must not start a claim.'),
    fetchImpl: async (url) => {
      assert.equal(url, SITEVERIFY_URL);
      deadline.abort();
      return turnstileVerified();
    },
  });
  assert.equal(response.status, 503);
});

test('D1 uses one cleanup-and-claim batch, bound hash values and strict insert results', async () => {
  for (const outcome of ['accepted', 'duplicate', 'error', 'bad-row', 'bad-shape']) {
    let verifications = 0;
    let contactCalls = 0;
    let batches = 0;
    const database = {
      prepare(query) { return { bind(...values) { return { query, values }; } }; },
      async batch(statements) {
        batches++;
        assert.equal(verifications, 1);
        assert.equal(contactCalls, 0);
        assert.equal(statements.length, 2);
        assert.equal(statements[0].query, 'DELETE FROM release_signup_tokens WHERE expires_at <= ?');
        assert.match(statements[1].query, /ON CONFLICT\(token_hash\) DO NOTHING RETURNING token_hash$/);
        const [hash, expiresAt] = statements[1].values;
        assert.equal(hash, createHash('sha256').update('test-turnstile-token').digest('hex'));
        assert.ok(expiresAt > statements[0].values[0]);
        if (outcome === 'error') throw new Error('private database detail');
        if (outcome === 'bad-shape') return [{ success: true }];
        return [
          { success: true, results: [] },
          { success: true, results: outcome === 'duplicate' ? [] : [{ token_hash: outcome === 'bad-row' ? 'unexpected' : hash }] },
        ];
      },
    };
    const response = await handleReleaseSignup(signupRequest(), { ...ENV, SIGNUP_REPLAY_DB: database }, {
      fetchImpl: async (url) => {
        if (url === SITEVERIFY_URL) {
          verifications++;
          return turnstileVerified();
        }
        contactCalls++;
        return url.includes('/segments?') ? segmentList() : contactRecord();
      },
    });
    assert.equal(batches, 1);
    assert.equal(response.status, outcome === 'accepted' ? 200 : outcome === 'duplicate' ? 403 : 503);
    assert.equal(contactCalls, outcome === 'accepted' ? 2 : 0);
  }
});

test('the local replay store rejects duplicates, expires claims and stays bounded', () => {
  const claim = createLocalReplayStore(2);
  assert.equal(claim('first-hash', 110, 100), true);
  assert.equal(claim('first-hash', 110, 100), false);
  assert.equal(claim('second-hash', 120, 100), true);
  assert.throws(() => claim('third-hash', 130, 100), /full/);
  assert.equal(claim('third-hash', 130, 111), true);
  assert.equal(claim('third-hash', 130, 111), false);
});

test('saves only the contact and segment, without overriding subscription choices', async () => {
  let calls = 0;
  const signals = new Set();
  const response = await verifiedSignup(signupRequest({ email: '  person+release@example.com  ', website: '' }), ENV, {
    fetchImpl: async (url, options) => {
      calls++;
      signals.add(options.signal);
      assert.equal(options.headers.Authorization, 'Bearer test-server-key');
      assert.equal(options.headers['Content-Type'], 'application/json');
      assert.ok(options.signal instanceof AbortSignal);
      assert.equal(options.signal.aborted, false);
      assert.equal(options.redirect, 'manual');
      if (calls === 1) {
        assert.equal(url, 'https://api.resend.com/contacts/person%2Brelease%40example.com');
        assert.equal(options.method, 'GET');
        return new Response(null, { status: 404 });
      }
      if (calls === 2) {
        assert.equal(url, 'https://api.resend.com/contacts');
        assert.equal(options.method, 'POST');
        assert.deepEqual(JSON.parse(options.body), {
          email: 'person+release@example.com', segments: [{ id: DEFAULT_SEGMENT_ID }],
        });
        assert.equal(Object.hasOwn(JSON.parse(options.body), 'unsubscribed'), false);
        return confirmedContact();
      }
      if (calls === 3) {
        assert.equal(url, `https://api.resend.com/contacts/${CONTACT_ID}`);
        return contactRecord('person+release@example.com');
      }
      assert.equal(url, `https://api.resend.com/contacts/${CONTACT_ID}/segments?limit=100`);
      return segmentList();
    },
  });
  assert.equal(calls, 4);
  assert.equal(signals.size, 1, 'All upstream calls share one deadline.');
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { success: true });
});

test('supports a private segment override', async () => {
  const segmentId = '78261eea-8f8b-4381-83c6-79fa7120f1cf';
  let calls = 0;
  const response = await verifiedSignup(signupRequest(), { ...ENV, RESEND_SEGMENT_ID: segmentId }, {
    fetchImpl: async (url, options) => {
      calls++;
      if (calls === 1) return contactRecord();
      if (calls === 2) return segmentList([]);
      assert.equal(url, `https://api.resend.com/contacts/${CONTACT_ID}/segments/${segmentId}`);
      assert.equal(options.method, 'POST');
      assert.equal(options.body, undefined);
      return Response.json({ id: segmentId });
    },
  });
  assert.equal(calls, 3);
  assert.equal(response.status, 200);
});

test('an existing release subscriber succeeds without creating or updating a contact', async () => {
  const methods = [];
  const response = await verifiedSignup(signupRequest(), ENV, {
    fetchImpl: async (url, options) => {
      methods.push(options.method);
      return url.includes('/segments?') ? segmentList() : contactRecord();
    },
  });
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { success: true });
  assert.deepEqual(methods, ['GET', 'GET']);
});

test('a globally unsubscribed contact is preserved without segment or subscription writes', async () => {
  let calls = 0;
  const response = await verifiedSignup(signupRequest(), ENV, {
    fetchImpl: async (_url, options) => {
      calls++;
      assert.equal(options.method, 'GET');
      return contactRecord('person@example.com', true);
    },
  });
  assert.equal(calls, 1);
  assert.equal(response.status, 409);
  assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
});

test('a concurrent create conflict requires a verified contact and membership', async () => {
  for (const conflictStatus of [409, 422]) {
    let calls = 0;
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async (_url, options) => {
        calls++;
        if (calls === 1) return new Response(null, { status: 404 });
        if (calls === 2) {
          assert.equal(options.method, 'POST');
          return new Response(null, { status: conflictStatus });
        }
        if (calls === 3) return contactRecord();
        return segmentList();
      },
    });
    assert.equal(calls, 4);
    assert.equal(response.status, 200);
  }
});

test('a concurrent create conflict without a confirmed contact remains a failure', async () => {
  let calls = 0;
  const response = await verifiedSignup(signupRequest(), ENV, {
    fetchImpl: async () => {
      calls++;
      return new Response(null, { status: calls === 2 ? 409 : 404 });
    },
  });
  assert.equal(calls, 3);
  assert.equal(response.status, 502);
});

test('rechecks subscription state after creation and does not add an opted-out contact', async () => {
  let calls = 0;
  const response = await verifiedSignup(signupRequest(), ENV, {
    fetchImpl: async () => {
      calls++;
      if (calls === 1) return new Response(null, { status: 404 });
      if (calls === 2) return confirmedContact();
      return contactRecord('person@example.com', true);
    },
  });
  assert.equal(calls, 3);
  assert.equal(response.status, 409);
});

test('rejects contact records with a different email or missing subscription state', async () => {
  const records = [
    { object: 'contact', id: CONTACT_ID, email: 'another@example.com', unsubscribed: false },
    { object: 'contact', id: CONTACT_ID, email: 'person@example.com' },
  ];
  for (const record of records) {
    const response = await verifiedSignup(signupRequest(), ENV, { fetchImpl: async () => Response.json(record) });
    assert.equal(response.status, 502);
  }
});

test('follows segment pagination before deciding membership is missing', async () => {
  const otherSegment = '78261eea-8f8b-4381-83c6-79fa7120f1cf';
  let calls = 0;
  const response = await verifiedSignup(signupRequest(), ENV, {
    fetchImpl: async (url, options) => {
      calls++;
      assert.equal(options.method, 'GET');
      if (calls === 1) return contactRecord();
      if (calls === 2) return segmentList([otherSegment], true);
      assert.equal(new URL(url).searchParams.get('after'), otherSegment);
      return segmentList();
    },
  });
  assert.equal(calls, 3);
  assert.equal(response.status, 200);
});

test('requires a valid membership response before reporting an added subscriber', async () => {
  for (const membership of [{}, { id: CONTACT_ID }]) {
    let calls = 0;
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async () => {
        calls++;
        if (calls === 1) return contactRecord();
        if (calls === 2) return segmentList([]);
        return Response.json(membership);
      },
    });
    assert.equal(calls, 3);
    assert.equal(response.status, 502);
  }
});

test('returns a safe retryable429 at any upstream stage', async () => {
  for (const rateLimitedCall of [1, 2, 3]) {
    let calls = 0;
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async () => {
        calls++;
        if (calls === rateLimitedCall) return new Response('private upstream detail', { status: 429, headers: { 'Retry-After': '12' } });
        return calls === 1 ? contactRecord() : segmentList([]);
      },
    });
    assert.equal(calls, rateLimitedCall);
    assert.equal(response.status, 429);
    assert.equal(response.headers.get('Retry-After'), '12');
    assert.deepEqual(await response.json(), { success: false, error: 'Too many attempts. Please try again shortly.' });
  }
});

test('normalizes invalid or excessive upstream retry delays', async () => {
  for (const [header, expected] of [['invalid', '60'], ['-1', '60'], ['0', '60'], ['99999', '3600'], ['1.2', '2']]) {
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async () => new Response(null, { status: 429, headers: { 'Retry-After': header } }),
    });
    assert.equal(response.headers.get('Retry-After'), expected);
  }
});

test('does not turn upstream errors, including conflicts, into signup success', async () => {
  for (const status of [401, 409, 422, 500]) {
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async () => Response.json({ message: 'private upstream details' }, { status }),
    });
    assert.equal(response.status, 502);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('requires a well-formed contact id in a successful upstream response', async () => {
  for (const body of ['not JSON', '{}', 'null', '{"id":"not-a-uuid","object":"contact"}', JSON.stringify({ id: CONTACT_ID, object: 'error' })]) {
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async () => new Response(body, { status: 200 }),
    });
    assert.equal(response.status, 502);
    assert.equal((await response.json()).success, false);
  }
});

test('rejects upstream redirects without following them or exposing their destinations', async () => {
  for (const status of [301, 302, 307, 308]) {
    let calls = 0;
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async (_url, options) => {
        calls++;
        assert.equal(options.redirect, 'manual');
        return new Response(null, { status, headers: { Location: 'https://other.example/private' } });
      },
    });
    assert.equal(calls, 1);
    assert.equal(response.status, 502);
    assert.equal(response.headers.get('Location'), null);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('handles network failures and timeouts without returning upstream details', async () => {
  for (const error of [new Error('private network detail'), new DOMException('Timed out', 'TimeoutError')]) {
    const response = await verifiedSignup(signupRequest(), ENV, {
      fetchImpl: async () => { throw error; },
    });
    assert.equal(response.status, 502);
    assert.deepEqual(await response.json(), { success: false, error: 'Could not save your signup. Please try again.' });
  }
});

test('dev middleware leaves other routes alone and handles only its signup route', async () => {
  let middleware;
  releaseSignupDevPlugin().configureServer({ middlewares: { use(handler) { middleware = handler; } } });
  let nextCalled = false;
  await middleware({ url: '/unrelated' }, {}, () => { nextCalled = true; });
  assert.equal(nextCalled, true);

  const request = Readable.from([]);
  Object.assign(request, { url: '/api/release-signup', method: 'GET', headers: { host: '127.0.0.1:4323' }, socket: {} });
  let body;
  const headers = {};
  const response = { setHeader(name, value) { headers[name] = value; }, end(value) { body = value; } };
  await middleware(request, response, () => assert.fail('Signup route must be handled.'));
  assert.equal(response.statusCode, 405);
  assert.equal(headers.allow, 'POST');
  assert.equal(JSON.parse(body).success, false);
});

test('dev middleware rejects oversized streams without contacting Resend', async () => {
  let middleware;
  releaseSignupDevPlugin().configureServer({ middlewares: { use(handler) { middleware = handler; } } });
  const request = Readable.from([Buffer.alloc(MAX_SIGNUP_BODY_BYTES + 1)]);
  Object.assign(request, {
    url: '/api/release-signup', method: 'POST', socket: {},
    headers: { host: '127.0.0.1:4323', origin: ORIGIN, 'content-type': 'application/json' },
  });
  let body;
  const response = { setHeader() {}, end(value) { body = value; } };
  await middleware(request, response, () => assert.fail('Signup route must be handled.'));
  assert.equal(response.statusCode, 413);
  assert.equal(JSON.parse(body).success, false);
});

test('the local dev bridge rejects non-loopback hosts before reading configuration', async () => {
  let middleware;
  releaseSignupDevPlugin().configureServer({ middlewares: { use(handler) { middleware = handler; } } });
  const request = Readable.from([]);
  Object.assign(request, {
    url: '/api/release-signup', method: 'POST', socket: {},
    headers: { host: 'another.example', origin: 'http://another.example', 'content-type': 'application/json' },
  });
  let body;
  const response = { setHeader() {}, end(value) { body = value; } };
  await middleware(request, response, () => assert.fail('Signup route must be handled.'));
  assert.equal(response.statusCode, 403);
  assert.equal(JSON.parse(body).success, false);
});

test('Pages wrapper rejects alternate hostnames even with a matching Origin', async () => {
  for (const origin of ['https://teamctl-docs.pages.dev', 'https://preview.teamctl-docs.pages.dev', 'http://teamctl.run']) {
    const request = new Request(`${origin}/api/release-signup`, {
      method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' },
      body: JSON.stringify({ email: 'person@example.com', consentVersion: RELEASE_CONSENT_VERSION }),
    });
    const response = await onRequest({ request, env: {} });
    assert.equal(response.status, 403);
    assert.equal((await response.json()).success, false);
  }
});

test('Pages wrapper permits its canonical or explicitly configured origin', async () => {
  const canonical = await onRequest({ request: new Request('https://teamctl.run/api/release-signup'), env: {} });
  assert.equal(canonical.status, 405);
  const configured = await onRequest({ request: new Request(`${ORIGIN}/api/release-signup`), env: { RELEASE_SIGNUP_ORIGIN: ORIGIN } });
  assert.equal(configured.status, 405);
});

test('Pages invocation routes include only the signup endpoint', async () => {
  const routes = JSON.parse(await readFile(new URL('../public/_routes.json', import.meta.url), 'utf8'));
  assert.deepEqual(routes, { version: 1, include: ['/api/release-signup'], exclude: [] });
});

test('Pages failures log only status metadata without contact details or credentials', async (t) => {
  const warnings = [];
  t.mock.method(console, 'warn', (entry) => warnings.push(entry));
  t.mock.method(globalThis, 'fetch', async (url) => url === SITEVERIFY_URL
    ? turnstileVerified() : new Response('private upstream detail', { status: 401 }));
  const response = await onRequest({
    request: signupRequest(undefined, { headers: { Origin: ORIGIN } }),
    env: { ...ENV, RELEASE_SIGNUP_ORIGIN: ORIGIN, SIGNUP_REPLAY_DB: {
      prepare(query) { return { query, bind(...values) { return { query, values }; } }; },
      async batch(statements) { return [{ success: true, results: [] }, { success: true, results: [{ token_hash: statements[1].values[0] }] }]; },
    } },
  });
  assert.equal(response.status, 502);
  assert.deepEqual(warnings, [{ event: 'release_signup_failed', status: 502, upstreamStatuses: [200, 401] }]);
  assert.equal((await response.json()).success, false);
});
