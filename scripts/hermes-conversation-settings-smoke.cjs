// Native session settings with disposable data and a local model endpoint.
const fs = require("node:fs/promises"),
  http = require("node:http"),
  assert = require("node:assert/strict");
const { HermesProvider } = require("../electron/agents/hermes-provider.cjs");
const { HermesTransport } = require("../electron/agents/hermes-transport.cjs");
(async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-session-settings-");
  let requestedModel,
    ownedChild,
    stallNext = false,
    requests = 0;
  const server = http.createServer(async (req, res) => {
    res.setHeader("Content-Type", "application/json");
    if (req.url.endsWith("/models")) {
      res.end(JSON.stringify({ data: [{ id: "synthetic-primary" }] }));
      return;
    }
    if (req.method !== "POST" || !req.url.endsWith("/chat/completions")) {
      res.writeHead(404);
      res.end("{}");
      return;
    }
    let raw = "";
    for await (const chunk of req) raw += chunk;
    const body = JSON.parse(raw);
    requestedModel = body.model;
    requests++;
    if (stallNext) {
      stallNext = false;
      return;
    }
    const result = {
      id: "synthetic-completion",
      object: "chat.completion",
      model: body.model,
      choices: [
        {
          index: 0,
          message: { role: "assistant", content: "Synthetic reply." },
          finish_reason: "stop",
        },
      ],
      usage: { prompt_tokens: 10, completion_tokens: 3, total_tokens: 13 },
    };
    if (body.stream) {
      res.setHeader("Content-Type", "text/event-stream");
      res.end(
        `data: ${JSON.stringify({ ...result, object: "chat.completion.chunk", choices: [{ index: 0, delta: { role: "assistant", content: "Synthetic reply." }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
      );
    } else res.end(JSON.stringify(result));
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const base = `http://127.0.0.1:${server.address().port}/v1`;
  await fs.writeFile(
    home + "/config.yaml",
    JSON.stringify({
      model: {
        provider: "custom",
        default: "synthetic-primary",
        base_url: base,
        api_key: "synthetic-test",
      },
      agent: { reasoning_effort: "medium" },
      toolsets: [],
      enabled_toolsets: [],
      auxiliary: { background_review: { enabled: false } },
    }),
  );
  const makeProvider = () =>
    new HermesProvider({
      transportFactory: (o) =>
        new HermesTransport({
          ...o,
          spawn: (...args) => { ownedChild = require("node:child_process").spawn(...args); return ownedChild; },
          spawn: (...args) => {
            ownedChild = require("node:child_process").spawn(...args);
            return ownedChild;
          },
          env: {
            HERMES_HOME: home,
            OPENAI_BASE_URL: base,
            OPENAI_API_KEY: "synthetic-test",
            HERMES_SKIP_UPDATE_CHECK: "1",
          },
        }),
    });
  let p = makeProvider();
  try {
    await p.listAgents();
    const a = await p.create({
        agentId: "default",
        title: "Synthetic settings",
      }),
      b = await p.create({
        agentId: "default",
        title: "Unaffected conversation",
      });
    const input = { agentId: "default", conversationId: a.conversationId },
      other = { agentId: "default", conversationId: b.conversationId };
    const setting = (action, extra = {}) =>
      p.operations.get(`conversations.settings.${action}`)({
        ...input,
        ...extra,
      });
    const suggestions = await setting("options");
    assert.ok(suggestions.providers.some(provider => provider.models.includes("synthetic-primary")));
    assert.ok(!JSON.stringify(suggestions).includes("synthetic-test"));
    await setting("reasoning", { value: "low" });
    assert.equal((await setting("read")).reasoning, "low");
    const result = await setting("model", { model: "synthetic-alternative" });
    assert.equal(result.ok, true);
    assert.equal(result.value, "synthetic-alternative");
    assert.ok(result.warning);
    assert.equal(
      (await p.operations.get("conversations.settings.read")(other)).reasoning,
      "medium",
    );
    const config = await p
      .transport("default")
      .request("GET", "/api/config", { profile: "default" });
    assert.equal(config.model, "synthetic-primary");
    assert.equal(config.agent.reasoning_effort, "medium");
    await p.send({ ...input, text: "Reply with a short synthetic response." });
    const deadline = Date.now() + 30000;
    while (p.snapshot(input).status !== "idle" && Date.now() < deadline)
      await new Promise((r) => setTimeout(r, 100));
    assert.equal(p.snapshot(input).status, "idle");
    assert.equal(requestedModel, "synthetic-alternative");
    assert.equal((await setting("read")).reasoning, "low");
    const detail = await p
      .transport("default")
      .request("GET", `/api/sessions/${a.conversationId}`, {
        profile: "default",
      });
    const metadata =
      typeof detail.model_config === "string"
        ? JSON.parse(detail.model_config)
        : detail.model_config;
    assert.equal(metadata?.reasoning_config?.effort, "low");
    await p.close();
    p = makeProvider();
    await p.listAgents();
    const persisted = await p
      .transport("default")
      .request("GET", `/api/sessions/${a.conversationId}`, {
        profile: "default",
      });
    const persistedConfig =
      typeof persisted.model_config === "string"
        ? JSON.parse(persisted.model_config)
        : persisted.model_config;
    assert.equal(persistedConfig?.reasoning_config?.effort, "low");
    await p.open(input);
    await p.open(other);
    assert.equal(
      (await setting("read")).reasoning,
      "low",
      "Reasoning must survive restart",
    );
    assert.equal(
      (await setting("read")).model,
      "synthetic-alternative",
      "Model must survive restart",
    );
    assert.equal(
      (await p.operations.get("conversations.settings.read")(other)).reasoning,
      "medium",
    );
    requestedModel = null;
    await p.send({ ...input, text: "Reply after the synthetic restart." });
    const resumedDeadline = Date.now() + 30000;
    while (p.snapshot(input).status !== "idle" && Date.now() < resumedDeadline)
      await new Promise((r) => setTimeout(r, 100));
    assert.equal(p.snapshot(input).status, "idle");
    assert.equal(requestedModel, "synthetic-alternative");
    const beforeCrash = requests;
    stallNext = true;
    await p.send({ ...input, text: "Synthetic crash recovery request." });
    const startedDeadline = Date.now() + 30000;
    while (requests === beforeCrash && Date.now() < startedDeadline)
      await new Promise((r) => setTimeout(r, 100));
    assert.equal(requests, beforeCrash + 1);
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
    assert.notEqual(p.get(input).runtime, previousRuntime);
    assert.equal(p.snapshot(input).status, "idle");
    assert.equal(
      requests,
      beforeCrash + 2,
      "Native recovery should continue once without an adapter retry",
    );
    assert.equal(p.get(input).autoContinuing, true);
    assert.ok(
      p
        .snapshot(input)
        .items.some((i) =>
          /according to its recovery settings/.test(i.text || ""),
        ),
    );
    console.log(
      JSON.stringify({
        passed: true,
        nativeConversationSettings: true,
        nativeModelSuggestions: true,
        defaultSettingsUnchanged: true,
        otherConversationUnchanged: true,
        selectedModelExecuted: true,
        warningPreserved: true,
        settingsSurviveRestart: true,
        nativeAutomaticContinuation: true,
        selectedModelExecutedAfterRestart: true,
      }),
    );
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
