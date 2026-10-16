const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  harnessFor,
  isHarness,
  screenObjects,
} = require("../electron/harness.cjs");

const lines = (...events) => events.map((e) => JSON.stringify(e)).join("\n");

test("an unknown harness is a clear error", () => {
  assert.throws(() => harnessFor("gemini"), /Unknown harness gemini/);
  assert.throws(() => harnessFor("constructor"), /Unknown harness/);
  assert.equal(isHarness("codex"), true);
  assert.equal(isHarness("toString"), false);
});

test("every harness implements the same interface", () => {
  for (const name of ["claude", "codex"]) {
    const harness = harnessFor(name);
    assert.equal(harness.name, name);
    for (const fn of [
      "chatArgs",
      "chatEvent",
      "mcpArgs",
      "headlessTurnArgs",
      "parseTurn",
    ])
      assert.equal(typeof harness[fn], "function", `${name}.${fn}`);
    assert.ok(Array.isArray(harness.efforts) && harness.efforts.length);
    assert.ok(Array.isArray(harness.builderEfforts));
    assert.ok(harness.login.length >= 2);
  }
});

test("screenObjects rejoins wrapped rows and skips noise", () => {
  const raw = lines({ type: "a", text: "x".repeat(60) }, { type: "b" });
  const cut = 30;
  const screen = `banner\nlog line\n${raw.slice(0, cut)}\r\n${raw.slice(cut)}\nERROR tail`;
  assert.deepEqual(screenObjects(screen), [
    { type: "a", text: "x".repeat(60) },
    { type: "b" },
  ]);
  assert.deepEqual(screenObjects(null), []);
  assert.deepEqual(screenObjects('{"type":"half'), []);
});

test("screenObjects accepts an object whose first key is not type, even wrapped at a nested start", () => {
  const object = {
    cost: 1,
    nested: [{ k: "v" }],
    type: "result",
    result: "ok",
  };
  const raw = JSON.stringify(object);
  assert.deepEqual(screenObjects(raw), [object]);
  // The terminal breaks the line just before a nested `{"`.
  const cut = raw.indexOf('{"k"');
  assert.deepEqual(screenObjects(`${raw.slice(0, cut)}\r\n${raw.slice(cut)}`), [
    object,
  ]);
});

test("Claude parseTurn reads the result object, wrapped or not", () => {
  const claude = harnessFor("claude");
  const answer = JSON.stringify({
    type: "result",
    result: "All set " + "word ".repeat(40),
    session_id: "c-1",
    is_error: false,
  });
  const expected = {
    state: "done",
    text: "All set " + "word ".repeat(40),
    session: "c-1",
  };
  assert.deepEqual(claude.parseTurn(answer), expected);
  assert.deepEqual(
    claude.parseTurn(`${answer.slice(0, 50)}\r\n${answer.slice(50)}`),
    expected,
  );
  assert.equal(claude.parseTurn("Loading...").state, "running");
  assert.equal(claude.parseTurn(answer.slice(0, 40)).state, "running");
  const failed = claude.parseTurn(
    JSON.stringify({ type: "result", result: "nope", is_error: true }),
  );
  assert.deepEqual(failed, { state: "failed", text: "nope", session: "" });
});

test("Codex parseTurn takes the last agent message and the thread id", () => {
  const codex = harnessFor("codex");
  const events = lines(
    { type: "thread.started", thread_id: "t-1" },
    { type: "item.completed", item: { type: "error", message: "old" } },
    { type: "item.completed", item: { type: "agent_message", text: "first" } },
    { type: "item.completed", item: { type: "agent_message", text: "final" } },
    { type: "turn.completed", usage: {} },
  );
  assert.deepEqual(codex.parseTurn(`Reading input\n${events}`), {
    state: "done",
    text: "final",
    session: "t-1",
  });
  const at = events.indexOf("final") + 2;
  assert.equal(
    codex.parseTurn(`${events.slice(0, at)}\r\n${events.slice(at)}`).text,
    "final",
  );
  // No turn.completed yet: still running.
  assert.equal(
    codex.parseTurn(events.slice(0, events.lastIndexOf("\n"))).state,
    "running",
  );
  assert.deepEqual(
    codex.parseTurn(
      lines(
        { type: "thread.started", thread_id: "t-2" },
        { type: "turn.failed", error: { message: "not logged in" } },
      ),
    ),
    { state: "failed", text: "not logged in", session: "t-2" },
  );
  assert.equal(
    codex.parseTurn(lines({ type: "turn.failed" })).text,
    "The turn failed.",
  );
});

test("mcp flags and headless args follow each harness shape", () => {
  const server = { name: "srv", command: "cmd", args: ["a"], tools: ["t1"] };
  const claude = harnessFor("claude");
  const flags = claude.mcpArgs(server);
  assert.equal(flags[0], "--mcp-config");
  assert.deepEqual(JSON.parse(flags[1]), {
    mcpServers: { srv: { command: "cmd", args: ["a"] } },
  });
  const args = claude.headlessTurnArgs({ effort: "max", mcp: server });
  assert.deepEqual(args.slice(0, 4), [
    "--output-format",
    "json",
    "--effort",
    "max",
  ]);
  assert.ok(args[5].split(" ").includes("mcp__srv__t1"));
  assert.deepEqual(args.slice(-2), flags);

  const codex = harnessFor("codex");
  assert.deepEqual(codex.mcpArgs(server), [
    "-c",
    'mcp_servers.srv.command="cmd"',
    "-c",
    'mcp_servers.srv.args=["a"]',
  ]);
  const bare = codex.headlessTurnArgs({ effort: "high" });
  assert.deepEqual(bare, [
    "--ignore-user-config",
    "-c",
    'sandbox_mode="workspace-write"',
    "-c",
    "sandbox_workspace_write.network_access=false",
    "-c",
    "sandbox_workspace_write.exclude_slash_tmp=true",
    "-c",
    "sandbox_workspace_write.exclude_tmpdir_env_var=true",
    "-c",
    "model_reasoning_effort=high",
    "-c",
    "mcp_servers={}",
  ]);
  // The owner's own MCP servers are cleared before the sushiai one is added.
  const withMcp = codex.headlessTurnArgs({ effort: "high", mcp: server });
  assert.ok(
    withMcp.indexOf("mcp_servers={}") <
      withMcp.indexOf('mcp_servers.srv.command="cmd"'),
  );
  assert.deepEqual(withMcp.slice(-6, -2), codex.mcpArgs(server));
  assert.deepEqual(withMcp.slice(-2), [
    "-c",
    'mcp_servers.srv.tools.t1.approval_mode="approve"',
  ]);
});

test("the claude builder allowlist holds only the build commands", () => {
  const args = harnessFor("claude").headlessTurnArgs({ effort: "low" });
  const tools = args[args.indexOf("--allowedTools") + 1].split(" ");
  const bash = [];
  // Entries are space separated, so re-read the Bash ones whole.
  const joined = args[args.indexOf("--allowedTools") + 1];
  for (const m of joined.matchAll(/Bash\(([^)]*)\)/g)) bash.push(m[1]);
  assert.deepEqual(bash.sort(), [
    "sushiai hosts:*",
    "sushiai projects:*",
    "sushiai routine dry-run:*",
    "sushiai routine save:*",
    "sushiai routine validate:*",
    "sushiai routines",
    "sushiai routines --json:*",
    "sushiai routines help:*",
    "sushiai routines show:*",
  ]);
  assert.ok(tools.includes("Edit(./**)"));
  assert.doesNotMatch(joined, /propose|sushiai:\*|register|grant|approve/);
});
