const test = require("node:test");
const assert = require("node:assert/strict");
const { HermesProvider } = require("../electron/agents/hermes-provider.cjs");
function fixture(t) {
  const calls = [],
    transports = new Map();
  let next = 0;
  const p = new HermesProvider({
    transportFactory: (options) => {
      const transport = {
        options,
        async request(method, path, input = {}) {
          calls.push({ profile: options.profile, method, path, input });
          if (path === "/api/profiles")
            return { profiles: [{ name: "default" }, { name: "second" }] };
          if (path.endsWith("/messages"))
            return {
              session_id: "shared",
              messages: [{ id: 1, role: "user", content: options.profile }],
            };
          return { ok: true };
        },
        async rpc(profile, method, input) {
          calls.push({ profile, method, input });
          assert.equal(profile, options.profile);
          if (method === "session.list")
            return { sessions: [{ id: "shared", title: "Bot Chat" }] };
          if (method === "session.resume")
            return {
              session_id: `runtime-${profile}`,
              info: { model: "test" },
            };
          if (method === "session.create")
            return {
              session_id: `runtime-${profile}-${++next}`,
              stored_session_id: `draft-${next}`,
            };
          return { ok: true };
        },
        close() {},
      };
      transports.set(options.profile, transport);
      return transport;
    },
  });
  t.after(() => p.close());
  return { p, calls, transports };
}
test("same durable IDs in two profiles have separate runtime bindings and events", async (t) => {
  const { p, transports, calls } = fixture(t);
  await p.listAgents();
  const a = await p.canonical({ agentId: "default" });
  const b = await p.canonical({ agentId: "second" });
  assert.equal(a.conversationId, b.conversationId);
  assert.notEqual(a.items[0].text, b.items[0].text);
  transports.get("second").options.onEvent({
    type: "message.delta",
    session_id: "runtime-second",
    payload: { text: "second-only" },
  });
  assert.equal(
    p.snapshot({ agentId: "default", conversationId: "shared" }).items.length,
    1,
  );
  assert.equal(
    p.snapshot({ agentId: "second", conversationId: "shared" }).items.at(-1)
      .text,
    "second-only",
  );
  await p.send({ agentId: "default", conversationId: "shared", text: "hello" });
  assert.equal(calls.at(-1).input.session_id, "runtime-default");
  await assert.rejects(
    p.send({ agentId: "default", conversationId: "shared", text: "duplicate" }),
    /Wait/,
  );
});
test("canonical lookup failure never creates an alternate primary chat", async (t) => {
  const { p, calls } = fixture(t);
  await p.listAgents();
  const transport = p.transport("default");
  transport.rpc = async () => {
    throw Error("lookup failed");
  };
  await assert.rejects(p.canonical({ agentId: "default" }), /lookup failed/);
  assert.equal(
    calls.some((c) => c.method === "session.create"),
    false,
  );
});
test("activity reports committed tool writes and post-turn review exactly once", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  const emit = transports.get("default").options.onEvent;
  emit({
    type: "message.start",
    session_id: "runtime-default",
    seq: 1,
    payload: {},
  });
  emit({
    type: "tool.start",
    session_id: "runtime-default",
    seq: 2,
    payload: { tool_id: "write", name: "memory", args: { action: "add" } },
  });
  const complete = {
    type: "tool.complete",
    session_id: "runtime-default",
    seq: 3,
    payload: { tool_id: "write", name: "memory", result: { success: true } },
  };
  emit(complete);
  emit(complete);
  emit({
    type: "message.complete",
    session_id: "runtime-default",
    seq: 4,
    payload: { text: "Done" },
  });
  emit({
    type: "review.summary",
    session_id: "runtime-default",
    seq: 5,
    payload: { text: "Created a reusable skill" },
  });
  const entries = await p.operations.get("activity.list")({});
  assert.deepEqual(
    entries.map((e) => e.kind),
    ["memory", "self-improvement"],
  );
  assert.equal(
    p.snapshot({ agentId: "default", conversationId: "shared" }).status,
    "idle",
  );
});
test("interaction IDs are scoped to the owning conversation", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  await p.canonical({ agentId: "second" });
  transports.get("default").options.onEvent({
    type: "approval.request",
    session_id: "runtime-default",
    payload: { request_id: "r1", command: "example" },
  });
  await assert.rejects(
    p.respond({
      agentId: "second",
      conversationId: "shared",
      requestId: "r1",
      response: { choice: "once" },
    }),
    /no longer pending/,
  );
  await p.respond({
    agentId: "default",
    conversationId: "shared",
    requestId: "r1",
    response: { choice: "once" },
  });
  assert.equal(
    p.snapshot({ agentId: "default", conversationId: "shared" }).requests
      .length,
    0,
  );
});

test("a chosen title persists before the tab is reopenable; an unnamed chat is left for Hermes to name", async (t) => {
  const { p, calls } = fixture(t);
  await p.listAgents();
  const named = await p.create({ agentId: "default", title: "Release notes" });
  const save = calls.find((c) => c.method === "session.title");
  assert.equal(save.input.session_id, "runtime-default-1");
  assert.equal(save.input.title, "Release notes");
  assert.equal(named.conversationId, "draft-1");
  calls.length = 0;
  const unnamed = await p.create({ agentId: "default" });
  const create = calls.find((c) => c.method === "session.create");
  assert.equal("title" in create.input, false);
  assert.equal(
    calls.some((c) => c.method === "session.title"),
    false,
  );
  assert.equal(unnamed.title, "New conversation");
});
test("search uses the native full-history endpoint and list exposes pagination", async (t) => {
  const { p, calls } = fixture(t);
  await p.listAgents();
  const transport = p.transport("second");
  transport.request = async (method, path, input) => {
    calls.push({ method, path, input });
    return path.endsWith("search")
      ? {
          results: [
            { session_id: "older", title: "Found in message", archived: true },
          ],
        }
      : { sessions: [{ id: "page-two" }], total: 80 };
  };
  const page = await p.listConversations({ agentId: "second", offset: 50 });
  assert.equal(page.nextOffset, 51);
  assert.equal(calls.at(-1).input.profile, "second");
  const search = await p.listConversations({
    agentId: "second",
    search: "needle",
  });
  assert.equal(calls.at(-1).path, "/api/sessions/search");
  assert.equal(calls.at(-1).input.query.q, "needle");
  assert.equal(search.conversations[0].id, "older");
  assert.equal(search.conversations[0].archived, true);
  assert.equal(search.nextOffset, null);
});
test("concurrent older-history requests share one fetch and overlapping rows are unique", async (t) => {
  const { p } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  const input = { agentId: "default", conversationId: "shared" };
  const session = p.get(input);
  session.hasEarlier = true;
  session.historyOffset = 100;
  let resolve,
    calls = 0;
  p.transport("default").request = () => {
    calls++;
    return new Promise((r) => (resolve = r));
  };
  const one = p.history(input),
    two = p.history(input);
  resolve({
    messages: [
      { id: 0, role: "user", content: "earlier" },
      { id: 1, role: "user", content: "default" },
    ],
  });
  const [a, b] = await Promise.all([one, two]);
  assert.equal(calls, 1);
  assert.deepEqual(a, b);
  assert.equal(a.items.length, 2);
  assert.equal(a.items[0].text, "earlier");
});

test("batch clarification uses native qids, preserves remaining answers, and rejects unknown questions", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  const transport = transports.get("default");
  transport.options.onEvent({
    type: "clarify.request",
    session_id: "runtime-default",
    payload: {
      request_id: "batch",
      questions: [
        { qid: "goal", question: "Goal?" },
        { qid: "format", question: "Format?" },
      ],
    },
  });
  const sent = [];
  transport.rpc = async (_, method, input) => {
    sent.push(input);
    assert.equal(method, "clarify.respond");
    return {
      status: "ok",
      remaining: input.question_id === "goal" ? ["format"] : [],
    };
  };
  const input = {
    agentId: "default",
    conversationId: "shared",
    requestId: "batch",
  };
  await assert.rejects(
    p.respond({ ...input, response: { answer: "Release" } }),
    /question ID/,
  );
  await assert.rejects(
    p.respond({
      ...input,
      response: { answer: "Release", questionId: "undefined" },
    }),
    /question ID/,
  );
  assert.equal(sent.length, 0);
  await p.respond({
    ...input,
    response: { answer: "Release", questionId: "goal" },
  });
  assert.deepEqual(
    p.snapshot(input).requests[0].input.questions.map((q) => q.qid),
    ["format"],
  );
  transport.rpc = async () => {
    throw Error("Connection lost");
  };
  await assert.rejects(
    p.respond({
      ...input,
      response: { answer: "Markdown", questionId: "format" },
    }),
    /Connection lost/,
  );
  assert.deepEqual(
    p.snapshot(input).requests[0].input.questions.map((q) => q.qid),
    ["format"],
  );
  transport.rpc = async (_, method, input) => {
    sent.push(input);
    return { status: "ok", remaining: [] };
  };
  await p.respond({
    ...input,
    response: { answer: "Markdown", questionId: "format" },
  });
  assert.deepEqual(
    sent.map((x) => x.question_id),
    ["goal", "format"],
  );
  assert.equal(p.snapshot(input).requests.length, 0);
  assert.equal(p.snapshot(input).status, "running");
});

test("approval choices follow the native request and expired responses are not reported as applied", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  const transport = transports.get("default");
  transport.options.onEvent({
    type: "approval.request",
    session_id: "runtime-default",
    payload: { request_id: "restricted", choices: ["once", "deny"] },
  });
  let sent = 0;
  transport.rpc = async () => {
    sent++;
    return { resolved: false };
  };
  const input = {
    agentId: "default",
    conversationId: "shared",
    requestId: "restricted",
  };
  await assert.rejects(
    p.respond({ ...input, response: { choice: "session" } }),
    /approval choice/,
  );
  await assert.rejects(
    p.respond({ ...input, response: { choice: "always" } }),
    /approval choice/,
  );
  assert.equal(sent, 0);
  await assert.rejects(
    p.respond({ ...input, response: { choice: "once" } }),
    /expired/,
  );
  assert.equal(p.snapshot(input).requests.length, 0);
  transport.options.onEvent({
    type: "approval.request",
    session_id: "runtime-default",
    payload: { request_id: "permitted", choices: ["once", "always", "deny"] },
  });
  transport.rpc = async () => ({ resolved: true });
  await p.respond({
    ...input,
    requestId: "permitted",
    response: { choice: "always" },
  });
});

test("resumed batch clarification omits answers already accepted by Hermes", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  const transport = transports.get("default"),
    rpc = transport.rpc.bind(transport);
  transport.rpc = async (profile, method, input) =>
    method === "session.resume"
      ? {
          session_id: "runtime-default",
          pending_clarify: {
            request_id: "batch",
            questions: [
              { qid: "done", question: "First" },
              { qid: "next", question: "Second" },
            ],
            answers: { done: "Accepted" },
          },
        }
      : rpc(profile, method, input);
  const s = await p.canonical({ agentId: "default" });
  assert.deepEqual(
    s.requests[0].input.questions.map((q) => q.qid),
    ["next"],
  );
});

test("ordinary conversations restore native runtime settings, not child-watch mode", async (t) => {
  const { p, calls } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  const resume = calls.find((c) => c.method === "session.resume");
  assert.equal(resume.input.eager_build, true);
  assert.equal(resume.input.lazy, undefined);
  assert.equal(resume.input.omit_messages, true);
  assert.equal(resume.input.close_on_disconnect, false);
});

test("ordinary new conversations keep their own runtime settings instead of following Bot Chat defaults", async (t) => {
  const { p, calls } = fixture(t);
  await p.listAgents();
  await p.create({ agentId: "default", title: "Independent chat" });
  const create = calls.find((c) => c.method === "session.create");
  assert.notEqual(create.input.follow_profile_config, true);
});

test("native auto-continuation is marked running and buffered completion survives rebinding", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  await p.canonical({ agentId: "default" });
  const transport = transports.get("default");
  transport.rpc = async () => ({
    session_id: "restarted-runtime",
    resumed: "shared",
    auto_continue: { attempt: 1 },
    info: { model: "restored" },
  });
  const input = { agentId: "default", conversationId: "shared" };
  await p.reattach(p.get(input));
  assert.equal(p.snapshot(input).status, "running");
  assert.equal(p.get(input).autoContinuing, true);
  transport.rpc = async () => {
    transport.options.onEvent({
      type: "message.complete",
      session_id: "completed-runtime",
      payload: { text: "Native continuation finished" },
    });
    return {
      session_id: "completed-runtime",
      auto_continue: { attempt: 1 },
      info: {},
    };
  };
  await p.reattach(p.get(input));
  assert.equal(p.snapshot(input).status, "idle");
  assert.ok(
    p
      .snapshot(input)
      .items.some((i) => i.text === "Native continuation finished"),
  );
});

test("attachment send stages files before the prompt and never accepts renderer paths", async (t) => {
  const { p, calls, transports } = fixture(t);
  await p.listAgents();
  const chat = await p.create({ agentId: "default", title: "Attachments" });
  const input = { agentId: "default", conversationId: chat.conversationId };
  const transport = transports.get("default"),
    original = transport.rpc;
  transport.rpc = async (profile, method, args) => {
    if (method === "file.attach") {
      calls.push({ profile, method, input: args });
      return {
        attached: true,
        path: "/synthetic/note",
        ref_text: "@file:/synthetic/note",
      };
    }
    return original(profile, method, args);
  };
  await assert.rejects(
    p.send({
      ...input,
      text: "Read",
      attachments: [{ name: "../secret", data: "YQ==" }],
    }),
    /name/,
  );
  await p.send({
    ...input,
    text: "Read",
    attachments: [{ name: "note.txt", data: "YQ==" }],
  });
  const upload = calls.findIndex((c) => c.method === "file.attach"),
    submit = calls.findIndex((c) => c.method === "prompt.submit");
  assert.ok(upload >= 0 && submit > upload);
  assert.equal(calls[upload].input.path, undefined);
  assert.match(calls[submit].input.text, /@file:\/synthetic\/note/);
});
test("unconfirmed attachment upload prevents accidental reuse on another send", async (t) => {
  const { p, transports } = fixture(t);
  await p.listAgents();
  const chat = await p.create({ agentId: "default", title: "Failure" });
  const input = { agentId: "default", conversationId: chat.conversationId };
  const transport = transports.get("default"),
    original = transport.rpc;
  let submits = 0;
  transport.rpc = async (profile, method, args) => {
    if (method === "image.attach_bytes") throw Error("Timed out");
    if (method === "prompt.submit") submits++;
    return original(profile, method, args);
  };
  await assert.rejects(
    p.send({
      ...input,
      text: "Review",
      attachments: [{ name: "image.png", data: "YQ==" }],
    }),
    /Timed out/,
  );
  await assert.rejects(
    p.send({ ...input, text: "Another message" }),
    /Restart sushiAI/,
  );
  assert.equal(submits, 0);
});
