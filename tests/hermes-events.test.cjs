'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { normalizeHistory, reduceEvent } = require('../electron/agents/hermes-events.cjs');
const event = (type, payload = {}, extra = {}) => ({ type, payload, ...extra });
const run = (...events) => events.reduce(reduceEvent, undefined);
const start = () => event('message.start');
const freeze = value => {
  if (value && typeof value === 'object') { Object.freeze(value); Object.values(value).forEach(freeze); }
  return value;
};

test('raw REST history pairs snake_case call IDs, inputs, results and reasoning', () => {
  const input = freeze([
    { role: 'user', content: 'Inspect the example' },
    { role: 'assistant', reasoning_content: 'Check the file first.', content: 'Reading.',
      tool_calls: [{ id: 'call-1', function: { name: 'read_file', arguments: '{"path":"example.txt"}' } }] },
    { role: 'tool', tool_call_id: 'call-1', content: '{"content":"example"}' },
    { role: 'assistant', content: 'Done.' }
  ]);
  const result = normalizeHistory(input);
  assert.deepEqual(result.map(i => i.kind), ['text', 'reasoning', 'text', 'tool', 'text']);
  assert.deepEqual(result[3], { id: 'call-1', kind: 'tool', name: 'read_file',
    input: { path: 'example.txt' }, output: { content: 'example' }, status: 'complete', category: 'files' });
});

test('projected RPC history keeps supplied fields without inventing missing results', () => {
  const items = normalizeHistory([
    { role: 'user', row_id: 3, text: '/review example', display_kind: 'skill_invocation' },
    { role: 'tool', name: 'skill_view', args: { name: 'review' }, context: 'review' },
    { role: 'assistant', text: 'Ready', reasoning: 'Read the instructions.' }
  ]);
  assert.equal(items[1].category, 'skills');
  assert.deepEqual(items[1].input, { name: 'review' });
  assert.equal(Object.hasOwn(items[1], 'output'), false);
  assert.equal(items[2].text, 'Read the instructions.');
  assert.deepEqual(normalizeHistory([{ role: 'tool', name: 'terminal', args: {}, result: 'ok' }])[0].output, 'ok');
});

test('structured content preserves block order, reasoning and tool results', () => {
  const items = normalizeHistory([
    { role: 'assistant', content: [
      { type: 'thinking', thinking: 'Consider this.' }, { type: 'text', text: 'Run it.' },
      { type: 'tool_use', id: 'a', name: 'terminal', input: { command: 'pwd' } }
    ] },
    { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'a', content: '/example' }] }
  ]);
  assert.deepEqual(items.map(i => i.kind), ['reasoning', 'text', 'tool']);
  assert.equal(items[2].output, '/example');
});

test('reasoning-only history and real summaries survive; encrypted data does not', () => {
  const items = normalizeHistory([
    { role: 'assistant', reasoning_details: [{ type: 'reasoning.text', text: 'Actual thought.' }] },
    { role: 'assistant', codex_reasoning_items: [{ summary: [{ type: 'summary_text', text: 'Actual summary.' }] }] },
    { role: 'assistant', reasoning_details: [{ type: 'reasoning.encrypted', data: 'ciphertext' }] },
    { role: 'assistant', reasoning: 'Same', reasoning_content: 'Same' }
  ]);
  assert.deepEqual(items.map(i => i.text), ['Actual thought.', 'Actual summary.', 'Same']);
});

test('malformed arguments and orphan results are retained', () => {
  const items = normalizeHistory([
    { role: 'assistant', tool_calls: [{ id: 'bad', function: { name: 'terminal', arguments: '{oops' } }] },
    { role: 'tool', tool_call_id: 'orphan', tool_name: 'terminal', content: 'output' }
  ]);
  assert.equal(items[0].input, '{oops'); assert.equal(items[1].output, 'output');
});

test('null and falsy tool results remain present, including orphan failures', () => {
  for (const result of [null, false, 0, '']) {
    assert.equal(normalizeHistory([{ role: 'tool', tool_call_id: 'orphan', content: result }])[0].output, result);
    assert.equal(run(event('tool.complete', { tool_id: 'a', result })).items[0].output, result);
  }
  assert.equal(normalizeHistory([{ role: 'tool', content: '{"success":false}' }])[0].status, 'error');
});

test('categories use exact tools/paths, never descriptive prose', () => {
  const names = ['memory', 'skills_list', 'skill_view', 'skill_manage', 'terminal', 'unknown_memory_skill'];
  const items = normalizeHistory(names.map(name => ({ role: 'tool', name, context: 'updated memory and skills' })));
  assert.deepEqual(items.map(i => i.category), ['memory', 'skills', 'skills', 'skills', 'terminal', 'other']);
  assert.equal(normalizeHistory([{ role: 'tool', name: 'read_file', args: { path: '/example/SKILL.md' } }])[0].category, 'skills');
});

test('native JSON-RPC envelopes and Desktop bare events normalize identically', () => {
  const e = event('message.delta', { text: 'Hi' }, { seq: 1, session_id: 'synthetic' });
  assert.deepEqual(reduceEvent(undefined, e), reduceEvent(undefined, { jsonrpc: '2.0', method: 'event', params: e }));
});

test('streamed interims, tools and final text keep order without duplicate bubbles', () => {
  const s = run(start(), event('reasoning.delta', { text: 'Inspect.' }),
    event('reasoning.available', { text: 'Inspect.' }),
    event('message.delta', { text: 'I will check.' }),
    event('message.interim', { text: 'I will check.', already_streamed: true }),
    event('tool.start', { tool_id: 't', name: 'terminal', args: { command: 'pwd' } }),
    event('tool.complete', { tool_id: 't', name: 'terminal', result: '/example' }),
    event('reasoning.delta', { text: 'Now answer.' }),
    event('message.delta', { text: 'Done' }), event('message.delta', { text: '.' }),
    event('message.complete', { text: 'Done.', reasoning: 'Now answer.', status: 'complete' }));
  assert.deepEqual(s.items.map(i => i.kind), ['reasoning', 'text', 'tool', 'reasoning', 'text']);
  assert.equal(s.items[2].output, '/example'); assert.equal(s.status, 'idle');
});

test('interim arriving after tool start seals its earlier streamed text', () => {
  const s = run(start(), event('message.delta', { text: 'Checking' }),
    event('tool.start', { tool_id: 't', name: 'terminal' }),
    event('message.interim', { text: 'Checking.', already_streamed: true }),
    event('message.delta', { text: 'Final' }), event('message.complete', { text: 'Final' }));
  assert.deepEqual(s.items.map(i => i.text).filter(Boolean), ['Checking.', 'Final']);
});

test('unstreamed interim survives, and identical answers in different turns survive', () => {
  const s = run(start(), event('message.interim', { text: 'Checking.', already_streamed: false }),
    event('message.complete', { text: 'Done' }), start(), event('message.complete', { text: 'Done' }));
  assert.deepEqual(s.items.map(i => i.text), ['Checking.', 'Done', 'Done']);
});

test('interleaved text and reasoning preserve order through final snapshots', () => {
  const s = run(start(), event('message.delta', { text: 'First. ' }),
    event('reasoning.delta', { text: 'Check. ' }), event('message.delta', { text: 'Second.' }),
    event('reasoning.delta', { text: 'Confirm.' }),
    event('message.complete', { text: 'First. Second.', reasoning: 'Check. Confirm.' }));
  assert.deepEqual(s.items.map(i => [i.kind, i.text]), [
    ['text', 'First. '], ['reasoning', 'Check. '], ['text', 'Second.'], ['reasoning', 'Confirm.']
  ]);
});

test('delayed interim stays sealed when a final answer shares its prefix', () => {
  const s = run(start(), event('message.delta', { text: 'Check' }),
    event('tool.start', { tool_id: 't', name: 'terminal' }),
    event('message.interim', { text: 'Check', already_streamed: true }),
    event('message.complete', { text: 'Check completed.' }));
  assert.deepEqual(s.items.filter(i => i.kind === 'text').map(i => i.text), ['Check', 'Check completed.']);
});

test('explicitly unstreamed repeated interims are distinct occurrences', () => {
  const s = run(start(), event('message.interim', { text: 'Checking', already_streamed: false }),
    event('tool.complete', { tool_id: 't', name: 'terminal', result: 'ok' }),
    event('message.interim', { text: 'Checking', already_streamed: false }));
  assert.equal(s.items.filter(i => i.kind === 'text').length, 2);
});

test('replayed sequence is idempotent through serialized snapshots, repeated tokens are not deduped', () => {
  let s = run(start(), event('message.delta', { text: 'ha' }, { seq: 1, session_id: 's' }));
  s = reduceEvent(JSON.parse(JSON.stringify(s)), event('message.delta', { text: 'ha' }, { seq: 1, session_id: 's' }));
  s = reduceEvent(s, event('message.delta', { text: 'ha' }, { seq: 2, session_id: 's' }));
  assert.equal(s.items[0].text, 'haha');
  s = reduceEvent(s, event('message.delta', { text: '!' }, { seq: 1, session_id: 's', epoch: 'new' }));
  assert.equal(s.items[0].text, 'haha!');
});

test('parallel same-name tools correlate by ID and preserve args through progress', () => {
  const s = run(start(), event('tool.start', { tool_id: 'a', name: 'terminal', args: { command: 'one' } }),
    event('tool.start', { tool_id: 'b', name: 'terminal', args: { command: 'two' } }),
    event('tool.progress', { tool_call_id: 'a', preview: 'Working' }),
    event('tool.complete', { tool_id: 'b', result: { success: false, error: 'Failed' } }),
    event('tool.complete', { tool_id: 'a', result: 'ok' }));
  assert.equal(s.items.length, 2); assert.equal(s.items[0].input.command, 'one');
  assert.equal(s.items[1].status, 'error'); assert.equal(s.items[0].status, 'complete');
});

test('thinking/status/MoA progress do not synthesize reasoning', () => {
  const s = run(event('thinking.delta', { text: 'Thinking…' }),
    event('status.update', { kind: 'compacting', text: 'Compacting' }),
    event('moa.progress', { refs_done: 1, refs_total: 2 }));
  assert.ok(s.items.every(i => i.kind === 'activity'));
});

for (const kind of ['approval', 'clarify', 'sudo', 'secret']) {
  test(`${kind} cards retain request ID/kind, replay safely and expire by ID`, () => {
    const payload = { request_id: 'req', question: 'Choose', choices: ['a', 'b'], env_var: 'EXAMPLE_KEY', password: 'never-store', value: 'never-store' };
    let s = run(start(), event(`${kind}.request`, payload), event(`${kind}.request`, payload));
    assert.equal(s.requests.length, 1); assert.equal(s.requests[0].id, 'req');
    assert.equal(s.requests[0].kind, kind); assert.equal(s.status, 'waiting');
    assert.equal(JSON.stringify(s).includes('never-store'), false);
    s = reduceEvent(s, event(`${kind}.expire`, { request_id: 'old' }));
    assert.equal(s.requests.length, 1);
    s = reduceEvent(s, event(`${kind}.expire`, { request_id: 'req' }));
    assert.equal(s.requests.length, 0); assert.equal(s.status, 'running');
  });
}

test('batch clarify data and simultaneous request kinds survive', () => {
  const s = run(event('clarify.request', { request_id: 'same', questions: [{ id: 'q', question: 'Which?' }], answers: { q: 'A' } }),
    event('approval.request', { request_id: 'same', command: 'example' }));
  assert.equal(s.requests.length, 2); assert.equal(s.requests[0].input.answers.q, 'A');
});

test('completion merges usage, clears requests, and preserves partial errors', () => {
  const s = run(start(), event('message.delta', { text: 'Partial' }),
    event('sudo.request', { request_id: 'r' }),
    event('message.complete', { status: 'error', partial: true, text: 'Partial', error: 'Synthetic failure', usage: { output: 2 } }));
  assert.equal(s.status, 'error'); assert.deepEqual(s.requests, []);
  assert.equal(s.items[0].text, 'Partial'); assert.equal(s.items[1].kind, 'notice');
  assert.equal(s.usage.output, 2);
});

test('cancel ends pending work and late deltas do not revive it', () => {
  const s = run(start(), event('tool.start', { tool_id: 't', name: 'memory' }),
    event('secret.request', { request_id: 'r' }),
    event('message.complete', { status: 'interrupted', text: '' }), event('message.delta', { text: 'late' }));
  assert.equal(s.status, 'cancelled'); assert.equal(s.items[0].status, 'cancelled');
  assert.equal(s.items.length, 1); assert.deepEqual(s.requests, []);
});

test('session info, title, and native session.usage update live values', () => {
  const s = run(event('session.info', { model: 'example-model', cwd: '/example', running: true, usage: { input: 10 } }),
    event('session.usage', { usage: { output: 5 } }), event('session.title', { title: 'Example' }),
    event('session.info', { running: false }));
  assert.equal(s.info.model, 'example-model'); assert.equal(s.info.title, 'Example');
  assert.deepEqual(s.usage, { input: 10, output: 5 }); assert.equal(s.status, 'idle');
});

test('memory/skill tool updates and review summaries retain meaningful data', () => {
  const s = run(event('tool.complete', { tool_id: 'm', name: 'memory', args: { action: 'add', target: 'user', content: 'Example preference' }, result: { success: true } }),
    event('tool.complete', { tool_id: 'k', name: 'skill_manage', args: { action: 'create', name: 'example' }, result: 'Created' }),
    event('review.summary', { text: 'Saved the example skill.' }));
  assert.equal(s.items[0].category, 'memory'); assert.equal(s.items[1].category, 'skills');
  assert.equal(s.items[2].text, 'Saved the example skill.');
});

test('subagent identity and output remain separate from the parent lifecycle', () => {
  const s = run(start(), event('subagent.start', { subagent_id: 'child', child_session_id: 'cs', goal: 'Inspect' }),
    event('subagent.tool', { subagent_id: 'child', tool_name: 'terminal', tool_preview: 'pwd' }),
    event('subagent.complete', { subagent_id: 'child', summary: 'Done', status: 'complete', output_tokens: 12 }));
  assert.equal(s.status, 'running'); assert.equal(s.items.length, 3);
  assert.equal(s.items[1].output.tool_name, 'terminal'); assert.equal(s.items[2].output.subagent_id, 'child');
});

test('todo revisions, keyed notices, and unknown events are retained predictably', () => {
  const s = run(event('todo.updated', { revision: 2, todos: [{ id: 'a', content: 'Example', status: 'pending' }] }),
    event('todo.updated', { revision: 1, todos: [] }),
    event('notification.show', { key: 'credits', text: 'Low' }),
    event('notification.show', { key: 'credits', text: 'Restored' }),
    event('notification.clear', { key: 'credits' }), event('future.event', { nested: { count: 1 } }));
  assert.equal(s.items[0].output.length, 1);
  assert.equal(s.items.some(i => i.kind === 'notice'), false);
  assert.equal(s.items.at(-1).name, 'future.event'); assert.equal(s.items.at(-1).output.nested.count, 1);
});

test('states and incoming nested payloads are independent immutable snapshots', () => {
  const e = freeze(event('tool.start', { tool_id: 't', name: 'terminal', args: { nested: { command: 'example' } } }));
  const before = freeze(reduceEvent(undefined, e));
  const saved = JSON.stringify(before);
  const after = reduceEvent(before, event('tool.complete', { tool_id: 't', result: { ok: true } }));
  assert.notEqual(after.items[0], before.items[0]);
  assert.equal(after.items[0].input, before.items[0].input);
  assert.equal(after.items[0].output.ok, true);
  assert.equal(JSON.stringify(before), saved); assert.equal(e.payload.args.nested.command, 'example');
  assert.deepEqual(reduceEvent(JSON.parse(saved), event('session.title', { title: 'x' })),
    reduceEvent(before, event('session.title', { title: 'x' })));
});

test('defensive bounds, cycles, prototype keys and HTML remain plain safe data', () => {
  const cycle = {}; cycle.self = cycle;
  const s = reduceEvent(undefined, event('unknown', { cycle, text: '<script>example</script>', rendered: '<b>ignore</b>' }));
  assert.equal(s.items[0].text, '<script>example</script>');
  assert.equal(JSON.stringify(s).includes('<b>ignore</b>'), false);
  assert.doesNotThrow(() => JSON.stringify(s));
  assert.deepEqual(normalizeHistory(null), []);
  assert.doesNotThrow(() => reduceEvent(null, null));
  assert.doesNotThrow(() => reduceEvent({ requests: [null, 1, 'bad'] }, event('sudo.request', { request_id: 'r' })));
  assert.ok(normalizeHistory(Array.from({ length: 2100 }, () => ({ role: 'user', text: 'x' }))).length <= 2000);
  assert.ok(run(event('message.delta', { text: 'x'.repeat(100000) })).items[0].text.length <= 65536);
  const polluted = reduceEvent(undefined, event('session.info', JSON.parse('{"__proto__":{"polluted":true}}')));
  assert.equal(polluted.info.polluted, undefined); assert.equal({}.polluted, undefined);
});

test('successful history hydration updates state without polluting the transcript', () => {
 const s=run(event('session.resume_progress',{phase:'history',status:'loading'}),event('session.resume_progress',{phase:'history',status:'complete'}));
 assert.equal(s.items.length,0);assert.equal(s.info.hydrating,false);
 const failed=reduceEvent(s,event('session.resume_progress',{phase:'history',status:'failed',message:'Unable to restore'}));
 assert.equal(failed.items.length,1);
});
