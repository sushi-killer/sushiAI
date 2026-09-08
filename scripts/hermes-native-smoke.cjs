// Real installed Hermes runtime, isolated home, deterministic local model.
// No user conversations, credentials, memories or skills are copied.
const fs = require("node:fs/promises");
const http = require("node:http");
const { spawn } = require("node:child_process");
const assert = require("node:assert/strict");
const { HermesProvider } = require("../electron/agents/hermes-provider.cjs");
const { HermesTransport } = require("../electron/agents/hermes-transport.cjs");

(async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-hermes-native-");
  let stallNext = false,
    scheduleMode = false,
    clarifyMode = false;
  let requests = 0,
    reviewRequests = 0;
  const server = http.createServer(async (req, res) => {
    if (req.url === "/mcp") {
      if (req.method !== "POST") {
        res.writeHead(405);
        res.end();
        return;
      }
      let raw = "";
      for await (const chunk of req) raw += chunk;
      const rpc = JSON.parse(raw);
      if (rpc.id === undefined) {
        res.writeHead(202);
        res.end();
        return;
      }
      const result =
        rpc.method === "initialize"
          ? {
              protocolVersion: "2025-03-26",
              capabilities: { tools: {} },
              serverInfo: { name: "sushiai-test", version: "1" },
            }
          : rpc.method === "tools/list"
            ? {
                tools: [
                  {
                    name: "synthetic_check",
                    description: "Synthetic connection check",
                    inputSchema: { type: "object", properties: {} },
                  },
                ],
              }
            : rpc.method === "resources/list"
              ? { resources: [] }
              : rpc.method === "prompts/list"
                ? { prompts: [] }
                : {};
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify({ jsonrpc: "2.0", id: rpc.id, result }));
      return;
    }
    if (req.url.endsWith("/models")) {
      res.setHeader("Content-Type", "application/json");
      res.end(
        JSON.stringify({
          object: "list",
          data: [{ id: "sushiai-test-model", object: "model" }],
        }),
      );
      return;
    }
    if (!req.url.endsWith("/chat/completions")) {
      res.writeHead(404);
      res.end();
      return;
    }
    let data = "";
    for await (const chunk of req) data += chunk;
    const body = JSON.parse(data);
    requests++;
    if (stallNext) {
      stallNext = false;
      return;
    }
    const history = body.messages || [];
    const done = history
      .flatMap((m) => m.tool_calls || [])
      .map((c) => c.function?.name);
    const available = (body.tools || []).map((t) => t.function?.name);
    const lastUser = history.findLast((m) => m.role === "user");
    const review =
      done.includes("skill_manage") &&
      !String(lastUser?.content).startsWith("Save the synthetic review rule");
    if (review) reviewRequests++;
    let calls;
    if (
      review &&
      available.includes("memory") &&
      !history.some((m) =>
        (m.tool_calls || []).some((c) => c.id === "native-review"),
      )
    )
      calls = [
        {
          id: "native-review",
          type: "function",
          function: {
            name: "memory",
            arguments: JSON.stringify({
              action: "add",
              target: "memory",
              content:
                "Synthetic background review: verify notifications after completion.",
            }),
          },
        },
      ];
    else if (available.includes("memory") && !done.includes("memory"))
      calls = [
        {
          id: "native-memory",
          type: "function",
          function: {
            name: "memory",
            arguments: JSON.stringify({
              action: "add",
              target: "memory",
              content: "Synthetic smoke test: review before publishing.",
            }),
          },
        },
      ];
    else if (
      available.includes("skill_manage") &&
      !done.includes("skill_manage")
    )
      calls = [
        {
          id: "native-skill",
          type: "function",
          function: {
            name: "skill_manage",
            arguments: JSON.stringify({
              action: "create",
              name: "sushiai-native-test",
              content:
                "---\nname: sushiai-native-test\ndescription: Synthetic integration test skill.\n---\n\n# Review\nCheck the build before publishing.\n",
            }),
          },
        },
      ];
    if (scheduleMode) calls = undefined;
    if (clarifyMode)
      calls =
        available.includes("clarify") && !done.includes("clarify")
          ? [
              {
                id: "native-clarify",
                type: "function",
                function: {
                  name: "clarify",
                  arguments: JSON.stringify({
                    questions: [
                      { id: "goal", question: "What is the goal?" },
                      { id: "format", question: "Which format?" },
                    ],
                  }),
                },
              },
            ]
          : undefined;
    const message = {
      role: "assistant",
      content: calls ? null : "Native integration completed.",
      ...(calls ? { tool_calls: calls } : {}),
    };
    const base = {
      id: "test-completion",
      object: "chat.completion",
      created: Math.floor(Date.now() / 1000),
      model: "sushiai-test-model",
    };
    if (body.stream) {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write(
        `data: ${JSON.stringify({ ...base, object: "chat.completion.chunk", choices: [{ index: 0, delta: { ...message, ...(calls ? { tool_calls: calls.map((c, index) => ({ ...c, index })) } : {}) }, finish_reason: null }] })}\n\n`,
      );
      res.write(
        `data: ${JSON.stringify({ ...base, object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: calls ? "tool_calls" : "stop" }], usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 } })}\n\n`,
      );
      res.end("data: [DONE]\n\n");
    } else {
      res.setHeader("Content-Type", "application/json");
      res.end(
        JSON.stringify({
          ...base,
          choices: [
            { index: 0, message, finish_reason: calls ? "tool_calls" : "stop" },
          ],
          usage: {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
          },
        }),
      );
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const baseURL = `http://127.0.0.1:${server.address().port}/v1`;
  await fs.writeFile(
    home + "/config.yaml",
    JSON.stringify({
      model: {
        provider: "custom",
        default: "sushiai-test-model",
        base_url: baseURL,
        api_key: "synthetic-local-test",
        context_length: 131072,
      },
      toolsets: ["memory", "skills", "clarify"],
      enabled_toolsets: ["memory", "skills", "clarify"],
      auxiliary: { background_review: { enabled: true } },
      desktop: { auto_continue: { enabled: false } },
      memory: {
        memory_enabled: true,
        user_profile_enabled: true,
        nudge_interval: 1,
      },
      skills: { creation_nudge_interval: 1 },
      display: { memory_notifications: "on" },
    }),
  );
  let ownedChild;
  const makeProvider = () =>
    new HermesProvider({
      transportFactory: (options) =>
        new HermesTransport({
          ...options,
          spawn: (...args) => {
            ownedChild = spawn(...args);
            return ownedChild;
          },
          env: {
            ...options.env,
            HERMES_HOME: home,
            OPENAI_BASE_URL: baseURL,
            OPENAI_API_KEY: "synthetic-local-test",
            HERMES_SKIP_UPDATE_CHECK: "1",
          },
        }),
    });
  let p = makeProvider();
  try {
    await p.listAgents();
    const mcp = (action, input = {}) =>
      p.operations.get(`addons.mcp.${action}`)({
        agentId: "default",
        ...input,
      });
    await mcp("create", {
      id: "synthetic-mcp",
      url: `http://127.0.0.1:${server.address().port}/mcp`,
    });
    assert.ok(
      (await mcp("list")).servers.some((s) => s.id === "synthetic-mcp"),
    );
    await mcp("toolPolicy", {
      id: "synthetic-mcp",
      include: [],
      exclude: [],
      prompts: false,
      resources: false,
    });
    const policy = (await mcp("list")).servers.find(
      (s) => s.id === "synthetic-mcp",
    ).tools;
    assert.deepEqual(policy.include, []);
    assert.equal(policy.resources, false);
    await mcp("toolPolicy", {
      id: "synthetic-mcp",
      include: null,
      exclude: [],
      prompts: true,
      resources: true,
    });
    const probe = await mcp("test", { id: "synthetic-mcp" });
    assert.equal(probe.ok, true, JSON.stringify(probe));
    assert.ok(probe.tools.some((t) => t.name.includes("synthetic_check")));
    await mcp("toggle", { id: "synthetic-mcp", enabled: false });
    assert.equal(
      (await mcp("list")).servers.find((s) => s.id === "synthetic-mcp").enabled,
      false,
    );
    await mcp("remove", { id: "synthetic-mcp" });
    assert.equal(
      (await mcp("list")).servers.some((s) => s.id === "synthetic-mcp"),
      false,
    );
    const catalogEntries = (await mcp("catalog")).entries;
    assert.ok(Array.isArray(catalogEntries));
    const installable = catalogEntries.find(
      (e) =>
        e.url &&
        !e.installUrl &&
        !e.requiredEnv.some((v) => v.required) &&
        !e.installed,
    );
    let nativeCatalogInstall = false;
    if (installable) {
      const installed = await mcp("install", {
        id: installable.name,
        env: {},
        enabled: false,
      });
      assert.equal(installed.status, "installed");
      assert.ok(
        (await mcp("list")).servers.some((s) => s.id === installable.name),
      );
      await mcp("remove", { id: installable.name });
      nativeCatalogInstall = true;
    }
    const schedules = (action, input = {}) =>
      p.operations.get(`addons.schedules.${action}`)({
        agentId: "default",
        ...input,
      });
    const scheduled = await schedules("create", {
      name: "Synthetic schedule",
      prompt: "Synthetic schedule check.",
      schedule: "every 1d",
      deliver: "local",
    });
    const jobId = scheduled.resource.id;
    assert.equal(scheduled.resource.data.scheduleExpression, "every 1440m");
    await schedules("pause", { id: jobId });
    scheduleMode = true;
    assert.equal((await schedules("trigger", { id: jobId })).status, "running");
    let jobState;
    const jobDeadline = Date.now() + 90000;
    do {
      jobState = await schedules("triggerStatus", { id: jobId });
      if (jobState.status !== "running") break;
      await new Promise((r) => setTimeout(r, 100));
    } while (Date.now() < jobDeadline);
    assert.equal(jobState.status, "completed", JSON.stringify(jobState));
    const jobRuns = await schedules("runs", { id: jobId });
    assert.ok(jobRuns.runs.length > 0);
    await schedules("update", {
      id: jobId,
      updates: { model: "", provider: "", deliver: "" },
    });
    await schedules("delete", { id: jobId });
    scheduleMode = false;
    scheduleMode = true;
    const automatic = await schedules("create", {
      name: "Synthetic automatic schedule",
      prompt: "Synthetic schedule check.",
      schedule: new Date(Date.now() + 1000).toISOString(),
      deliver: "local",
    });
    await schedules("schedulerSet", { enabled: true });
    assert.equal((await schedules("schedulerStatus")).enabled, true);
    let autoRuns = [];
    const autoDeadline = Date.now() + 85000;
    do {
      autoRuns = (await schedules("runs", { id: automatic.resource.id })).runs;
      if (autoRuns.some((r) => r.ended_at)) break;
      await new Promise((r) => setTimeout(r, 500));
    } while (Date.now() < autoDeadline);
    assert.ok(
      autoRuns.some((r) => r.ended_at),
      "The native automatic ticker must finish the scheduled run",
    );
    await schedules("schedulerSet", { enabled: false });
    assert.equal((await schedules("schedulerStatus")).enabled, false);
    scheduleMode = false;
    const s = await p.create({
      agentId: "default",
      title: "sushiAI native test",
    });
    const input = { agentId: "default", conversationId: s.conversationId };
    await p.close();
    p = makeProvider();
    await p.listAgents();
    const reopened = await p.open({ ...input, title: s.title });
    assert.equal(reopened.conversationId, s.conversationId);
    assert.equal(reopened.items.length, 0);
    await p.send({
      ...input,
      text: "Save the synthetic review rule in memory, create the synthetic review skill, then confirm.",
    });
    const deadline = Date.now() + 90000;
    while (Date.now() < deadline) {
      const state = p.snapshot(input);
      if (!["sending", "running", "waiting"].includes(state.status)) break;
      await new Promise((r) => setTimeout(r, 100));
    }
    const reviewDeadline = Date.now() + 60000;
    while (
      Date.now() < reviewDeadline &&
      !p.activities.some((a) => a.kind === "self-improvement")
    )
      await new Promise((r) => setTimeout(r, 100));
    const state = p.snapshot(input);
    const activities = await p.operations.get("activity.list")({});
    assert.equal(
      state.status,
      "idle",
      JSON.stringify({
        status: state.status,
        notices: state.items
          .filter((i) => i.kind === "notice")
          .map((i) => i.text),
      }),
    );
    assert.ok(
      activities.some((a) => a.kind === "memory"),
      JSON.stringify({
        activities,
        toolResults: state.items.filter((i) => i.kind === "tool"),
      }),
    );
    assert.ok(
      activities.some((a) => a.kind === "skills"),
      JSON.stringify({
        activities,
        toolResults: state.items.filter((i) => i.kind === "tool"),
      }),
    );
    assert.ok(
      activities.some((a) => a.kind === "self-improvement"),
      JSON.stringify({ reviewRequests, activities }),
    );
    const memory = await fs.readFile(home + "/memories/MEMORY.md", "utf8");
    assert.match(memory, /Synthetic smoke test/);
    assert.match(memory, /Synthetic background review/);
    const beforeRestartRequests = requests;
    const previousRuntime = p.get(input).runtime;
    ownedChild.kill("SIGKILL");
    const recoveryDeadline = Date.now() + 60000;
    while (Date.now() < recoveryDeadline) {
      if (
        p.get(input).runtime !== previousRuntime &&
        !p.recoveries.size &&
        p.snapshot(input).status === "idle"
      )
        break;
      await new Promise((r) => setTimeout(r, 100));
    }
    assert.notEqual(
      p.get(input).runtime,
      previousRuntime,
      "Owned backend must be recovered",
    );
    assert.equal(p.snapshot(input).status, "idle");
    assert.equal(
      requests,
      beforeRestartRequests,
      "Recovery must never resend prompts",
    );
    assert.ok(
      p
        .snapshot(input)
        .items.some((i) => i.kind === "tool" && i.name === "memory"),
    );
    stallNext = true;
    const beforeInterrupt = requests;
    await p.send({
      ...input,
      text: "Synthetic interruption: wait for the controlled test.",
    });
    const requestDeadline = Date.now() + 30000;
    while (requests === beforeInterrupt && Date.now() < requestDeadline)
      await new Promise((r) => setTimeout(r, 100));
    assert.ok(
      requests > beforeInterrupt,
      "The interrupted request reached the local model",
    );
    const interruptedCount = requests,
      interruptedRuntime = p.get(input).runtime;
    ownedChild.kill("SIGKILL");
    const interruptedDeadline = Date.now() + 60000;
    while (Date.now() < interruptedDeadline) {
      if (
        p.get(input).runtime !== interruptedRuntime &&
        !p.recoveries.size &&
        p.snapshot(input).status === "idle"
      )
        break;
      await new Promise((r) => setTimeout(r, 100));
    }
    assert.notEqual(p.get(input).runtime, interruptedRuntime);
    assert.equal(p.snapshot(input).status, "idle");
    assert.equal(
      requests,
      interruptedCount,
      "An interrupted prompt must not be submitted twice",
    );

    clarifyMode = true;
    const clarifySession = await p.create({
      agentId: "default",
      title: "Synthetic clarification",
    });
    const clarifyInput = {
      agentId: "default",
      conversationId: clarifySession.conversationId,
    };
    await p.send({
      ...clarifyInput,
      text: "Ask the synthetic clarification questions.",
    });
    const clarifyDeadline = Date.now() + 30000;
    while (
      !p.snapshot(clarifyInput).requests.length &&
      Date.now() < clarifyDeadline
    )
      await new Promise((r) => setTimeout(r, 100));
    const pending = p.snapshot(clarifyInput).requests[0];
    assert.equal(
      pending?.kind,
      "clarify",
      "Native tool must request clarification",
    );
    assert.deepEqual(
      pending.input.questions.map((q) => q.qid),
      ["q0", "q1"],
    );
    const first = await p.respond({
      ...clarifyInput,
      requestId: pending.id,
      response: { questionId: "q0", answer: "Release" },
    });
    assert.deepEqual(first.remaining, ["q1"]);
    await p.respond({
      ...clarifyInput,
      requestId: pending.id,
      response: { questionId: "q1", answer: "Markdown" },
    });
    const completionDeadline = Date.now() + 30000;
    while (
      p.snapshot(clarifyInput).status !== "idle" &&
      Date.now() < completionDeadline
    )
      await new Promise((r) => setTimeout(r, 100));
    const clarified = p.snapshot(clarifyInput);
    assert.equal(clarified.status, "idle");
    assert.deepEqual(
      clarified.items
        .find((i) => i.kind === "tool" && i.name === "clarify")
        .output.responses.map((r) => r.user_response),
      ["Release", "Markdown"],
    );

    console.log(
      JSON.stringify({
        passed: true,
        nativeBatchClarification: true,
        emptyConversationRestart: true,
        nativeMcpLifecycle: true,
        nativeScheduleRun: true,
        nativeAutomaticSchedule: true,
        nativeCatalogEntries: catalogEntries.length,
        nativeCatalogInstall,
        backendCrashRecovery: true,
        interruptedPromptNotResent: true,
        modelRequests: requests,
        reviewRequests,
        activities: activities.map((a) => a.kind),
        nativeTools: state.items
          .filter((i) => i.kind === "tool")
          .map((i) => i.name),
      }),
    );
  } catch (e) {
    console.error("Native check:", e.message);
    throw e;
  } finally {
    await p.close();
    server.closeAllConnections();
    await new Promise((r) => server.close(r));
    await fs.rm(home, {
      recursive: true,
      force: true,
      maxRetries: 5,
      retryDelay: 200,
    });
  }
})().catch((e) => {
  console.error(e.message);
  process.exitCode = 1;
});
