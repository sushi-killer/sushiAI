const fs = require("node:fs/promises"),
  assert = require("node:assert/strict");
const { HermesProvider } = require("../electron/agents/hermes-provider.cjs");
const { HermesTransport } = require("../electron/agents/hermes-transport.cjs");
(async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-settings-");
  await fs.writeFile(
    `${home}/config.yaml`,
    JSON.stringify({
      agent: { reasoning_effort: "medium" },
      mcp_servers: {
        "synthetic-command": {
          command: "node",
          args: ["old.js"],
          env: {
            KEEP: "synthetic-secret-keep",
            CHANGE: "synthetic-old",
            REMOVE: "synthetic-remove",
          },
          enabled: false,
          tools: { include: ["read"] },
        },
        "synthetic-http": {
          url: "https://example.com/mcp",
          enabled: false,
          headers: { Authorization: "Bearer synthetic-header" },
        },
      },
    }),
  );
  const p = new HermesProvider({
    transportFactory: (options) =>
      new HermesTransport({
        ...options,
        env: { HERMES_HOME: home, HERMES_SKIP_UPDATE_CHECK: "1" },
      }),
  });
  const call = (operation, input = {}) =>
    p.operations.get(`addons.models.${operation}`)({
      agentId: "default",
      ...input,
    });
  try {
    await p.listAgents();
    assert.equal((await call("reasoning")).value, "medium");
    await call("setReasoning", { value: "none" });
    assert.equal((await call("reasoning")).value, "none");
    await call("setReasoning", { value: "high" });
    assert.equal((await call("reasoning")).value, "high");
    assert.match(
      await fs.readFile(`${home}/config.yaml`, "utf8"),
      /reasoning_effort"?\s*:\s*"?high/,
    );
    const mcp = (operation, input = {}) =>
      p.operations.get(`addons.mcp.${operation}`)({
        agentId: "default",
        ...input,
      });
    const opened = await mcp("read", { id: "synthetic-command" });
    assert.ok(!JSON.stringify(opened).includes("synthetic-secret-keep"));
    await mcp("update", {
      id: opened.id,
      revision: opened.revision,
      command: "node",
      args: ["new.js"],
      env: { CHANGE: "synthetic-new" },
      removeEnv: ["REMOVE"],
    });
    const after = await p
      .transport("default")
      .request("GET", "/api/config", { profile: "default" });
    assert.deepEqual(after.mcp_servers[opened.id].env, {
      KEEP: "synthetic-secret-keep",
      CHANGE: "synthetic-new",
    });
    assert.deepEqual(after.mcp_servers[opened.id].args, ["new.js"]);
    assert.equal(after.mcp_servers[opened.id].enabled, false);
    assert.deepEqual(after.mcp_servers[opened.id].tools, { include: ["read"] });
    assert.equal(
      after.mcp_servers["synthetic-http"].headers.Authorization,
      "Bearer synthetic-header",
    );
    await assert.rejects(
      mcp("update", {
        id: opened.id,
        revision: opened.revision,
        command: "node",
        args: [],
      }),
      /changed since/,
    );
    const http = await mcp("read", { id: "synthetic-http" });
    await mcp("update", {
      id: http.id,
      revision: http.revision,
      url: "https://example.com/edited",
    });
    const updated = await p
      .transport("default")
      .request("GET", "/api/config", { profile: "default" });
    assert.equal(
      updated.mcp_servers[http.id].url,
      "https://example.com/edited",
    );
    assert.equal(
      updated.mcp_servers[http.id].headers.Authorization,
      "Bearer synthetic-header",
    );
    assert.equal(
      updated.mcp_servers[opened.id].env.KEEP,
      "synthetic-secret-keep",
    );
    const recovery = (action, extra = {}) => p.operations.get(`addons.recovery.${action}`)({ agentId: "default", ...extra });
    assert.equal((await recovery("read")).enabled, true);
    await recovery("update", { enabled: false });
    assert.equal((await recovery("read")).enabled, false);
    await recovery("update", { enabled: true });
    assert.equal((await recovery("read")).enabled, true);
    console.log(
      JSON.stringify({
        passed: true,
        nativeReasoningReadWrite: true,
        nativeRecoveryPolicy: true,
        nativeConnectionEdit: true,
        preservedCredentials: true,
        staleEditRejected: true,
      }),
    );
  } finally {
    await p.close();
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
