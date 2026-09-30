const { test } = require("node:test");
const assert = require("node:assert/strict");

const client = import("../src/orchestrator/client.ts");
const model = import("../src/orchestrator/toolsModel.ts");

const config = (patch = {}) => ({
  id: "slack",
  label: "Slack",
  enabled: true,
  server: { ref: "plugin:slack/slack" },
  ...patch,
});
const row = (patch = {}) => ({
  id: "slack",
  transport: "http",
  status: "ok",
  tools: [
    { name: "search", kind: "read", guess: "read" },
    { name: "read_channel", kind: "read", guess: "read" },
    { name: "send_message", kind: "write", guess: "write" },
  ],
  ...patch,
});

test("a tool's line says what it gives the chat or why it gives nothing", async () => {
  const { toolStatusLine } = await model;
  assert.equal(toolStatusLine(config({ enabled: false }), row()), "Off");
  assert.equal(
    toolStatusLine(config(), row()),
    "Connected · 2 read · 1 ask first",
  );
  assert.equal(toolStatusLine(config(), undefined), "Not checked yet");
  assert.equal(
    toolStatusLine(
      config(),
      row({ status: "needs-auth", reason: "Needs sign-in", tools: [] }),
    ),
    "Needs sign-in",
  );
  assert.match(
    toolStatusLine(
      config(),
      row({ status: "failed", reason: null, tools: [] }),
    ),
    /runs without it/,
  );
  assert.match(
    toolStatusLine(
      config(),
      row({ codex: "Codex cannot use http servers" }),
      true,
    ),
    /Codex cannot use http servers$/,
  );
});

test("adding a server from the MCP config stores only a reference and a unique id", async () => {
  const { toolFromServer, newToolId } = await model;
  const server = {
    ref: "claude-json:google-sheets-main",
    label: "Google Sheets",
    source: "~/.claude.json",
    transport: "stdio",
  };
  const added = toolFromServer(server, ["google-sheets"]);
  assert.deepEqual(added, {
    id: "google-sheets-2",
    label: "Google Sheets",
    enabled: true,
    server: { ref: "claude-json:google-sheets-main" },
  });
  assert.equal(newToolId("  ", []), "tool");
});

test("an override is stored only when it differs from the guess", async () => {
  const { withOverride } = await model;
  const flipped = withOverride(config(), "search", "write", "read");
  assert.deepEqual(flipped.overrides, { search: "write" });
  const back = withOverride(flipped, "search", "read", "read");
  assert.equal("overrides" in back, false);
});

test("edited arguments must be a JSON object", async () => {
  const { parseArgsText, payloadPreview, actionOutcome } = await model;
  assert.deepEqual(parseArgsText('{"text":"hi"}'), { args: { text: "hi" } });
  assert.ok("error" in parseArgsText("[1]"));
  assert.ok("error" in parseArgsText("{oops"));
  assert.equal(payloadPreview({ a: 1 }), '{\n  "a": 1\n}');
  const base = {
    id: "a",
    server: "s",
    tool: "t",
    args: {},
    summary: "",
    target: "",
  };
  assert.equal(actionOutcome({ ...base, state: "pending" }), null);
  assert.equal(actionOutcome({ ...base, state: "sent" }), "Sent");
  assert.equal(actionOutcome({ ...base, state: "declined" }), "Not sent");
  assert.equal(
    actionOutcome({ ...base, state: "failed", error: "boom" }),
    "Failed: boom",
  );
});

test("the client sends the OK card's calls and the tools lookups", async () => {
  const { orchestratorClientFor } = await client;
  const calls = [];
  globalThis.window = {
    bridge: {
      orchestrator: async (method, params) => {
        calls.push([method, params]);
        return {};
      },
    },
  };
  const c = orchestratorClientFor();
  await c.chatActionSend("/repo", "m1", { text: "x" });
  await c.chatActionSend("/repo", "m2");
  await c.chatActionDecline("/repo", "m3");
  await c.chatTools(true);
  await c.chatTools();
  await c.chatToolServers();
  assert.deepEqual(calls, [
    [
      "chat.actionSend",
      { repo: "/repo", messageId: "m1", args: { text: "x" } },
    ],
    ["chat.actionSend", { repo: "/repo", messageId: "m2" }],
    ["chat.actionDecline", { repo: "/repo", messageId: "m3" }],
    ["chat.tools", { refresh: true }],
    ["chat.tools", {}],
    ["chat.toolServers", undefined],
  ]);
  delete globalThis.window;
});
