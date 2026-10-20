const { test } = require("node:test");
const assert = require("node:assert/strict");
const { harnessFor, chatPrompt } = require("../electron/harness.cjs");
// What the daemon's `agents.catalog` serves for one agent. `chatArgs` refuses
// anything outside it, so a catalog that has not arrived refuses everything.
const OFFERED = {
  efforts: ["low", "high"],
  permissions: ["default", "acceptEdits"],
  defaultModel: "the-default",
};
const chatArgs = (agent, options) =>
  harnessFor(agent).chatArgs(options, OFFERED);
const claudeEvent = harnessFor("claude").chatEvent;
const codexEvent = harnessFor("codex").chatEvent;
const threads = import("../src/chat-threads.ts");

const CLAUDE_BASE = [
  "--print",
  "--output-format",
  "stream-json",
  "--verbose",
  "--include-partial-messages",
];
test("claude args carry model, effort and permission mode", () => {
  assert.deepEqual(chatArgs("claude"), CLAUDE_BASE);
  assert.deepEqual(
    chatArgs("claude", {
      model: "claude-sonnet-5",
      effort: "high",
      permission: "acceptEdits",
    }),
    [
      ...CLAUDE_BASE,
      "--model",
      "claude-sonnet-5",
      "--effort",
      "high",
      "--permission-mode",
      "acceptEdits",
    ],
  );
  assert.deepEqual(chatArgs("claude", { permission: "default" }), CLAUDE_BASE);
});
test("claude events become answer text, the resolved model, usage and window", () => {
  const delta = (delta) => ({
    type: "stream_event",
    event: { type: "content_block_delta", delta },
  });
  assert.deepEqual(claudeEvent(delta({ type: "text_delta", text: "Hi" })), {
    text: "Hi",
  });
  assert.equal(
    claudeEvent(delta({ type: "thinking_delta", thinking: "" })).note,
    "Thinking",
  );
  assert.equal(
    claudeEvent({
      type: "stream_event",
      event: {
        type: "content_block_start",
        content_block: { type: "tool_use", name: "Bash" },
      },
    }).note,
    "Tool Bash",
  );
  assert.equal(
    claudeEvent({ type: "system", subtype: "init", model: "claude-opus-5[1m]" })
      .model,
    "claude-opus-5[1m]",
  );
  // Hook chatter, status lines and echoed assistant messages say nothing new.
  assert.equal(claudeEvent({ type: "system", subtype: "hook_started" }), null);
  assert.equal(
    claudeEvent({ type: "assistant", message: { model: "x", content: [] } }),
    null,
  );
  assert.equal(claudeEvent({ type: "rate_limit_event" }), null);
  const result = claudeEvent({
    type: "result",
    subtype: "success",
    result: "Hi",
    usage: {
      input_tokens: 10,
      cache_creation_input_tokens: 22178,
      cache_read_input_tokens: 13782,
      output_tokens: 121,
    },
    modelUsage: { "claude-haiku-4-5-20251001": { contextWindow: 200000 } },
  });
  // Context in use is the whole prompt Claude sent, cached parts included.
  assert.deepEqual(result.usage, {
    input: 35970,
    output: 121,
    cached: 13782,
    context: 200000,
  });
  assert.equal(result.model, "claude-haiku-4-5-20251001");
  assert.equal(result.full, "Hi");
  assert.deepEqual(
    claudeEvent({ type: "result", is_error: true, result: "nope" }),
    {
      error: "nope",
      fatal: true,
    },
  );
});
test("codex keeps the stdin prompt marker last and rejects unsafe values", () => {
  const args = chatArgs("codex", {
    model: "gpt-5.4",
    effort: "low",
    permission: "acceptEdits",
  });
  assert.equal(args.at(-1), "-");
  assert.ok(args.includes("--sandbox") && args.includes("workspace-write"));
  assert.ok(args.includes('model_reasoning_effort="low"'));
  assert.throws(
    () => chatArgs("claude", { model: "x; rm -rf /" }),
    /Invalid model/,
  );
  assert.throws(
    () => chatArgs("claude", { effort: "ultra" }),
    /Invalid effort/,
  );
  assert.throws(
    () => chatArgs("codex", { permission: "root" }),
    /Invalid permission/,
  );
});
test("attachments: claude add-dir stays last, codex images precede flags", () => {
  const c = chatArgs("claude", {
    permission: "acceptEdits",
    dirs: ["/a", "/b c"],
  });
  assert.deepEqual(c.slice(-3), ["--add-dir", "/a", "/b c"]);
  const x = chatArgs("codex", { images: ["/p/i.png"], dirs: ["/p"] });
  assert.equal(x.at(-1), "-");
  assert.ok(x.indexOf("--image") < x.indexOf("--json"));
  assert.ok(!x.includes("--add-dir"));
  const prompt = chatPrompt([
    { role: "user", text: "look", attachments: ["/p/i.png", "/p/dir"] },
    { role: "assistant", text: "ok" },
  ]);
  assert.match(
    prompt,
    /^user: look\n\[Attached[^\]]*\/p\/i\.png, \/p\/dir\]\n\nassistant: ok$/,
  );
});
test("chat options are checked against the agent's catalog entry", () => {
  const bare = (agent, options) =>
    harnessFor(agent).chatArgs(options, undefined);
  assert.ok(chatArgs("claude", { effort: "high" }).includes("high"));
  assert.ok(
    chatArgs("codex", { effort: "high" }).includes(
      'model_reasoning_effort="high"',
    ),
  );
  // A model that names no level needs no catalog.
  assert.ok(bare("claude", {}).includes("--print"));
  // A value the entry lacks is refused, and so is anything while it is empty.
  for (const [run, agent, options, error] of [
    [chatArgs, "claude", { effort: "max" }, /Invalid effort/],
    [chatArgs, "codex", { effort: "ultra" }, /Invalid effort/],
    [chatArgs, "claude", { permission: "plan" }, /Invalid permission mode/],
    [bare, "claude", { effort: "high" }, /Invalid effort/],
    [bare, "claude", { permission: "default" }, /Invalid permission mode/],
  ])
    assert.throws(() => run(agent, options), error);
});
test("codex events become answer text, progress notes, usage and errors", () => {
  const answer = {
    type: "item.completed",
    item: { type: "agent_message", text: "Done" },
  };
  assert.deepEqual(codexEvent(answer), { text: "Done\n" });
  assert.equal(codexEvent({ ...answer, type: "item.started" }), null);
  assert.equal(
    codexEvent({
      type: "item.started",
      item: { type: "command_execution", command: "npm test" },
    }).note,
    "Running npm test",
  );
  assert.equal(
    codexEvent({
      type: "item.updated",
      item: { type: "reasoning", text: "**Checking config**\nmore" },
    }).note,
    "Checking config",
  );
  assert.equal(
    codexEvent({
      type: "item.completed",
      item: { type: "file_change", changes: [1, 2] },
    }).note,
    "Edited 2 files",
  );
  // A deprecation notice arrives on runs that succeed, so it must not be fatal.
  assert.deepEqual(
    codexEvent({
      type: "item.completed",
      item: { type: "error", message: "deprecated" },
    }),
    { error: "deprecated", fatal: false },
  );
  assert.deepEqual(
    codexEvent({ type: "turn.failed", error: { message: "boom" } }),
    {
      error: "boom",
      fatal: true,
    },
  );
  assert.deepEqual(
    codexEvent({
      type: "turn.completed",
      usage: { input_tokens: 10, output_tokens: 2 },
    }).usage,
    { input: 10, output: 2, cached: 0 },
  );
  assert.equal(codexEvent({ type: "thread.started" }), null);
});
test("thread helpers: titles, relative time and grouping", async () => {
  const { titleFrom, relativeTime, groupThreads, contextUsage, modelName } =
    await threads;
  assert.equal(modelName("claude-opus-5[1m]"), "Opus 5 · 1M");
  assert.equal(modelName("claude-haiku-4-5-20251001"), "Haiku 4.5");
  assert.equal(modelName("claude-fable-5-1"), "Fable 5.1");
  assert.equal(modelName("gpt-6-astra"), "gpt-6-astra");
  assert.equal(contextUsage([{ text: "a".repeat(32800) }]), "8.2k · 4%");
  // Codex reports real tokens, so the estimate is ignored when they arrive.
  assert.equal(contextUsage([{ text: "x" }], 272000, 27200), "27.2k · 10%");
  assert.equal(contextUsage([{ text: "hi" }]), "1 · 1%");
  assert.equal(contextUsage([]), "");
  assert.equal(
    titleFrom("  Why did the pane drag\nmore text"),
    "Why did the pane drag",
  );
  assert.equal(titleFrom("a".repeat(60)).length, 44);
  const now = Date.now();
  assert.equal(relativeTime(now - 5_000, now), "now");
  assert.equal(relativeTime(now - 22 * 60_000, now), "22m");
  assert.equal(relativeTime(now - 3 * 3600_000, now), "3h");
  assert.equal(relativeTime(undefined, now), "");
  const w = (id, panels) => ({ id, name: id, cwd: "/", panels, layout: null });
  const chat = (id, updatedAt, pinned) => ({
    id,
    kind: "chat",
    title: id,
    updatedAt,
    pinned,
  });
  const { pinned, groups } = groupThreads([
    w("a", [
      chat("old", 1),
      chat("new", 2),
      chat("pin", 3, true),
      { id: "t", kind: "terminal", title: "zsh" },
    ]),
    w("b", []),
  ]);
  assert.deepEqual(
    pinned.map((t) => t.panel.id),
    ["pin"],
  );
  assert.deepEqual(
    groups.map((g) => g.threads.map((t) => t.panel.id)),
    [["new", "old"]],
  );
});

// The chat handler checks a turn against the catalog of the host it runs on.
function chatHandler(catalogs) {
  const { registerChatIpc } = require("../electron/ipc/chat.cjs");
  const handlers = {};
  registerChatIpc({
    handle: (name, fn) => (handlers[name] = fn),
    send: () => {},
    getConnections: () => ({
      get: () => ({ id: "build-host" }),
      // A turn that passed every check reaches the ssh line; stop it there.
      args: () => {
        throw new Error("reached ssh");
      },
    }),
    agentCatalog: { of: async (host) => catalogs[host] ?? {} },
    executable: () => "/bin/false",
    directory: () => {},
    id: (value) => value,
    chats: new Map(),
  });
  return (effort) =>
    handlers.chat({
      panelId: "p1",
      cwd: "/work/my-app",
      agent: "claude",
      endpoint: "ssh:build-host",
      messages: [{ role: "user", text: "hi" }],
      effort,
    });
}

test("an ssh chat without a daemon there is checked against this Mac's catalog", async () => {
  const chat = chatHandler({ local: { claude: OFFERED } });
  // A level this Mac's entry offers passes; one it lacks is refused.
  await assert.rejects(chat("low"), /reached ssh/);
  await assert.rejects(chat("max"), /Invalid effort/);
  // With no catalog anywhere the reason is named, not an invalid value.
  await assert.rejects(chatHandler({})("low"), /agent list is not loaded yet/);
});
