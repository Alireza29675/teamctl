export const MAX_FRAME_BYTES = 256 * 1024;
export const MAX_TEXT_BYTES = 64 * 1024;
export const MESSAGE_KINDS = new Set([
  'text', 'image', 'file', 'audio', 'video', 'voice', 'media', 'reaction', 'typing', 'media_error',
]);
const CONNECTION_STATES = new Set([
  'starting', 'connecting', 'connected', 'disconnected', 'pairing_required', 'auth_failed', 'failed',
]);

export class AdapterError extends Error {
  constructor(code, message, retryable = false) {
    super(message);
    this.name = 'AdapterError';
    this.code = code;
    this.retryable = retryable;
  }
}

export function invalid() {
  return new AdapterError('protocol_error', 'Invalid adapter protocol frame');
}

export function object(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid();
  return value;
}

function fields(value, allowed) {
  if (Object.keys(value).some((name) => !allowed.includes(name))) throw invalid();
}

export function identity(value) {
  if (typeof value !== 'string' || !value.isWellFormed() || !value.trim() || value !== value.trim()
    || /\p{Cc}/u.test(value) || Buffer.byteLength(value) > 1024) throw invalid();
  return value;
}

export function text(value, { empty = false } = {}) {
  if (typeof value !== 'string' || !value.isWellFormed() || (!empty && value.length === 0)
    || Buffer.byteLength(value) > MAX_TEXT_BYTES || value.includes('\0')) throw invalid();
  return value;
}

function optionalIdentity(value) {
  return value == null ? null : identity(value);
}

export function normalizedMessage(value) {
  object(value);
  fields(value, ['text', 'kind', 'payload', 'reply_to_external_id']);
  const kind = value.kind ?? 'text';
  if (!MESSAGE_KINDS.has(kind)) throw invalid();
  if (value.payload != null && Buffer.byteLength(JSON.stringify(value.payload)) > MAX_FRAME_BYTES / 2) throw invalid();
  return {
    text: text(value.text, { empty: kind !== 'text' }),
    kind,
    payload: value.payload ?? null,
    reply_to_external_id: optionalIdentity(value.reply_to_external_id),
  };
}

export function inbound(value) {
  object(value);
  fields(value, ['type', 'external_id', 'peer_id', 'sender_id', 'text', 'kind', 'payload',
    'title', 'reply_to_external_id']);
  const message = normalizedMessage({ text: value.text, kind: value.kind,
    payload: value.payload, reply_to_external_id: value.reply_to_external_id });
  if (['reaction', 'typing'].includes(message.kind)) throw invalid();
  const event = {
    type: 'message',
    external_id: identity(value.external_id),
    peer_id: identity(value.peer_id),
    sender_id: identity(value.sender_id),
    ...message,
  };
  if (value.title != null) event.title = identity(value.title);
  encoded(event);
  return event;
}

export function hostFrame(value) {
  object(value);
  switch (value.type) {
    case 'initialize':
      fields(value, ['type', 'version', 'instance_id', 'state_dir', 'mode', 'config']);
      if (value.version !== 1 || !['run', 'pair'].includes(value.mode)
        || typeof value.state_dir !== 'string') throw invalid();
      identity(value.instance_id);
      object(value.config);
      return value;
    case 'send':
      fields(value, ['type', 'request_id', 'peer_id', 'message']);
      return { type: 'send', request_id: identity(value.request_id),
        peer_id: identity(value.peer_id), message: normalizedMessage(value.message) };
    case 'accepted':
    case 'rejected':
      fields(value, value.type === 'accepted' ? ['type', 'peer_id', 'external_id']
        : ['type', 'peer_id', 'external_id', 'code']);
      identity(value.peer_id);
      identity(value.external_id);
      if (value.type === 'rejected') identity(value.code);
      return value;
    case 'shutdown':
      fields(value, ['type']);
      return { type: 'shutdown' };
    default: throw invalid();
  }
}

export function connectionState(state) {
  if (!CONNECTION_STATES.has(state)) throw invalid();
  return state;
}

export function encoded(value) {
  const body = JSON.stringify(value);
  if (Buffer.byteLength(body) + 1 > MAX_FRAME_BYTES) throw invalid();
  return `${body}\n`;
}

export async function* readFrames(input) {
  let pending = Buffer.alloc(0);
  for await (const chunk of input) {
    const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    let offset = 0;
    while (offset < bytes.length) {
      const newline = bytes.indexOf(10, offset);
      const end = newline === -1 ? bytes.length : newline;
      if (pending.length + end - offset + 1 > MAX_FRAME_BYTES) throw invalid();
      pending = Buffer.concat([pending, bytes.subarray(offset, end)]);
      if (newline === -1) break;
      let value;
      try {
        value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(pending));
      } catch { throw invalid(); }
      pending = Buffer.alloc(0);
      yield hostFrame(value);
      offset = newline + 1;
    }
  }
  if (pending.length !== 0) throw invalid();
}
