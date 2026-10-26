// Run with: node --test crates/teamctl/assets/web/app*.test.cjs
// Exercise real async browser handlers against deferred fetch responses. The DOM
// fixture covers only the browser surface these workflows use; no provider runs.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const json = (value, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { 'Content-Type': 'application/json' },
});

async function until(predicate) {
  for (let attempt = 0; attempt < 30; attempt++) {
    if (predicate()) return;
    await new Promise(setImmediate);
  }
  assert.fail('Expected async browser work did not occur');
}

function browser(fetch, width = 1200) {
  const nodes = new Map(), timers = [];
  let document;
  class Element {
    constructor(id = '') {
      this.id = id; this.value = ''; this.dataset = {}; this.style = {};
      this.children = []; this.attributes = new Map(); this.listeners = new Map();
      this.hidden = ['inspector', 'terminal-panel', 'chat-search-bar', 'sidebar-scrim'].includes(id);
      this.disabled = false; this.inert = false; this.open = false;
      this.textContent = ''; this.innerHTML = '';
      this.scrollTop = 0; this.scrollHeight = 100; this.clientHeight = 100;
      const classes = new Set();
      this.classList = {
        add: (...values) => values.forEach((value) => classes.add(value)),
        remove: (...values) => values.forEach((value) => classes.delete(value)),
        contains: (value) => classes.has(value),
        toggle: (value, force) => {
          const enabled = force === undefined ? !classes.has(value) : force;
          enabled ? classes.add(value) : classes.delete(value); return enabled;
        },
      };
    }
    addEventListener(name, handler) { this.listeners.set(name, handler); }
    setAttribute(name, value) { this.attributes.set(name, String(value)); }
    getAttribute(name) { return this.attributes.get(name) ?? null; }
    querySelectorAll() { return []; }
    querySelector() { return new Element(); }
    append(node) { this.children.push(node); node.parentElement = this; }
    remove() { this.removed = true; }
    focus() { document.activeElement = this; }
    contains(node) {
      for (let current = node; current; current = current.parentElement) if (current === this) return true;
      return false;
    }
    showModal() { this.open = true; }
    close() { this.open = false; }
  }
  const element = (id) => {
    if (!nodes.has(id)) nodes.set(id, new Element(id));
    return nodes.get(id);
  };
  document = {
    hidden: false, activeElement: null, title: '',
    documentElement: new Element('html'),
    getElementById: element,
    querySelectorAll: () => [], querySelector: () => null,
    createElement: () => new Element(), addEventListener: () => {},
  };
  ['agent-search', 'close-sidebar', 'agent-list', 'channel-list', 'theme-toggle'].forEach((id) => element('sidebar').append(element(id)));
  ['open-sidebar', 'message-input'].forEach((id) => element('main').append(element(id)));
  const storage = new Map();
  const storageApi = { getItem: (key) => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) };
  let uuid = 0;
  const context = {
    document, window: { addEventListener: () => {} }, innerWidth: width,
    location: { hash: '#token=test-token', pathname: '/', search: '' }, history: { replaceState: () => {} },
    sessionStorage: storageApi, localStorage: storageApi,
    matchMedia: () => ({ matches: false }),
    URL, URLSearchParams, AbortSignal, Response,
    crypto: { randomUUID: () => `request-${++uuid}` },
    fetch, setTimeout: (callback) => { timers.push(callback); return timers.length; },
    clearTimeout: () => {}, setInterval: () => {}, clearInterval: () => {},
  };
  const source = fs.readFileSync(require.resolve('./app.js'), 'utf8');
  const bootstrap = '  poll(); setInterval(poll, 2500);';
  assert.equal(source.split(bootstrap).length, 2, 'Locate the single polling bootstrap for in-memory test injection');
  const injected = source.replace(bootstrap, '  globalThis.__app = { state, uploadFiles, sendMessage, switchSelection, openSidebar, closeSidebar, fitComposer };');
  vm.createContext(context); vm.runInContext(injected, context, { filename: 'app.js' });
  const app = context.__app;
  app.state.agents = [{ id: 'A', name: 'Agent A' }, { id: 'B', name: 'Agent B' }];
  app.state.channels = [{ id: 'notes', name: 'Notes' }];
  app.state.selected = { type: 'agent', id: 'A' };
  return { app, element, document, timers };
}

test('a pending multi-file batch stays in its original agent when selection changes', async (t) => {
  for (const destination of [{ type: 'agent', id: 'B' }, { type: 'channel', id: 'notes' }]) {
    await t.test(`switch to ${destination.type}`, async () => {
      const uploads = [];
      const { app } = browser((path, options) => {
        if (path === '/api/assets') {
          const response = deferred(); uploads.push({ options, response }); return response.promise;
        }
        return Promise.resolve(json({ messages: [] }));
      });
      const pending = app.uploadFiles([
        { name: 'first-private-file.txt', size: 1, type: 'text/plain' },
        { name: 'second-private-file.txt', size: 1, type: 'text/plain' },
      ]);
      assert.equal(uploads.length, 1);
      app.switchSelection(destination.type, destination.id);
      uploads[0].response.resolve(json({ id: 'file-1' }));
      await until(() => uploads.length === 2);
      uploads[1].response.resolve(json({ id: 'file-2' }));
      await pending;
      assert.equal(app.state.attachments.length, 0, 'The destination draft must not receive files from A');
      const original = app.state.drafts.get('agent:A');
      assert.deepEqual(Array.from(original.attachments, (item) => item.id), ['file-1', 'file-2']);
      assert(original.attachments.every((item) => !item.uploading && !item.failed));
    });
  }
});

test('offscreen send success preserves an edited follow-up and removes only submitted attachments', async () => {
  const post = deferred();
  const { app, element } = browser((path, options) => options?.method === 'POST'
    ? post.promise : Promise.resolve(json({ messages: [] })));
  element('message-input').value = 'The original message';
  const sent = { localId: 'sent', id: 'already-submitted', name: 'old.txt' };
  app.state.attachments.push(sent); app.fitComposer();
  const pending = app.sendMessage();
  app.switchSelection('agent', 'B');
  app.switchSelection('agent', 'A');
  element('message-input').value = 'An unsent follow-up';
  app.state.attachments.push({ localId: 'new', id: 'follow-up-only', name: 'new.txt' });
  app.switchSelection('agent', 'B');
  post.resolve(json({ message: { id: 1 } })); await pending;
  const draft = app.state.drafts.get('agent:A');
  assert(draft, 'An offscreen follow-up draft must survive successful delivery of the earlier message');
  assert.equal(draft.text, 'An unsent follow-up');
  assert.deepEqual(Array.from(draft.attachments, (item) => item.id), ['follow-up-only']);
});

test('an acknowledged POST remains sent when history refresh fails and the next identical send gets a new ID', async () => {
  const requestIds = []; let failHistory = true;
  const { app, element } = browser((path, options) => {
    if (options?.method === 'POST') {
      requestIds.push(JSON.parse(options.body).request_id);
      return Promise.resolve(json({ message: { id: requestIds.length } }));
    }
    return Promise.resolve(failHistory ? json({ error: 'History refresh unavailable' }, 500) : json({ messages: [] }));
  });
  element('message-input').value = 'ping'; app.fitComposer(); await app.sendMessage();
  assert.equal(app.state.sendRequests.has('agent:A'), false, 'Acknowledge the request before refreshing history');
  assert.equal(element('message-input').value, '');
  assert.doesNotMatch(element('compose-status').textContent, /not sent/i);
  assert.match(element('compose-status').textContent, /sent|deliver/i);
  failHistory = false;
  element('message-input').value = 'ping'; app.fitComposer(); await app.sendMessage();
  assert.equal(requestIds.length, 2);
  assert.notEqual(requestIds[0], requestIds[1], 'A later intentional identical message is a fresh request');
});

test('a failed POST keeps its draft and reuses its request ID on retry', async () => {
  const firstPost = deferred(); const requestIds = [];
  const { app, element } = browser((path, options) => {
    if (options?.method === 'POST') {
      requestIds.push(JSON.parse(options.body).request_id);
      return requestIds.length === 1 ? firstPost.promise : Promise.resolve(json({ message: { id: 1 } }));
    }
    return Promise.resolve(json({ messages: [] }));
  });
  element('message-input').value = 'Keep this draft'; app.fitComposer();
  const pending = app.sendMessage();
  firstPost.reject(new TypeError('Connection interrupted')); await pending;
  assert.equal(element('message-input').value, 'Keep this draft');
  assert.equal(app.state.sendRequests.has('agent:A'), true);
  await app.sendMessage();
  assert.equal(requestIds.length, 2); assert.equal(requestIds[0], requestIds[1]);
  assert.equal(element('message-input').value, '');
});

test('mobile navigation moves focus into the drawer and restores access and opener on close', () => {
  const { app, element, document } = browser(() => Promise.resolve(json({ messages: [] })), 390);
  element('open-sidebar').focus(); app.openSidebar();
  assert.equal(element('sidebar').inert, false);
  assert.equal(element('main').inert, true, 'The covered chat should not remain keyboard reachable');
  assert(element('sidebar').contains(document.activeElement), 'Opening the drawer moves focus into navigation');
  app.closeSidebar();
  assert.equal(element('main').inert, false);
  assert.equal(element('sidebar').inert, true);
  assert.equal(document.activeElement, element('open-sidebar'));
});
