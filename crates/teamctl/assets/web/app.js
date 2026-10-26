/* teamctl's embedded workspace: browser-native, no third-party runtime. */
(function () {
  'use strict';
  const escapeHtml = (value) => String(value ?? '').replace(/[&<>"']/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]));
  function safeLink(value) {
    try { const url = new URL(String(value)); return ['https:', 'http:', 'mailto:'].includes(url.protocol) ? url.href : null; } catch { return null; }
  }
  function safeAssetPath(asset) {
    const value = String(asset.url || `/api/assets/${encodeURIComponent(asset.id)}`);
    // Never send the workspace bearer to an attachment-supplied external URL.
    return /^\/api\/assets\/[A-Za-z0-9_%.-]+$/.test(value) && !/%2f|%5c|%2e/i.test(value) && !/\/\.\.?$/.test(value) ? value : null;
  }
  function inlineMarkdown(value) {
    const tokens = [];
    const save = (html) => `\u0000${tokens.push(html) - 1}\u0000`;
    let text = String(value).replace(/\u0000/g, '');
    text = text.replace(/`([^`\n]+)`/g, (_, code) => save(`<code>${escapeHtml(code)}</code>`));
    text = text.replace(/\[([^\]\n]+)\]\(([^\s)]+)\)/g, (_, label, target) => {
      const href = safeLink(target);
      return href ? save(`<a href="${escapeHtml(href)}" target="_blank" rel="noopener noreferrer">${escapeHtml(label)}</a>`) : label;
    });
    text = escapeHtml(text).replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>').replace(/__([^_\n]+)__/g, '<strong>$1</strong>').replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>').replace(/~~([^~\n]+)~~/g, '<del>$1</del>');
    return text.replace(/\u0000(\d+)\u0000/g, (_, index) => tokens[Number(index)] || '');
  }
  function markdown(value) {
    const lines = String(value ?? '').slice(0, 200000).replace(/\r\n?/g, '\n').split('\n');
    let result = '', paragraph = [], list = null, fence = null, code = [], quote = [];
    const flushParagraph = () => { if (paragraph.length) { result += `<p>${paragraph.map(inlineMarkdown).join('<br>')}</p>`; paragraph = []; } };
    const flushList = () => { if (list) { result += `</${list}>`; list = null; } };
    const flushQuote = () => { if (quote.length) { result += `<blockquote>${quote.map(inlineMarkdown).join('<br>')}</blockquote>`; quote = []; } };
    const flush = () => { flushParagraph(); flushList(); flushQuote(); };
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (/^\s*```/.test(line)) {
        if (fence !== null) { result += `<pre>${fence ? `<span class="code-label">${escapeHtml(fence)}</span>` : ''}<code>${escapeHtml(code.join('\n'))}</code></pre>`; fence = null; code = []; }
        else { flush(); fence = line.replace(/^\s*```/, '').trim(); }
        continue;
      }
      if (fence !== null) { code.push(line); continue; }
      if (!line.trim()) { flush(); continue; }
      const heading = line.match(/^(#{1,4})\s+(.+)$/);
      if (heading) { flush(); result += `<h${heading[1].length}>${inlineMarkdown(heading[2])}</h${heading[1].length}>`; continue; }
      if (/^\s*>\s?/.test(line)) { flushParagraph(); flushList(); quote.push(line.replace(/^\s*>\s?/, '')); continue; }
      flushQuote();
      const item = line.match(/^\s*(?:([-*])|(\d+)\.)\s+(.+)$/);
      if (item) { flushParagraph(); const tag = item[2] ? 'ol' : 'ul'; if (list !== tag) { flushList(); list = tag; result += `<${tag}>`; } result += `<li>${inlineMarkdown(item[3])}</li>`; continue; }
      flushList();
      if (line.includes('|') && i + 1 < lines.length && /^\s*\|?\s*:?-{3,}/.test(lines[i + 1])) {
        flushParagraph(); const cells = (row) => row.trim().replace(/^\||\|$/g, '').split('|').map((x) => inlineMarkdown(x.trim()));
        result += `<div class="table-scroll"><table><thead><tr>${cells(line).map((c) => `<th>${c}</th>`).join('')}</tr></thead><tbody>`; i++;
        while (i + 1 < lines.length && lines[i + 1].includes('|') && lines[i + 1].trim()) { i++; result += `<tr>${cells(lines[i]).map((c) => `<td>${c}</td>`).join('')}</tr>`; }
        result += '</tbody></table></div>'; continue;
      }
      paragraph.push(line);
    }
    flush();
    if (fence !== null) result += `<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`;
    return result;
  }
  function initials(name) { return String(name || '?').split(/[\s:_-]+/).filter(Boolean).slice(0, 2).map((part) => part[0]).join('').toUpperCase(); }
  function timestamp(value) { const number = Number(value); const date = typeof value === 'number' || /^\d+(\.\d+)?$/.test(String(value)) ? new Date(number < 1e12 ? number * 1000 : number) : new Date(value); return Number.isNaN(date.valueOf()) ? null : date; }
  function bytes(value) { const number = Number(value); if (!Number.isFinite(number)) return ''; return number < 1024 ? `${number} B` : number < 1048576 ? `${(number / 1024).toFixed(0)} KB` : `${(number / 1048576).toFixed(1)} MB`; }
  if (typeof module !== 'undefined' && module.exports) module.exports = { escapeHtml, safeLink, safeAssetPath, inlineMarkdown, markdown, initials, timestamp, bytes };
  if (typeof document === 'undefined') return;

  const icons = {
    message: '<path d="M21 11.5a8.4 8.4 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8v.5Z"/>',
    activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>', shield: '<path d="M12 3 4 6v6c0 5 8 9 8 9s8-4 8-9V6l-8-3Z"/><path d="m9 12 2 2 4-4"/>',
    search: '<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/>', terminal: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m7 9 3 3-3 3m6 0h4"/>', panel: '<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M15 4v16"/>', plus: '<path d="M12 5v14M5 12h14"/>', mic: '<rect x="9" y="3" width="6" height="12" rx="3"/><path d="M5 10v2a7 7 0 0 0 14 0v-2M12 19v3m-4 0h8"/>', 'arrow-up': '<path d="M12 19V5m-6 6 6-6 6 6"/>', close: '<path d="m6 6 12 12M6 18 18 6"/>', menu: '<path d="M4 6h16M4 12h16M4 18h16"/>', sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5"/>', moon: '<path d="M21 13a9 9 0 0 1-10-10 9 9 0 1 0 10 10Z"/>', upload: '<path d="M12 16V3m-5 5 5-5 5 5M4 15v5h16v-5"/>', file: '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6Z"/><path d="M14 2v6h6"/>', lock: '<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3m-4 4v3"/>', component: '<path d="m12 3 9 5-9 5-9-5 9-5Zm-9 9 9 5 9-5M3 16l9 5 9-5"/>', check: '<path d="m5 12 4 4L19 6"/>'
  };
  const icon = (name) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${icons[name] || icons.file}</svg>`;
  const $ = (id) => document.getElementById(id);
  const paintIcons = (node = document) => node.querySelectorAll('[data-icon]').forEach((element) => { element.innerHTML = icon(element.dataset.icon); });
  paintIcons();
  for (const id of ['close-sidebar', 'close-chat-search', 'terminal-close', 'inspector-close']) $(id).innerHTML = icon('close');
  $('open-sidebar').innerHTML = icon('menu');

  const state = { snapshot: {}, agents: [], channels: [], interfaces: [], selected: null, view: 'chat', messages: [], messageSignature: '', inspectorSignature: '', inspectorTab: 'overview', attachments: [], pending: false, polling: false, connected: false, recording: null, recordTimer: null, drafts: new Map(), sendRequests: new Map(), media: new Map(), mediaPending: new Map(), blobUrls: new Map(), terminalMode: 'agent', hasOlder: false, olderLoading: false, generation: 0 };
  let token = '';
  try {
    const fragment = new URLSearchParams(location.hash.slice(1));
    const incoming = fragment.get('token');
    if (incoming) { token = incoming; history.replaceState(null, '', location.pathname + location.search); sessionStorage.setItem('teamctl.access-token', incoming); }
    else token = sessionStorage.getItem('teamctl.access-token') || '';
  } catch { /* Private browser storage can be disabled; keep the in-memory token. */ }
  const theme = localStorageSafe('teamctl.theme') || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  setTheme(theme);
  function localStorageSafe(key, value) { try { if (value !== undefined) localStorage.setItem(key, value); return localStorage.getItem(key); } catch { return null; } }
  function setTheme(value) { document.documentElement.classList.toggle('theme-dark', value === 'dark'); $('theme-toggle').innerHTML = icon(value === 'dark' ? 'sun' : 'moon'); $('theme-toggle').setAttribute('aria-label', value === 'dark' ? 'Use light theme' : 'Use dark theme'); }
  $('theme-toggle').addEventListener('click', () => { const next = document.documentElement.classList.contains('theme-dark') ? 'light' : 'dark'; setTheme(next); localStorageSafe('teamctl.theme', next); });
  const array = (value) => Array.isArray(value) ? value : value && typeof value === 'object' ? Object.entries(value).map(([id, item]) => typeof item === 'object' && item ? { id, ...item } : { id, name: String(item) }) : [];
  const label = (agent) => agent?.display_name || agent?.name || agent?.agent || agent?.id || 'Agent';
  const agentById = (id) => state.agents.find((agent) => agent.id === id || agent.agent === id);
  const selectedAgent = () => state.selected?.type === 'agent' ? agentById(state.selected.id) : null;
  const selectedChannel = () => state.selected?.type === 'channel' ? state.channels.find((channel) => channel.id === state.selected.id) : null;
  const selectionKey = () => state.selected ? `${state.selected.type}:${state.selected.id}` : '';
  const statusOf = (agent) => { const value = String(agent?.status || agent?.state || 'unknown').toLowerCase(); return value === 'busy' ? 'working' : value; };
  const runtimeOf = (agent) => typeof agent?.runtime === 'object' ? agent.runtime.provider || agent.runtime.kind || 'Agent runtime' : agent?.runtime || agent?.provider || 'Agent runtime';
  const palette = [['#e8ecdf', '#60734a'], ['#e6ebf0', '#5a748a'], ['#f1e8db', '#957646'], ['#eee3e6', '#946777'], ['#e6e3ef', '#7c6c9d'], ['#deece6', '#557f6c']];
  function avatar(name, id, classes = '', status = false) { let hash = 0; for (const character of String(id || name)) hash = (hash * 31 + character.charCodeAt(0)) >>> 0; const colors = palette[hash % palette.length]; return `<span class="avatar ${classes}" style="--avatar-bg:${colors[0]};--avatar-text:${colors[1]}">${escapeHtml(initials(name))}${status ? `<i class="agent-status ${escapeHtml(status)}" aria-hidden="true"></i>` : ''}</span>`; }
  function timeLabel(value) { return timestamp(value)?.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) || ''; }
  function dateLabel(value) { const date = timestamp(value); if (!date) return ''; const today = new Date(); if (date.toDateString() === today.toDateString()) return 'Today'; const yesterday = new Date(); yesterday.setDate(today.getDate() - 1); if (date.toDateString() === yesterday.toDateString()) return 'Yesterday'; return date.toLocaleDateString([], { month: 'short', day: 'numeric' }); }
  function toast(message) { const node = document.createElement('div'); node.className = 'toast'; node.textContent = message; $('toast-region').append(node); setTimeout(() => node.remove(), 5000); }
  function setConnection(connected, message = '') { state.connected = connected; $('connection-label').textContent = connected ? 'Connected to your workspace' : 'Reconnecting…'; $('connection-dot').className = `connection-dot${connected ? '' : ' stale'}`; $('connection-banner').hidden = connected; $('connection-banner').textContent = message || 'Connection interrupted. Your draft is saved here. Reconnecting automatically…'; }
  async function api(path, options = {}) {
    const headers = { ...(options.body && typeof options.body === 'string' ? { 'Content-Type': 'application/json' } : {}), ...options.headers, Authorization: `Bearer ${token}` };
    const response = await fetch(path, { ...options, headers, cache: 'no-store', credentials: 'omit', signal: AbortSignal.timeout(15000) });
    if (response.status === 401 || response.status === 403) { if (!$('auth-dialog').open) $('auth-dialog').showModal(); throw new Error('The workspace access token is missing or no longer valid.'); }
    if (!response.ok) { const body = await response.text(); let message = body; try { message = JSON.parse(body).error || body; } catch { /* Server may return plain text. */ } throw new Error(String(message || `Request failed (${response.status})`).slice(0, 240)); }
    return response.status === 204 ? {} : response.json();
  }
  function currentInterfaces(agent = selectedAgent()) {
    const attached = array(agent?.interfaces);
    if (attached.length) return attached.map((item) => typeof item === 'string' ? state.interfaces.find((candidate) => candidate.id === item) || { id: item, name: item } : { ...state.interfaces.find((candidate) => candidate.id === item.id), ...item });
    return state.interfaces.filter((item) => !item.agents || item.agents.includes(agent?.id) || item.agents.includes(agent?.agent));
  }
  function webInterface() { return currentInterfaces().find((item) => item.id === 'web') || currentInterfaces().find((item) => ['web', 'browser'].includes(item.provider || item.kind)) || state.interfaces.find((item) => ['web', 'browser'].includes(item.provider || item.kind)) || { id: 'web', name: 'Web workspace', provider: 'web', description: 'This browser workspace' }; }
  function pendingApprovals(agent = null) { return array(state.snapshot.approvals).filter((item) => (!item.status || item.status === 'pending') && (!agent || [item.agent, item.agent_id, item.requester].includes(agent.id))); }

  function renderRoster() {
    const focusedAgent = document.activeElement?.dataset.agent, focusedChannel = document.activeElement?.dataset.channel;
    const query = $('agent-search').value.toLocaleLowerCase();
    $('project-name').textContent = state.snapshot.project?.name || state.snapshot.team_name || state.snapshot.project?.id || 'Agent workspace';
    $('agent-count').textContent = state.agents.length; $('workspace-kind').textContent = state.snapshot.demo ? 'DEMO' : 'LOCAL';
    $('agent-list').innerHTML = state.agents.filter((agent) => `${label(agent)} ${agent.id} ${agent.description || ''}`.toLocaleLowerCase().includes(query)).map((agent) => `<button class="agent-row${state.selected?.type === 'agent' && state.selected.id === agent.id ? ' selected' : ''}" data-agent="${escapeHtml(agent.id)}" ${state.selected?.id === agent.id ? 'aria-current="true"' : ''}>${avatar(label(agent), agent.id, '', statusOf(agent))}<span class="agent-copy"><span class="agent-name">${escapeHtml(label(agent))}</span><span class="agent-preview">${escapeHtml(agent.health?.last_error || agent.last_message || agent.description || `${runtimeOf(agent)} · ${statusOf(agent)}`)}</span></span>${Number(agent.inbox_count || agent.unread_mail || agent.unread || 0) ? `<span class="agent-unread">${Number(agent.inbox_count || agent.unread_mail || agent.unread)}</span>` : ''}</button>`).join('') || `<p class="small-empty">${query ? 'No matching agents.' : 'No agents defined yet.'}</p>`;
    $('channel-section').hidden = !state.channels.length;
    $('channel-list').innerHTML = state.channels.filter((channel) => `${channel.name} ${channel.id}`.toLocaleLowerCase().includes(query)).map((channel) => `<button class="channel-row${state.selected?.type === 'channel' && state.selected.id === channel.id ? ' selected' : ''}" data-channel="${escapeHtml(channel.id)}"><span class="channel-hash">#</span>${escapeHtml(channel.name || channel.id)}</button>`).join('');
    if (focusedAgent) [...$('agent-list').querySelectorAll('[data-agent]')].find((button) => button.dataset.agent === focusedAgent)?.focus({ preventScroll: true });
    if (focusedChannel) [...$('channel-list').querySelectorAll('[data-channel]')].find((button) => button.dataset.channel === focusedChannel)?.focus({ preventScroll: true });
    const approvals = pendingApprovals(); $('approval-count').textContent = approvals.length || ''; $('message-count').textContent = '';
  }
  function renderHeader() {
    const agent = selectedAgent(), channel = selectedChannel();
    const title = state.view === 'activity' ? 'Workspace activity' : state.view === 'approvals' ? 'Needs attention' : agent ? label(agent) : channel ? channel.name || channel.id : 'Your workspace';
    const subtitle = state.view === 'activity' ? 'The work happening between conversations' : state.view === 'approvals' ? 'Decisions waiting for you' : agent ? `${statusOf(agent)} · ${runtimeOf(agent)}` : channel ? `${array(channel.members).length || 'Shared'} participants` : `${state.agents.length} agents connected`;
    $('conversation-title').innerHTML = `${agent && state.view === 'chat' ? avatar(label(agent), agent.id) : `<span class="avatar neutral">${channel && state.view === 'chat' ? '#' : icon(state.view === 'approvals' ? 'shield' : state.view === 'activity' ? 'activity' : 'message')}</span>`}<div><h1>${escapeHtml(title)}</h1><p>${agent && state.view === 'chat' ? `<span class="status-word ${escapeHtml(statusOf(agent))}">${escapeHtml(subtitle)}</span>` : escapeHtml(subtitle)}</p></div>`;
    const healthError = agent?.health?.last_error; $('agent-health-banner').hidden = state.view !== 'chat' || !healthError; $('agent-health-message').textContent = healthError ? `${label(agent)} needs attention: ${String(healthError).slice(0, 240)}` : '';
    $('terminal-toggle').hidden = !agent || state.view !== 'chat'; $('inspector-toggle').hidden = !state.selected || state.view !== 'chat'; $('chat-search-toggle').hidden = state.view !== 'chat';
    $('message-input').placeholder = agent ? `Message ${label(agent)}…` : channel ? `Message #${channel.name || channel.id}…` : 'Choose an agent to start…';
    $('composer-interface').innerHTML = `<span class="tiny-dot"></span>${escapeHtml(webInterface().name || 'Web workspace')}`;
    $('compose-wrap').hidden = !state.selected || state.view !== 'chat'; updateSendButton();
    document.title = `${agent && state.view === 'chat' ? label(agent) : title} · teamctl`;
  }
  function switchSelection(type, id) {
    if (state.recording) return toast('Finish your voice recording before switching conversations.');
    if (selectionKey() === `${type}:${id}`) { setView('chat'); closeSidebar(); return; }
    state.drafts.set(selectionKey(), { text: $('message-input').value, attachments: state.attachments });
    state.selected = { type, id }; state.terminalMode = 'agent'; $('terminal-mode').value = 'agent'; state.hasOlder = false; state.olderLoading = false; state.messages = []; clearMediaCache(); state.messageSignature = ''; state.inspectorSignature = ''; state.generation++;
    const draft = state.drafts.get(selectionKey()); $('message-input').value = draft?.text || ''; state.attachments = draft?.attachments || []; fitComposer(); renderAttachments();
    try { sessionStorage.setItem('teamctl.selection', JSON.stringify(state.selected)); } catch { /* Optional preference. */ }
    $('messages').innerHTML = '<div class="loading-state"><div class="loading-orbit"></div><p>Opening conversation…</p></div>'; $('terminal-output').textContent = 'Connecting to session…';
    if (type === 'channel') toggleTerminal(false);
    setView('chat'); renderRoster(); renderHeader(); renderInspector(); closeSidebar(); loadMessages().catch(showError); if (!$('terminal-panel').hidden) loadTerminal().catch(showTerminalError);
  }
  function setView(view) { if (state.recording && view !== state.view) return toast('Finish your voice recording before changing views.'); state.view = view; $('chat-view').hidden = view !== 'chat'; $('activity-view').hidden = view !== 'activity'; $('approvals-view').hidden = view !== 'approvals'; document.querySelectorAll('[data-view]').forEach((button) => { button.classList.toggle('selected', button.dataset.view === view); }); if (view !== 'chat') { $('inspector').hidden = true; $('inspector-toggle').setAttribute('aria-pressed', 'false'); $('chat-search-bar').hidden = true; } renderHeader(); if (view === 'activity') renderActivity(); if (view === 'approvals') renderApprovals(); closeSidebar(); }
  function messageTime(message) { return message.created_at || message.sent_at || message.timestamp; }
  function messageText(message) { return message.text || message.content || message.body || message.payload?.text || ''; }
  function messageRole(message) { return message.role || (String(message.sender || '').startsWith('user:') || message.sender === 'user' || message.direction === 'inbound' && message.interface ? 'user' : 'assistant'); }
  function messageInterface(message) {
    let value = message.interface || message.interface_context || message.metadata?.interface || message.source?.interface;
    if (typeof value === 'string') value = state.interfaces.find((item) => item.id === value) || { id: value, name: value };
    if (!value && message.interface_id) value = state.interfaces.find((item) => item.id === message.interface_id) || { id: message.interface_id, name: message.interface_name, provider: message.interface_provider, description: message.interface_description };
    return value || (messageRole(message) === 'user' ? webInterface() : { id: 'mailbox', name: 'Agent mailbox', provider: 'internal', description: 'A message exchanged through the teamctl mailbox' });
  }
  function provenance(message) { const source = messageInterface(message); return `<details class="provenance"><summary>${escapeHtml(source.name || source.id || 'Interface')}<span>· ${escapeHtml(source.provider || source.kind || 'interface')}</span></summary><dl><dt>Interface</dt><dd>${escapeHtml(source.provider || source.kind || 'unknown')}</dd><dt>Name</dt><dd>${escapeHtml(source.name || source.id || 'Unnamed interface')}</dd><dt>Description</dt><dd>${escapeHtml(source.description || 'No description provided')}</dd><dt>ID</dt><dd>${escapeHtml(source.id || 'unknown')}</dd></dl></details>`; }
  function messageAssets(message) { return array(message.attachments || message.payload?.attachments).map((item) => typeof item === 'string' ? { id: item } : item); }
  function attachmentHtml(asset) { const key = String(asset.id || asset.url); state.media.set(key, asset); return `<div class="asset-placeholder" data-asset-key="${escapeHtml(key)}"><span class="asset-meta">Loading ${escapeHtml(asset.name || 'attachment')}…</span></div>`; }
  function messageHtml(message) { const content = message.html || (message.kind === 'html' ? message.payload?.html || message.payload?.content || messageText(message) : null) || message.payload?.html; return typeof content === 'string' ? content : null; }
  function renderMessages(force = false) {
    const query = $('message-search').value.trim().toLocaleLowerCase(); const messages = state.messages.filter((message) => !query || messageText(message).toLocaleLowerCase().includes(query));
    $('search-result-count').textContent = query ? `${messages.length} ${messages.length === 1 ? 'match' : 'matches'}` : '';
    const signature = JSON.stringify([state.messages, query, selectionKey(), state.hasOlder, state.olderLoading]); if (!force && signature === state.messageSignature) return;
    state.messageSignature = signature; const element = $('messages'), nearBottom = element.scrollHeight - element.scrollTop - element.clientHeight < 120, previousTop = element.scrollTop;
    const openSources = new Set([...element.querySelectorAll('.provenance[open]')].map((detail) => detail.closest('[data-message-id]')?.dataset.messageId));
    if (!state.selected) { element.innerHTML = `<div class="empty-state"><span class="avatar neutral">${icon('message')}</span><h2>A little space for big things.</h2><p>Your agents, conversations, and work come together here.<br>Define an agent, run <code>teamctl up</code>, and say hello.</p></div>`; return; }
    if (!messages.length) {
      const agent = selectedAgent(), name = agent ? label(agent) : `#${selectedChannel()?.name || 'channel'}`;
      element.innerHTML = `<div class="empty-state">${agent ? avatar(name, agent.id) : '<span class="avatar neutral">#</span>'}<h2>${query ? 'Nothing here matches yet.' : `Say hello to ${escapeHtml(name)}.`}</h2><p>${query ? 'Try a different word, or clear the search to see the conversation.' : agent ? 'Give your agent a task, share an idea, or ask what it’s working on. You can come back whenever you need.' : 'A shared conversation for agents to coordinate, exchange ideas, and keep you in the loop.'}</p>${query ? '' : '<div class="suggestion-list"><button data-suggestion="What are you working on?">What are you working on?</button><button data-suggestion="Help me organize my next steps.">Organize my next steps</button></div>'}</div>`; return;
    }
    let previousDay = '';
    const rendered = (!query && state.hasOlder ? `<div class="history-control" data-day="history-control"><button data-load-older ${state.olderLoading ? 'disabled' : ''}>${state.olderLoading ? 'Loading…' : 'Load earlier messages'}</button></div>` : '') + messages.map((message, index) => {
      const role = messageRole(message), sender = agentById(message.sender || message.agent || state.selected.id), name = role === 'user' ? 'You' : role === 'system' ? 'Workspace' : label(sender) || message.sender || 'Agent';
      const day = dateLabel(messageTime(message)), divider = day && day !== previousDay ? `<div class="day-divider" data-day="${escapeHtml(day)}">${escapeHtml(day)}</div>` : ''; previousDay = day;
      const html = messageHtml(message), assets = messageAssets(message), rawText = message.kind === 'html' && !message.text ? '' : messageText(message);
      return `${divider}<article class="message ${escapeHtml(role)}" data-message-id="${escapeHtml(message.id ?? index)}">${role === 'assistant' ? avatar(name, sender?.id || message.sender, 'message-avatar') : ''}<div class="message-content"><div class="message-heading"><strong>${escapeHtml(name)}</strong><time>${escapeHtml(timeLabel(messageTime(message)))}</time>${message.pending ? '<span class="asset-meta">Sending…</span>' : ''}</div>${rawText ? `<div class="message-body">${markdown(rawText)}</div>` : ''}${assets.length ? `<div class="message-attachments">${assets.map(attachmentHtml).join('')}</div>` : ''}${html ? `<div class="interactive-card"><div class="interactive-heading">${icon('component')}<span>Interactive component</span><span>Isolated</span></div><iframe title="Interactive component from ${escapeHtml(name)}" sandbox="allow-scripts" referrerpolicy="no-referrer" data-component="${index}"></iframe></div>` : ''}${role !== 'system' ? provenance(message) : ''}</div></article>`;
    }).join('');
    const template = document.createElement('template'); template.innerHTML = rendered;
    const existing = new Map([...element.children].map((row) => [row.dataset.messageId ? `message:${row.dataset.messageId}` : `day:${row.dataset.day}`, row]));
    const signatures = new Map(messages.map((message, index) => [String(message.id ?? index), JSON.stringify(message)]));
    const desired = [...template.content.children].map((row) => { const id = row.dataset.messageId, key = id ? `message:${id}` : `day:${row.dataset.day}`, old = existing.get(key), signature = id ? signatures.get(id) : row.textContent; if (old?.teamctlSignature === signature) return old; row.teamctlSignature = signature; return row; });
    const keep = new Set(desired); [...element.children].forEach((row) => { if (!keep.has(row)) { row.querySelectorAll('[data-asset-key]').forEach((holder) => assetObserver?.unobserve(holder)); row.remove(); } });
    let cursor = element.firstElementChild; for (const row of desired) { if (row === cursor) cursor = cursor.nextElementSibling; else element.insertBefore(row, cursor); }
    element.querySelectorAll('[data-message-id]').forEach((row) => { if (openSources.has(row.dataset.messageId)) row.querySelector('details')?.setAttribute('open', ''); });
    element.querySelectorAll('[data-component]').forEach((frame) => { if (!frame.dataset.componentReady) { frame.srcdoc = componentDocument(messageHtml(messages[Number(frame.dataset.component)])); frame.dataset.componentReady = 'true'; } });
    hydrateAssets();
    if (force || nearBottom) { element.scrollTop = element.scrollHeight; $('jump-latest').hidden = true; } else { element.scrollTop = previousTop; $('jump-latest').hidden = false; }
  }
  function componentDocument(html) {
    const parsed = new DOMParser().parseFromString(String(html || ''), 'text/html');
    parsed.querySelectorAll('base,meta[http-equiv],script[src],link').forEach((node) => node.remove());
    const nonce = document.querySelector('meta[name="component-nonce"]')?.content || '';
    parsed.querySelectorAll('script').forEach((script) => script.setAttribute('nonce', nonce));
    const resize = parsed.createElement('script'); resize.setAttribute('nonce', nonce); resize.textContent = "new ResizeObserver(()=>parent.postMessage({type:'teamctl-component-size',height:Math.max(document.body.scrollHeight,document.body.getBoundingClientRect().height)+2},'*')).observe(document.body);"; parsed.body.append(resize);
    // Keep both the component and its sandbox origin away from the authenticated host.
    const policy = document.createElement('meta'); policy.httpEquiv = 'Content-Security-Policy'; policy.content = `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline'; img-src data: blob:; media-src data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none';`;
    parsed.head.prepend(policy);
    // Nonces also authorize external scripts. A second intersecting policy
    // permits inline scripts only, so components cannot reuse their nonce
    // to create an outbound script request after the initial sanitization.
    const inlineOnly = parsed.createElement('meta'); inlineOnly.httpEquiv = 'Content-Security-Policy'; inlineOnly.content = "script-src 'unsafe-inline';"; parsed.head.prepend(inlineOnly);
    const referrer = document.createElement('meta'); referrer.name = 'referrer'; referrer.content = 'no-referrer'; parsed.head.prepend(referrer);
    return '<!doctype html>' + parsed.documentElement.outerHTML;
  }
  async function loadAsset(asset) {
    const path = safeAssetPath(asset); if (!path) throw new Error('Unsupported attachment location');
    if (state.blobUrls.has(path)) return state.blobUrls.get(path);
    if (state.mediaPending.has(path)) return state.mediaPending.get(path);
    const promise = (async () => {
      const response = await fetch(path, { headers: { Authorization: `Bearer ${token}` }, cache: 'no-store', credentials: 'omit', signal: AbortSignal.timeout(15000) }); if (!response.ok) throw new Error(response.status === 404 ? 'Attachment expired or removed' : 'Attachment unavailable');
      const blob = await response.blob(); const url = URL.createObjectURL(blob); const value = { url, mime: asset.mime_type || blob.type, size: asset.size || blob.size }; state.blobUrls.set(path, value); return value;
    })(); state.mediaPending.set(path, promise); try { return await promise; } finally { state.mediaPending.delete(path); }
  }
  const assetObserver = 'IntersectionObserver' in window ? new IntersectionObserver((entries) => { entries.forEach((entry) => { if (entry.isIntersecting) { assetObserver.unobserve(entry.target); hydrateAsset(entry.target); } }); }, { root: $('messages'), rootMargin: '250px' }) : null;
  function clearMediaCache() { assetObserver?.disconnect(); state.blobUrls.forEach((item) => URL.revokeObjectURL(item.url)); state.blobUrls.clear(); state.media.clear(); }
  function hydrateAssets() { $('messages').querySelectorAll('[data-asset-key]').forEach((holder) => { if (!holder.dataset.assetLoaded) assetObserver ? assetObserver.observe(holder) : hydrateAsset(holder); }); }
  async function hydrateAsset(holder) {
    const asset = state.media.get(holder.dataset.assetKey); if (!asset || holder.dataset.assetLoaded) return;
    try { const loaded = await loadAsset(asset); if (!holder.isConnected) return; const mime = loaded.mime || '', name = asset.name || 'Attachment'; holder.dataset.assetLoaded = 'true';
      if (mime.startsWith('image/') && !mime.startsWith('image/svg+xml')) { const image = new Image(); image.src = loaded.url; image.alt = name; image.className = 'asset-image'; image.loading = 'lazy'; holder.replaceChildren(image); }
      else if (mime.startsWith('audio/')) { const audio = document.createElement('audio'); audio.src = loaded.url; audio.controls = true; audio.preload = 'metadata'; audio.className = 'asset-audio'; audio.setAttribute('aria-label', name); holder.replaceChildren(audio); }
      else { holder.innerHTML = `<a class="asset-file" download="${escapeHtml(name)}" href="${escapeHtml(loaded.url)}">${icon('file')}<span>${escapeHtml(name)}<br><span class="asset-meta">${escapeHtml(bytes(loaded.size))} · Download</span></span></a>`; }
    } catch (error) { if (holder.isConnected) { holder.className = 'asset-error'; holder.textContent = `${asset.name || 'Attachment'} · ${error.message}`; } }
  }
  async function loadMessages() { if (!state.selected) { renderMessages(); return; } const key = selectionKey(), query = `${state.selected.type}=${encodeURIComponent(state.selected.id)}`; const response = await api(`/api/messages?${query}`); if (selectionKey() !== key) return; const incoming = array(response.messages || response); if (!state.messages.length) state.hasOlder = incoming.length >= 200; const merged = new Map(state.messages.map((message) => [String(message.id), message])); incoming.forEach((message) => merged.set(String(message.id), message)); state.messages = [...merged.values()].sort((a, b) => Number(a.id) - Number(b.id)); renderMessages(); }
  async function loadOlderMessages() { if (!state.selected || state.olderLoading || !state.messages.length) return; const key = selectionKey(), oldest = state.messages[0].id, top = $('messages').scrollTop, height = $('messages').scrollHeight; state.olderLoading = true; renderMessages(); try { const result = await api(`/api/messages?${state.selected.type}=${encodeURIComponent(state.selected.id)}&before=${encodeURIComponent(oldest)}`); if (key !== selectionKey()) return; const older = array(result.messages || result), merged = new Map([...older, ...state.messages].map((message) => [String(message.id), message])); state.messages = [...merged.values()].sort((a, b) => Number(a.id) - Number(b.id)); state.hasOlder = older.length >= 200; } catch (error) { showError(error); } finally { if (key === selectionKey()) { state.olderLoading = false; renderMessages(); $('messages').scrollTop = top + $('messages').scrollHeight - height; } } }

  function renderInspector() {
    if ($('inspector').hidden) return;
    const agent = selectedAgent(), channel = selectedChannel(); const signature = JSON.stringify([agent, channel, state.inspectorTab, state.snapshot.approvals, state.snapshot.budgets, state.interfaces, state.snapshot.events, state.snapshot.web, state.snapshot.cache]); if (signature === state.inspectorSignature) return; state.inspectorSignature = signature;
    document.querySelectorAll('[data-inspector-tab]').forEach((button) => button.setAttribute('aria-selected', String(button.dataset.inspectorTab === state.inspectorTab)));
    $('inspector-body').innerHTML = !agent ? channelInspector(channel) : state.inspectorTab === 'connections' ? connectionsInspector(agent) : state.inspectorTab === 'activity' ? activityInspector(agent) : overviewInspector(agent);
  }
  const detailList = (rows) => `<dl class="detail-list">${rows.filter(([, value]) => value !== undefined && value !== null && value !== '').map(([name, value]) => `<dt>${escapeHtml(name)}</dt><dd>${escapeHtml(typeof value === 'object' ? JSON.stringify(value) : value)}</dd>`).join('')}</dl>`;
  const tags = (items) => `<div class="tag-list">${array(items).map((item) => `<span class="pill">${escapeHtml(typeof item === 'string' ? item : item.name || item.id || JSON.stringify(item))}</span>`).join('')}</div>`;
  function overviewInspector(agent) {
    const budget = array(state.snapshot.budgets).find((item) => [item.agent, item.agent_id, item.id].includes(agent.id)) || agent.budget;
    const retention = state.snapshot.cache?.retention_days, retentionLabel = retention === null ? 'Keep indefinitely' : `${retention ?? state.snapshot.web?.asset_retention_days ?? 30} days`;
    const tools = agent.tools || agent.config?.tools || []; const mcp = agent.mcp || agent.mcp_servers || agent.config?.mcp || [];
    return `<div class="inspector-agent">${avatar(label(agent), agent.id, 'large')}<h3>${escapeHtml(label(agent))}</h3><p>${escapeHtml(agent.description || 'An agent in your workspace')}</p></div>${agent.health?.last_error ? `<div class="health-detail"><strong>Runtime needs attention</strong><p>${escapeHtml(agent.health.last_error)}</p></div>` : ''}${detailList([['Agent ID', agent.id], ['Status', statusOf(agent)], ['Runtime', runtimeOf(agent)], ['Execution', agent.execution?.mode || 'tmux'], ['Sandbox', agent.execution?.sandbox?.type || 'host'], ['Model', agent.model], ['Workspace', agent.workspace || agent.working_dir || agent.cwd], ['Reports to', agent.reports_to], ['Unread mail', agent.inbox_count ?? agent.unread_mail ?? agent.unread], ['Loop', agent.loop || agent.heartbeat]])}<section class="inspector-section"><h3>Tools & MCP</h3>${array(tools).length ? tags(tools) : '<p class="empty-detail">Runtime tools are available inside the agent session.</p>'}${array(mcp).length ? `<div style="margin-top:9px">${tags(mcp)}</div>` : ''}</section>${budget ? `<section class="inspector-section"><h3>Budget</h3>${detailList(Object.entries(budget).filter(([key]) => !['agent', 'agent_id', 'id'].includes(key)))}</section>` : ''}<section class="inspector-section"><h3>Asset storage</h3>${detailList([['Retention', retentionLabel], ['Cache size', (state.snapshot.cache?.size_bytes ?? state.snapshot.cache?.bytes) ? bytes(state.snapshot.cache.size_bytes ?? state.snapshot.cache.bytes) : undefined], ['Location', state.snapshot.cache?.path || state.snapshot.web?.cache_dir]])}<p class="empty-detail">Shared images, audio, and files stay in the workspace cache. Retention is configurable in your team definition.</p></section><details class="raw-config"><summary>View agent snapshot</summary><pre>${escapeHtml(JSON.stringify(agent.config || agent, null, 2))}</pre></details>`;
  }
  function connectionsInspector(agent) {
    const interfaces = currentInterfaces(agent), channels = state.channels.filter((channel) => !channel.members || channel.members === '*' || array(channel.members).some((item) => (typeof item === 'string' ? item : item.id) === agent.id || (typeof item === 'string' ? item : item.id) === agent.agent));
    return `<section><h3 class="section-label" style="padding-left:0">Messaging interfaces</h3><p class="empty-detail">Each interface has its own identity and context, including separate connections to the same provider.</p>${interfaces.map((item) => `<div class="detail-card"><div class="card-meta"><span class="pill">${escapeHtml(item.provider || item.kind || 'interface')}</span>${item.status ? `<span class="asset-meta">${escapeHtml(item.status)}</span>` : ''}</div><h4>${escapeHtml(item.name || item.id)}</h4><p>${escapeHtml(item.description || 'No description provided')}</p><p>ID · ${escapeHtml(item.id)}</p></div>`).join('') || '<p class="empty-detail">No explicit interfaces are configured.</p>'}</section><section class="inspector-section"><h3>Channels</h3>${channels.map((item) => `<div class="detail-card"><h4># ${escapeHtml(item.name || item.id)}</h4><p>${escapeHtml(item.description || item.id)}</p>${item.members ? `<p>${item.members === '*' ? 'All agents' : array(item.members).map((member) => escapeHtml(typeof member === 'string' ? label(agentById(member)) || member : member.name || member.id)).join(', ')}</p>` : ''}</div>`).join('') || '<p class="empty-detail">No shared channels for this agent.</p>'}</section>`;
  }
  function activityInspector(agent) { const approvals = pendingApprovals(agent), events = array(state.snapshot.events).filter((event) => [event.agent, event.agent_id, event.sender, event.subject].includes(agent.id)).slice(0, 25); return `<section><h3 class="section-label" style="padding-left:0">Needs attention</h3>${approvals.length ? approvals.map((item) => `<div class="detail-card"><h4>${escapeHtml(item.title || item.action || item.kind || 'Approval requested')}</h4><p>${escapeHtml(item.summary || item.description || item.prompt || item.reason || '')}</p><button class="secondary-button" data-view="approvals">Review request</button></div>`).join('') : '<p class="empty-detail">Nothing needs your attention right now.</p>'}</section><section class="inspector-section"><h3>Recent activity</h3>${events.length ? events.map(eventHtml).join('') : '<p class="empty-detail">Activity will appear here as this agent works.</p>'}</section><section class="inspector-section"><h3>Mailbox</h3>${detailList([['Unread', agent.inbox_count ?? agent.unread_mail ?? agent.unread ?? 0], ['Pending approvals', approvals.length]])}${array(agent.inbox).length ? array(agent.inbox).map((message) => `<div class="detail-card"><h4>${escapeHtml(message.sender || 'Mailbox message')}</h4><p>${escapeHtml(message.text || '')}</p><p>${escapeHtml(timeLabel(message.created_at || message.sent_at))}</p></div>`).join('') : '<p class="empty-detail">The mailbox has no pending messages.</p>'}</section>`; }
  function channelInspector(channel) { return channel ? `<div class="inspector-agent"><span class="avatar neutral large">#</span><h3>${escapeHtml(channel.name || channel.id)}</h3><p>${escapeHtml(channel.description || 'Shared agent conversation')}</p></div>${detailList([['Channel ID', channel.id], ['Project', channel.project_id]])}<section class="inspector-section"><h3>Participants</h3>${channel.members === '*' ? '<span class="pill">All agents</span>' : tags(channel.members || [])}</section><details class="raw-config"><summary>View channel configuration</summary><pre>${escapeHtml(JSON.stringify(channel, null, 2))}</pre></details>` : '<p class="empty-detail">Select an agent or channel to inspect it.</p>'; }
  function eventHtml(event) { const kind = event.kind || event.type || 'event'; const data = event.text || event.message || event.description || event.detail || event.payload; return `<article class="event-row"><span class="event-icon">${icon(/approval/.test(kind) ? 'shield' : /message/.test(kind) ? 'message' : 'activity')}</span><div class="event-copy"><h3>${escapeHtml(event.title || kind.replace(/[_\.]/g, ' '))}</h3><p>${escapeHtml(typeof data === 'object' ? JSON.stringify(data) : data || event.agent || event.agent_id || '')}</p></div><time>${escapeHtml(timeLabel(event.created_at || event.at || event.timestamp))}</time></article>`; }
  function renderActivity() { const events = array(state.snapshot.events); $('activity-view').innerHTML = `<h2>While you’re away.</h2><p class="view-subtitle">A shared timeline of your agents and the work they’re doing.</p>${events.length ? events.slice(0, 150).map(eventHtml).join('') : `<div class="empty-state" style="min-height:55%"><span class="avatar neutral">${icon('activity')}</span><h2>Room for what comes next.</h2><p>Messages, session updates, and agent events will appear here as work happens.</p></div>`}`; }
  function renderApprovals() { const approvals = pendingApprovals(), signature = JSON.stringify(approvals); if ($('approvals-view').dataset.signature === signature) return; $('approvals-view').dataset.signature = signature; $('approvals-view').innerHTML = `<h2>Needs your attention.</h2><p class="view-subtitle">Review the details, then let your agents know how to proceed.</p>${approvals.length ? approvals.map((item) => { const agent = agentById(item.agent || item.agent_id || item.requester), body = item.summary || item.description || item.prompt || item.reason || item.request || item.payload || ''; return `<article class="approval-card" data-approval="${escapeHtml(item.id)}"><div class="approval-top">${avatar(label(agent), agent?.id || item.id)}<div><strong>${escapeHtml(agent ? label(agent) : item.agent || item.agent_id || 'Agent')}</strong><p>${escapeHtml(timeLabel(item.created_at || item.requested_at))} · Awaiting your decision</p></div><span class="pill">Approval</span></div><h3>${escapeHtml(item.title || item.action || item.kind || 'Can I go ahead?')}</h3><div class="message-body">${markdown(typeof body === 'string' ? body : JSON.stringify(body, null, 2))}</div><div class="approval-actions"><button class="secondary-button" data-decision="reject">Decline</button>${array(item.options).length ? array(item.options).map((option) => `<button class="solid-button" data-decision="approve" data-value="${escapeHtml(option.value)}">${escapeHtml(option.label || option.value)}</button>`).join('') : '<button class="solid-button" data-decision="approve">Approve</button>'}</div></article>`; }).join('') : `<div class="empty-state" style="min-height:55%"><span class="avatar neutral">${icon('check')}</span><h2>You’re all caught up.</h2><p>Your agents will bring decisions here when they need your attention.</p></div>`}`; }

  async function poll() {
    if (state.polling || document.hidden) return; state.polling = true;
    try { const snapshot = await api('/api/state'); state.snapshot = snapshot; state.agents = array(snapshot.agents); state.channels = array(snapshot.channels); state.interfaces = array(snapshot.interfaces);
      if (!state.selected || state.selected.type === 'agent' && !agentById(state.selected.id) || state.selected.type === 'channel' && !state.channels.some((channel) => channel.id === state.selected.id)) {
        let saved = null; try { saved = JSON.parse(sessionStorage.getItem('teamctl.selection')); } catch { /* Optional preference. */ }
        state.selected = saved && (saved.type === 'agent' ? agentById(saved.id) : state.channels.some((channel) => channel.id === saved.id)) ? saved : state.agents[0] ? { type: 'agent', id: state.agents[0].id } : null; state.messageSignature = '';
      }
      setConnection(true); renderRoster(); renderHeader(); renderInspector(); if (state.view === 'activity') renderActivity(); if (state.view === 'approvals') renderApprovals();
      if (state.view === 'chat') await loadMessages(); if (!$('terminal-panel').hidden) await loadTerminal().catch(showTerminalError);
    } catch (error) { setConnection(false, error.message.startsWith('The workspace') ? error.message : undefined); if (!state.agents.length) $('messages').innerHTML = `<div class="empty-state"><span class="avatar neutral">${icon('lock')}</span><h2>Let’s get connected.</h2><p>${escapeHtml(error.message)}<br>Keep <code>teamctl serve</code> running and open its workspace link.</p><button class="secondary-button" data-reconnect>Reconnect</button></div>`; }
    finally { state.polling = false; }
  }
  function showError(error) { toast(error.message || 'Something went wrong. Please try again.'); }
  function updateSendButton() { $('send-button').disabled = !state.selected || state.pending || state.attachments.some((item) => item.uploading || item.failed) || (!($('message-input').value.trim()) && !state.attachments.length); }
  function fitComposer() { $('message-input').style.height = 'auto'; $('message-input').style.height = `${Math.min($('message-input').scrollHeight, 180)}px`; updateSendButton(); }
  function renderAttachments() { $('draft-attachments').innerHTML = state.attachments.map((item) => `<span class="draft-attachment${item.failed ? ' failed' : ''}">${icon(item.uploading ? 'upload' : 'file')}<span>${escapeHtml(item.name)}${item.uploading ? ' · Uploading…' : item.failed ? ' · Failed' : ''}</span><button type="button" class="icon-button" data-remove-attachment="${escapeHtml(item.localId)}" aria-label="Remove ${escapeHtml(item.name)}">${icon('close')}</button></span>`).join(''); updateSendButton(); }
  async function uploadFiles(files) {
    if (!state.selected) return toast('Choose an agent before adding files.');
    if (state.selected.type === 'channel') return toast('Channel conversations support text. Send files directly to an agent.');
    if (state.attachments.length + files.length > 10) return toast('Attach up to 10 files in one message.');
    const max = Number(state.snapshot.web?.max_upload_bytes || state.snapshot.cache?.max_upload_bytes || 25 * 1024 * 1024);
    // Reserve the complete batch in its original draft before any await. Later
    // chat selections and chip removals must not redirect the remaining files.
    const targetAttachments = state.attachments, uploads = [];
    for (const file of files) {
      if (file.size > max) { toast(`${file.name} exceeds the ${bytes(max)} upload limit.`); continue; }
      const item = { localId: `${Date.now()}-${Math.random()}`, name: file.name || 'Voice message', uploading: true };
      targetAttachments.push(item); uploads.push({ file, item });
    }
    renderAttachments();
    for (const { file, item } of uploads) {
      try {
        const asset = await api('/api/assets', { method: 'POST', headers: { 'Content-Type': file.type || 'application/octet-stream', 'X-File-Name': encodeURIComponent(item.name) }, body: file });
        Object.assign(item, asset, { uploading: false });
      } catch (error) { item.uploading = false; item.failed = true; showError(error); }
      renderAttachments();
    }
  }
  async function sendMessage(event) {
    event?.preventDefault();
    if ($('send-button').disabled) return;
    const selected = { ...state.selected }, key = selectionKey(), draftText = $('message-input').value;
    const text = draftText.trim(), attachments = [...state.attachments], attachmentIds = attachments.map((item) => item.id);
    const signature = JSON.stringify([text, attachmentIds]), previousRequest = state.sendRequests.get(key);
    const request = previousRequest?.signature === signature ? previousRequest : { signature, id: crypto.randomUUID() };
    state.sendRequests.set(key, request);
    state.pending = true; updateSendButton(); $('compose-status').textContent = 'Sending to your agent…';
    try {
      await api('/api/messages', { method: 'POST', body: JSON.stringify({ [selected.type]: selected.id, text, attachments: attachmentIds, request_id: request.id }) });
      // The POST receipt acknowledges delivery. A later history refresh failure
      // must never turn a sent message back into a retryable, unsent draft.
      state.sendRequests.delete(key);
      const submitted = new Set(attachments.map((item) => item.localId));
      const remaining = (items) => items.filter((item) => !submitted.has(item.localId));
      if (selectionKey() === key) {
        if ($('message-input').value === draftText) $('message-input').value = '';
        state.attachments = remaining(state.attachments); renderAttachments(); fitComposer();
      } else {
        const saved = state.drafts.get(key);
        if (saved) {
          const updated = { text: saved.text === draftText ? '' : saved.text, attachments: remaining(saved.attachments || []) };
          if (updated.text || updated.attachments.length) state.drafts.set(key, updated); else state.drafts.delete(key);
        }
      }
      $('compose-status').textContent = 'Message delivered to the workspace.';
      if (selectionKey() === key) {
        try {
          await loadMessages();
          if (selectionKey() === key) $('messages').scrollTop = $('messages').scrollHeight;
        } catch {
          $('compose-status').textContent = 'Message sent. Conversation history is reconnecting…';
          toast('Message sent. History will refresh when the connection returns.');
        }
      }
    } catch (error) {
      showError(error); $('compose-status').textContent = 'Message not sent. Your draft is still here.';
    } finally {
      state.pending = false; updateSendButton();
      setTimeout(() => { if (!state.pending) $('compose-status').textContent = 'Your agents keep working when you close this page.'; }, 4000);
    }
  }
  async function toggleRecording() {
    if (state.recording) { state.recording.stop(); return; }
    if (!selectedAgent()) return toast('Voice messages can be sent directly to an agent.');
    if (!navigator.mediaDevices?.getUserMedia || !window.MediaRecorder) return toast('Voice recording is unavailable in this browser. You can attach an audio file instead.');
    let stream;
    try { stream = await navigator.mediaDevices.getUserMedia({ audio: true }); const type = ['audio/webm;codecs=opus', 'audio/mp4', 'audio/webm'].find((candidate) => MediaRecorder.isTypeSupported(candidate)); const recorder = new MediaRecorder(stream, type ? { mimeType: type } : {}), chunks = []; state.recording = recorder;
      const started = Date.now(); $('recording-status').hidden = false; $('composer-interface').hidden = true; $('voice-button').classList.add('recording'); $('voice-button').setAttribute('aria-label', 'Stop recording voice message'); $('voice-button').innerHTML = '<span aria-hidden="true">■</span>';
      const update = () => { const seconds = Math.floor((Date.now() - started) / 1000); $('recording-status').textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')} · Tap to finish`; if (seconds >= 600 && recorder.state === 'recording') recorder.stop(); }; update(); state.recordTimer = setInterval(update, 1000);
      recorder.ondataavailable = (event) => { if (event.data.size) chunks.push(event.data); };
      recorder.onstop = () => { const mime = recorder.mimeType || 'audio/webm'; stream.getTracks().forEach((track) => track.stop()); clearInterval(state.recordTimer); state.recording = null; $('recording-status').hidden = true; $('composer-interface').hidden = false; $('voice-button').classList.remove('recording'); $('voice-button').setAttribute('aria-label', 'Record a voice message'); $('voice-button').innerHTML = icon('mic'); if (chunks.length) uploadFiles([new File(chunks, `voice-${new Date().toISOString().replace(/[:.]/g, '-')}.${mime.includes('mp4') ? 'm4a' : 'webm'}`, { type: mime })]); };
      recorder.onerror = () => { toast('Voice recording interrupted. Please try again.'); if (recorder.state !== 'inactive') recorder.stop(); }; recorder.start();
    } catch (error) { stream?.getTracks().forEach((track) => track.stop()); toast(error.name === 'NotAllowedError' ? 'Microphone access was declined. You can attach an audio file instead.' : 'Couldn’t start the microphone. Please try again.'); }
  }
  function toggleTerminal(force) { const open = force ?? $('terminal-panel').hidden; $('terminal-panel').hidden = !open; $('terminal-toggle').setAttribute('aria-pressed', String(open)); if (open && selectedAgent()) { loadTerminal().catch(showTerminalError); $('terminal-input').focus(); } }
  async function loadTerminal() { const agent = selectedAgent(); if (!agent || $('terminal-panel').hidden) return; const result = await api(`/api/agents/${encodeURIComponent(agent.id)}/terminal?mode=${state.terminalMode}`); if (selectedAgent()?.id !== agent.id) return; const output = String(result.output ?? result.text ?? 'Session has no output yet.'); if ($('terminal-output').textContent !== output) { $('terminal-output').textContent = output; if ($('terminal-autoscroll').checked) $('terminal-output').scrollTop = $('terminal-output').scrollHeight; } const interactive = result.connected !== false && result.interactive !== false; $('terminal-input').disabled = !interactive; $('terminal-input').placeholder = interactive ? 'Type directly into this session…' : state.terminalMode === 'shell' ? 'This shell has exited. Reconnect to open a new shell…' : 'Use chat, or choose Workspace shell to type commands…'; $('terminal-connect').hidden = state.terminalMode !== 'shell' || interactive; $('terminal-form').querySelector('button').disabled = !interactive; document.querySelectorAll('[data-terminal-key]').forEach((button) => { button.disabled = !interactive; }); $('terminal-status').textContent = !interactive ? state.terminalMode === 'shell' ? '· Disconnected' : '· Read only' : state.terminalMode === 'shell' ? '· Connected' : `· ${runtimeOf(agent)}`; }
  function showTerminalError(error) { $('terminal-status').textContent = '· Unavailable'; $('terminal-output').textContent = `${error.message}\n\nStart your agents with teamctl up to connect to their sessions.`; }
  async function terminalInput(input, key) { const agent = selectedAgent(); if (!agent) return; await api(`/api/agents/${encodeURIComponent(agent.id)}/terminal`, { method: 'POST', body: JSON.stringify({ input, mode: state.terminalMode, ...(key ? { key } : {}) }) }); setTimeout(() => loadTerminal().catch(showTerminalError), 250); }
  function toggleInspector(force) { const open = force ?? $('inspector').hidden; $('inspector').hidden = !open; $('inspector-toggle').setAttribute('aria-pressed', String(open)); if (open) { state.inspectorSignature = ''; renderInspector(); } }
  function syncSidebarAccess() {
    const mobile = innerWidth <= 680, open = mobile && $('workspace').classList.contains('sidebar-open');
    const hidden = mobile && !open;
    $('sidebar').inert = hidden; $('sidebar').setAttribute('aria-hidden', String(hidden));
    $('main').inert = open; $('sidebar-scrim').hidden = !open;
  }
  function closeSidebar() {
    const restore = innerWidth <= 680 && ($('sidebar').contains(document.activeElement) || $('main').inert);
    $('workspace').classList.remove('sidebar-open'); syncSidebarAccess();
    if (restore) {
      const opener = state.sidebarOpener;
      (opener?.isConnected && !opener.closest('[hidden],[inert]') ? opener : $('open-sidebar')).focus();
    }
  }
  function openSidebar() {
    state.sidebarOpener = document.activeElement;
    $('workspace').classList.add('sidebar-open'); syncSidebarAccess();
    if (innerWidth <= 680) $('agent-search').focus();
  }
  function toggleChatSearch(force) { $('chat-search-bar').hidden = !(force ?? $('chat-search-bar').hidden); if (!$('chat-search-bar').hidden) $('message-search').focus(); else { $('message-search').value = ''; renderMessages(); } }

  document.addEventListener('click', async (event) => {
    const button = event.target.closest('button'); if (!button) return;
    if (button.dataset.agent) switchSelection('agent', button.dataset.agent);
    if (button.dataset.channel) switchSelection('channel', button.dataset.channel);
    if (button.dataset.view) setView(button.dataset.view);
    if (button.dataset.suggestion) { $('message-input').value = button.dataset.suggestion; fitComposer(); $('message-input').focus(); }
    if (button.dataset.inspectorTab) { state.inspectorTab = button.dataset.inspectorTab; state.inspectorSignature = ''; renderInspector(); }
    if (button.dataset.removeAttachment) { state.attachments = state.attachments.filter((item) => item.localId !== button.dataset.removeAttachment); renderAttachments(); }
    if (button.dataset.terminalKey) { try { await terminalInput('', button.dataset.terminalKey); } catch (error) { showError(error); } }
    if (button.hasAttribute('data-load-older')) loadOlderMessages();
    if (button.hasAttribute('data-reconnect')) { if (!token) $('auth-dialog').showModal(); else poll(); }
    if (button.dataset.decision) { const card = button.closest('[data-approval]'); card.querySelectorAll('button').forEach((item) => { item.disabled = true; }); try { await api(`/api/approvals/${encodeURIComponent(card.dataset.approval)}`, { method: 'POST', body: JSON.stringify({ decision: button.dataset.decision, ...(button.dataset.value ? { value: button.dataset.value } : {}) }) }); toast(button.dataset.decision === 'approve' ? 'Approved. Your agent can continue.' : 'Declined. Your agent has been notified.'); await poll(); } catch (error) { showError(error); card.querySelectorAll('button').forEach((item) => { item.disabled = false; }); } }
  });
  $('agent-health-details').addEventListener('click', () => { state.inspectorTab = 'overview'; toggleInspector(true); });
  $('agent-search').addEventListener('input', renderRoster); $('message-search').addEventListener('input', () => renderMessages(true)); $('message-input').addEventListener('input', fitComposer);
  $('message-input').addEventListener('keydown', (event) => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); sendMessage(); } });
  $('composer').addEventListener('submit', sendMessage); $('attach-button').addEventListener('click', () => $('file-input').click()); $('file-input').addEventListener('change', (event) => { uploadFiles([...event.target.files]); event.target.value = ''; }); $('voice-button').addEventListener('click', toggleRecording);
  $('message-input').addEventListener('paste', (event) => { const files = [...(event.clipboardData?.files || [])]; if (files.length) { event.preventDefault(); uploadFiles(files); } });
  async function connectShell() {
    const agent = selectedAgent(); if (!agent) return;
    $('terminal-connect').disabled = true;
    try { await api(`/api/agents/${encodeURIComponent(agent.id)}/terminal`, { method: 'POST', body: JSON.stringify({ mode: 'shell', connect: true, input: '' }) }); await loadTerminal(); }
    catch (error) { showTerminalError(error); }
    finally { $('terminal-connect').disabled = false; }
  }
  $('terminal-connect').addEventListener('click', connectShell);
  $('terminal-mode').addEventListener('change', async () => {
    state.terminalMode = $('terminal-mode').value; $('terminal-output').textContent = 'Connecting to session…';
    if (state.terminalMode === 'shell') await connectShell(); else await loadTerminal().catch(showTerminalError);
  });
  $('terminal-toggle').addEventListener('click', () => toggleTerminal()); $('terminal-close').addEventListener('click', () => toggleTerminal(false)); $('terminal-form').addEventListener('submit', async (event) => { event.preventDefault(); const input = $('terminal-input').value; try { await terminalInput(input, 'Enter'); $('terminal-input').value = ''; } catch (error) { showError(error); } });
  $('inspector-toggle').addEventListener('click', () => toggleInspector()); $('inspector-close').addEventListener('click', () => toggleInspector(false)); $('open-sidebar').addEventListener('click', openSidebar); $('close-sidebar').addEventListener('click', closeSidebar); $('sidebar-scrim').addEventListener('click', closeSidebar);
  $('chat-search-toggle').addEventListener('click', () => toggleChatSearch()); $('close-chat-search').addEventListener('click', () => toggleChatSearch(false)); $('jump-latest').addEventListener('click', () => { $('messages').scrollTo({ top: $('messages').scrollHeight, behavior: 'smooth' }); $('jump-latest').hidden = true; }); $('messages').addEventListener('scroll', () => { if ($('messages').scrollHeight - $('messages').scrollTop - $('messages').clientHeight < 100) $('jump-latest').hidden = true; });
  $('auth-dialog').addEventListener('cancel', (event) => { event.preventDefault(); }); $('auth-form').addEventListener('submit', async (event) => { event.preventDefault(); token = $('access-token').value.trim(); if (!token) return; $('auth-error').textContent = ''; try { await api('/api/state'); try { sessionStorage.setItem('teamctl.access-token', token); } catch { /* Keep memory token. */ } $('auth-dialog').close(); $('access-token').value = ''; poll(); } catch (error) { $('auth-error').textContent = error.message; } });
  document.addEventListener('keydown', (event) => { const command = event.metaKey || event.ctrlKey; if (command && event.key.toLowerCase() === 'k') { event.preventDefault(); if (innerWidth <= 680) openSidebar(); $('agent-search').focus(); } if (command && event.key.toLowerCase() === 'f' && state.view === 'chat') { event.preventDefault(); toggleChatSearch(true); } if (command && event.key === 'Enter') { event.preventDefault(); sendMessage(); } if (event.key === 'Escape' && !$('auth-dialog').open) { closeSidebar(); toggleInspector(false); toggleChatSearch(false); } });
  let dragDepth = 0; document.addEventListener('dragenter', (event) => { if ([...(event.dataTransfer?.types || [])].includes('Files')) { event.preventDefault(); dragDepth++; if (state.view === 'chat' && state.selected) $('drop-overlay').hidden = false; } }); document.addEventListener('dragover', (event) => { if ([...(event.dataTransfer?.types || [])].includes('Files')) event.preventDefault(); }); document.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; $('drop-overlay').hidden = true; } }); document.addEventListener('drop', (event) => { const files = [...(event.dataTransfer?.files || [])]; if (files.length) { event.preventDefault(); if (state.view === 'chat') uploadFiles(files); } dragDepth = 0; $('drop-overlay').hidden = true; });
  window.addEventListener('message', (event) => { if (event.data?.type !== 'teamctl-component-size' || !Number.isFinite(event.data.height)) return; const frame = [...$('messages').querySelectorAll('iframe[data-component]')].find((candidate) => candidate.contentWindow === event.source); if (frame) frame.style.height = `${Math.max(150, Math.min(700, event.data.height))}px`; });
  document.addEventListener('visibilitychange', () => { if (!document.hidden) poll(); }); window.addEventListener('pagehide', () => { state.recording?.stop(); state.blobUrls.forEach((item) => URL.revokeObjectURL(item.url)); });
  window.addEventListener('resize', syncSidebarAccess); syncSidebarAccess();
  if (!token) $('auth-dialog').showModal();
  poll(); setInterval(poll, 2500);
})();
