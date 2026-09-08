'use strict';

// Wire evidence: tui_gateway/{agent_callbacks,tool_progress,session_history,
// prompt_turn,event_replay}.py and Desktop gateway-event/{message-stream,
// tools,input-requests,status,session-info}.ts. No renderer/provider dependency.
// normalizeHistory returns items. reduceEvent owns one session's snapshot.
// info._transcript is reserved, serializable reducer bookkeeping (no globals).
const { createHash } = require('node:crypto');
const { splitImageReferences } = require('./hermes-media.cjs');
const MAX_ITEMS = 2000, MAX_TEXT = 65536, MAX_REQUESTS = 64;
const record = v => v !== null && typeof v === 'object' && !Array.isArray(v);
const str = v => typeof v === 'string' ? v.slice(0, MAX_TEXT) : '';
const identity = v => typeof v === 'string' || typeof v === 'number' ? String(v).slice(0, 512) : '';
const defined = (...values) => values.find(v => v !== undefined);

function copy(value, depth = 0, seen = new WeakSet(), budget = { n: 8000 }) {
  if (--budget.n < 0 || depth > 12) return '[truncated]';
  if (typeof value === 'string') return str(value);
  if (value == null || typeof value === 'boolean') return value;
  if (typeof value === 'number') return Number.isFinite(value) ? value : null;
  if (typeof value !== 'object') return undefined;
  if (seen.has(value)) return '[circular]';
  seen.add(value);
  const out = Array.isArray(value) ? [] : {};
  for (const key of Object.keys(value).slice(0, 512)) {
    if (['__proto__', 'constructor', 'prototype', 'rendered'].includes(key)) continue;
    out[key] = copy(value[key], depth + 1, seen, budget);
  }
  seen.delete(value);
  return out;
}
function parsed(value) {
  if (typeof value === 'string' && value.length <= MAX_TEXT) {
    try { return copy(JSON.parse(value)); } catch { /* retain malformed arguments */ }
  }
  return copy(value);
}
function text(value, depth = 0) {
  if (depth > 12) return '';
  if (typeof value === 'string') return str(value);
  if (Array.isArray(value)) return str(value.slice(0, 512).map(v => text(v, depth + 1)).join(''));
  if (!record(value)) return '';
  // Encrypted reasoning/signatures are never displayed as reasoning text.
  return text(value.text ?? value.thinking ?? value.summary ?? value.content, depth + 1);
}
function reasoning(m) {
  return [...new Set(['reasoning', 'reasoning_content', 'reasoning_details', 'codex_reasoning_items']
    .map(k => text(m[k])).filter(Boolean))].join('\n').slice(0, MAX_TEXT);
}
function category(name, input) {
  if (name === 'memory') return 'memory';
  if (['skills_list', 'skill_view', 'skill_manage'].includes(name)) return 'skills';
  if (['terminal', 'execute_code', 'process'].includes(name)) return 'terminal';
  if (['read_file', 'write_file', 'search_files', 'patch'].includes(name)) {
    const path = str(input?.path ?? input?.file_path);
    if (/(^|\/)SKILL\.md$/.test(path)) return 'skills';
    return 'files';
  }
  if (['web_search', 'web_extract'].includes(name)) return 'web';
  if (['todo', 'todo_list'].includes(name)) return 'todo';
  if (['delegate_task'].includes(name)) return 'subagent';
  return 'other';
}
const callId = p => identity(p.tool_id ?? p.tool_call_id ?? p.call_id ?? p.id);
function tool(p, fallback, status) {
  const fn = record(p.function) ? p.function : {};
  const name = str(p.name ?? p.tool_name ?? fn.name) || 'tool';
  const input = parsed(defined(p.args, p.arguments, p.input, fn.arguments, p.args_text));
  const output = parsed(defined(p.result, p.output, p.result_text));
  return { id: callId(p) || fallback, kind: 'tool', name,
    ...(input !== undefined ? { input } : {}), ...(output !== undefined ? { output } : {}),
    ...(text(p.summary ?? p.preview ?? p.context) ? { text: text(p.summary ?? p.preview ?? p.context) } : {}),
    status: p.is_error || output?.is_error || output?.success === false || output?.error ? 'error' : status,
    category: category(name, input) };
}

// Hash the bounded, sanitized message with canonical key ordering. Row identity
// wins; fallback identity is independent of page position and object key order.
// Exact duplicate ID-less rows need an occurrence ordinal. Across separate pages
// of indistinguishable duplicates, a stateless API cannot infer that ordinal;
// callers need source row IDs (or normalize the combined history) for that case.
function historyHash(message) {
  const canonical = value => Array.isArray(value) ? value.map(canonical)
    : record(value) ? Object.fromEntries(Object.keys(value).sort().map(k => [k, canonical(value[k])])) : value;
  return createHash('sha256').update(JSON.stringify(canonical(message))).digest('hex');
}

function normalizeHistory(messages) {
  const items = [], calls = new Map(), occurrences = new Map();
  if (!Array.isArray(messages)) return items;
  const add = item => { items.push(item); if (items.length > MAX_ITEMS) items.shift(); };
  for (const raw of messages.slice(-MAX_ITEMS)) {
    if (!record(raw) || raw.display_kind === 'hidden') continue;
    const m = copy(raw), role = m.role;
    const rowId = identity(raw.row_id ?? raw._row_id ?? raw.id);
    let base;
    if (rowId) base = `history:row:${encodeURIComponent(rowId)}`;
    else {
      const hash = historyHash(m), occurrence = occurrences.get(hash) || 0;
      occurrences.set(hash, occurrence + 1);
      base = `history:hash:${hash}:${occurrence}`;
    }
    let part = 0;
    const emit = (kind, value) => {
      if (!value) return;
      if (kind === 'text' && role === 'user') {
        for (const item of splitImageReferences(value))
          add({ id: `${base}:${part++}`, role, ...item });
      } else add({ id: `${base}:${part++}`, kind, role, text: value });
    };
    const putTool = (p, result = false) => {
      const id = callId(p), old = id && calls.get(id);
      if (old && result) {
        old.output = parsed(defined(p.result, p.output, p.content));
        old.status = p.is_error || old.output?.error || old.output?.success === false ? 'error' : 'complete';
        return;
      }
      const item = tool(result ? { ...p, result: defined(p.result, p.output, p.content) } : p,
        `${base}:${part++}`, result ? 'complete' : 'pending');
      add(item); if (id) calls.set(id, item);
    };
    if (role === 'tool') { putTool(m, true); continue; }
    if (!['user', 'assistant', 'system'].includes(role)) {
      add({ id: base, kind: 'activity', name: str(m.type) || 'history', output: m }); continue;
    }
    emit('reasoning', reasoning(m));
    const content = m.content ?? m.text;
    const parts = Array.isArray(content) ? content : [content];
    for (const p of parts) {
      if (record(p) && ['tool_use', 'function_call'].includes(p.type)) putTool(p);
      else if (record(p) && ['tool_result', 'function_call_output'].includes(p.type)) {
        putTool({ ...p, tool_call_id: p.tool_use_id ?? p.call_id }, true);
      } else if (record(p) && ['thinking', 'reasoning', 'reasoning.text', 'summary_text'].includes(p.type)) {
        const actual = text(p); if (actual !== reasoning(m)) emit('reasoning', actual);
      } else {
        const kind = role === 'system' || (m.display_kind && m.display_kind !== 'skill_invocation') ? 'notice' : 'text';
        emit(kind, text(p));
      }
    }
    if (Array.isArray(m.tool_calls)) for (const call of m.tool_calls) if (record(call)) putTool(call);
  }
  return items;
}

function reduceEvent(state, event) {
  state = record(state) ? state : {};
  // Existing snapshots are immutable by contract. Sanitize/copy new wire data
  // only; unchanged history and large tool payloads are shared, never traversed.
  const boundedRecords = (value, max) => !Array.isArray(value) ? []
    : value.length <= max && value.every(record) ? value : value.slice(-max).filter(record);
  const s = { items: boundedRecords(state.items, MAX_ITEMS),
    status: str(state.status) || 'idle', info: record(state.info) ? state.info : {},
    usage: record(state.usage) ? state.usage : {},
    requests: boundedRecords(state.requests, MAX_REQUESTS) };
  const originalItems = s.items;
  const ownItems = () => { if (s.items === originalItems) s.items = s.items.slice(); };
  const update = (item, patch) => {
    if (Object.keys(patch).every(k => Object.is(item[k], patch[k]))) return item;
    const index = s.items.at(-1) === item ? s.items.length - 1 : s.items.indexOf(item);
    if (index < 0) return item;
    ownItems();
    return (s.items[index] = { ...item, ...patch });
  };
  const e = record(event?.params) && event.method === 'event' ? event.params : record(event) ? event : {};
  const type = str(e.type ?? e.event) || 'unknown';
  const p = record(e.payload) ? copy(e.payload) : record(e.data) ? copy(e.data) : copy(e);
  const originalMeta = s.info._transcript;
  const meta = record(originalMeta) ? { ...originalMeta } : { next: 0, turn: [] };
  meta.next = Number.isSafeInteger(meta.next) && meta.next >= 0 ? meta.next : 0;
  meta.turn = Array.isArray(meta.turn) ? (meta.turn.length > MAX_ITEMS ? meta.turn.slice(-MAX_ITEMS) : meta.turn) : [];
  const seqKey = `${identity(e.session_id)}:${identity(e.epoch ?? e.replay_epoch)}`;
  if (Number.isSafeInteger(e.seq) && e.seq > 0) {
    if (meta.seqKey === seqKey && e.seq <= meta.seq) return state;
    meta.seqKey = seqKey; meta.seq = e.seq;
  }
  const add = item => {
    item = { id: `event:${++meta.next}`, ...item };
    ownItems(); s.items.push(item); meta.turn = [...meta.turn, item.id]; turnIds = undefined; return item;
  };
  const find = id => s.items.at(-1)?.id === id ? s.items.at(-1) : s.items.find(i => i.id === id);
  let turnIds;
  const inTurn = item => (turnIds ||= new Set(meta.turn)).has(item.id);
  const last = kind => s.items.findLast(i => inTurn(i) && i.kind === kind && i.role === 'assistant');
  const seal = () => { delete meta.text; delete meta.reasoning; };
  const write = (kind, value, full = false) => {
    value = text(value); if (!value) return;
    let item = find(meta[kind]);
    // Tokens separated by another visible part form separate ordered segments.
    if (!full && item !== s.items.at(-1)) item = undefined;
    if (full) {
      const segments = s.items.filter(i => inTurn(i) && i.kind === kind && i.role === 'assistant' && i.status === 'running');
      const streamed = segments.map(i => i.text).join('');
      if (streamed && value.startsWith(streamed)) {
        const tail = segments.at(-1);
        update(tail, { text: str(tail.text + value.slice(streamed.length)) });
        meta[kind] = tail.id;
        return;
      }
    }
    if (full && !item) {
      const previous = last(kind);
      if (previous?.text === value) return;
      if (previous?.status === 'running' && value.startsWith(previous.text)) item = previous;
    }
    if (!item) { item = add({ kind, role: 'assistant', text: '', status: 'running' }); meta[kind] = item.id; }
    update(item, { text: str(full ? value : item.text + value) });
  };
  const activity = (kind = 'activity', name = type) => add({ kind, name,
    text: text(p.text ?? p.message ?? p.summary ?? p.goal ?? p.preview), output: p });
  const finish = status => {
    s.status = status; if (s.requests.length) s.requests = [];
    for (const i of s.items) if (i.status === 'running' || i.status === 'pending') {
      update(i, { status: status === 'idle' ? (i.kind === 'tool' ? 'interrupted' : 'complete') : status });
    }
    seal(); meta.ended = true;
  };
  const mergeUsage = value => { if (record(value)) s.usage = { ...s.usage, ...copy(value) }; };
  const request = /^(approval|clarify|sudo|secret|mcp\.setup)\.(request|expire|resolved|respond|cancelled)$/.exec(type);
  if (request) {
    const [, kind, action] = request, id = identity(p.request_id);
    if (!id) activity('notice');
    else if (action === 'request') {
      // Never retain submitted credential fields in cards or snapshots.
      delete p.password; delete p.value; delete p.secret;
      const card = { id, kind, status: 'pending', input: p,
        text: text(p.question ?? p.prompt ?? p.description ?? p.command) };
      s.requests = s.requests.filter(r => !(r.id === id && r.kind === kind));
      s.requests.push(card); s.status = 'waiting';
    } else {
      s.requests = s.requests.filter(r => !(r.id === id && r.kind === kind));
      if (!s.requests.length && s.status === 'waiting') s.status = 'running';
    }
  } else if (type === 'message.start') {
    seal(); meta.turn = []; turnIds = undefined; meta.ended = false; s.status = 'running'; if (s.requests.length) s.requests = [];
  } else if (type === 'message.delta' || type === 'reasoning.delta') {
    if (!meta.ended) { write(type === 'message.delta' ? 'text' : 'reasoning', p.text); if (!s.requests.length) s.status = 'running'; }
  } else if (type === 'reasoning.available') {
    write('reasoning', p.text, true);
  } else if (type === 'message.interim') {
    const streamed = s.items.some(i => inTurn(i) && i.kind === 'text' && i.role === 'assistant' && i.status === 'running');
    if (p.already_streamed === false && !streamed && text(p.text)) {
      add({ kind: 'text', role: 'assistant', text: text(p.text), status: 'complete' });
    } else write('text', p.text, true);
    for (const item of s.items) {
      if (inTurn(item) && item.role === 'assistant' && item.status === 'running') update(item, { status: 'complete' });
    }
    seal();
  } else if (['message.complete', 'error', 'message.cancelled', 'message.canceled', 'cancelled'].includes(type)) {
    const status = type === 'error' || p.status === 'error' ? 'error'
      : /cancel/.test(type) || ['interrupted', 'cancelled', 'canceled'].includes(p.status) ? 'cancelled' : 'idle';
    if (!meta.ended) {
      if (p.reasoning) write('reasoning', p.reasoning, true);
      if (type === 'message.complete' && (status !== 'error' || p.partial)) write('text', p.text, true);
      if (status === 'error') add({ kind: 'notice', name: 'error', text: text(p.error ?? p.message ?? p.text), status: 'error', output: p });
      if (p.warning) add({ kind: 'notice', text: text(p.warning) });
      finish(status);
    }
    mergeUsage(p.usage);
  } else if (['tool.start', 'tool.complete', 'tool.progress', 'tool.error', 'tool.cancelled'].includes(type)) {
    seal();
    const id = callId(p);
    const old = id ? s.items.find(i => i.kind === 'tool' && i.id === id) : undefined;
    const status = type === 'tool.complete' ? 'complete' : type === 'tool.error' ? 'error' : type === 'tool.cancelled' ? 'cancelled' : 'running';
    const item = tool(p, `event:${meta.next + 1}`, status);
    if (old) {
      if (item.name === 'tool') item.name = old.name;
      item.category = category(item.name, item.input ?? old.input);
      if (type === 'tool.start' && old.status !== 'running') item.status = old.status;
      update(old, item);
    } else add(item);
    if (type === 'tool.start' && !meta.ended && !s.requests.length) s.status = 'running';
  } else if (type === 'session.info') {
    const { _transcript, ...info } = p;
    s.info = { ...s.info, ...info, _transcript: meta }; mergeUsage(p.usage);
    if (p.running === true && !s.requests.length) s.status = 'running';
    if (p.running === false && ['running', 'waiting'].includes(s.status)) finish('idle');
  } else if (type === 'session.resume_progress') {
    s.info = { ...s.info, resumePhase: str(p.phase), hydrating: p.status === 'loading' };
    if (p.status === 'failed') activity('notice');
  } else if (type === 'session.usage' || type === 'usage.update') {
    mergeUsage(p.usage ?? p);
  } else if (type === 'session.title') {
    if (s.info.title !== str(p.title)) s.info = { ...s.info, title: str(p.title) };
  } else if (type === 'todo.updated') {
    if (Array.isArray(p.todos) && Number.isFinite(p.revision) && p.revision >= (meta.todoRevision ?? -1)) {
      meta.todoRevision = p.revision;
      const item = s.items.find(i => i.id === 'todo:snapshot');
      const patch = { id: 'todo:snapshot', kind: 'todo', name: 'todos', output: p.todos, status: 'complete' };
      if (item) update(item, patch); else add(patch);
    } else activity();
  } else if (type === 'notification.show') {
    const id = `notice:${identity(p.key ?? p.id) || ++meta.next}`;
    const item = find(id), patch = { id, kind: 'notice', name: str(p.kind), text: text(p.text), status: str(p.level), output: p };
    if (item) update(item, patch); else add(patch);
  } else if (type === 'notification.clear') {
    s.items = s.items.filter(i => i.id !== `notice:${identity(p.key ?? p.id)}`);
  } else if (type.startsWith('subagent.')) {
    // Keep child identity/tool fields; a child's lifecycle must not end its parent.
    activity('activity');
  } else if (type === 'review.summary') {
    activity('notice');
  } else {
    // thinking.delta is spinner/status prose, NOT actual model reasoning.
    // Includes future memory/skill notifications: retain their real payload.
    activity();
  }
  // Prune only when the collection actually changes size. A token update only
  // copies the item-reference array and its changed text item (O(items), not
  // O(total transcript bytes)); meta.turn, requests, usage and payloads share.
  if (s.items.length > MAX_ITEMS) s.items = s.items.slice(-MAX_ITEMS);
  if (s.requests.length > MAX_REQUESTS) s.requests = s.requests.slice(-MAX_REQUESTS);
  if (s.items.length !== originalItems.length || s.items[0] !== originalItems[0]) {
    const ids = new Set(s.items.map(i => i.id));
    if (meta.turn.some(id => !ids.has(id))) meta.turn = meta.turn.filter(id => ids.has(id));
  }
  const sameMeta = record(originalMeta) && Object.keys(meta).length === Object.keys(originalMeta).length
    && Object.keys(meta).every(k => Object.is(meta[k], originalMeta[k]));
  const nextMeta = sameMeta ? originalMeta : meta;
  if (s.info._transcript !== nextMeta) s.info = { ...s.info, _transcript: nextMeta };
  return Object.keys(s).every(k => Object.is(s[k], state[k])) ? state : s;
}

module.exports = { normalizeHistory, reduceEvent };
