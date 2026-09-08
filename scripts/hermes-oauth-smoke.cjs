// Native Hermes OAuth against a disposable local authorization/MCP server.
// No accounts, external services or user credentials are used.
const fs = require("node:fs/promises"),
  http = require("node:http"),
  assert = require("node:assert/strict");
const { createHash, randomUUID } = require("node:crypto");
const { HermesProvider } = require("../electron/agents/hermes-provider.cjs");
const { HermesTransport } = require("../electron/agents/hermes-transport.cjs");
(async () => {
  const home = await fs.mkdtemp("/tmp/sushiai-oauth-");
  const clients = new Map(),
    codes = new Map();
  let exchanges = 0,
    authenticatedProbes = 0,
    origin;
  const json = (res, status, value) => {
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(JSON.stringify(value));
  };
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url, origin);
      if (url.pathname.startsWith("/.well-known/oauth-protected-resource"))
        return json(res, 200, {
          resource: `${origin}/mcp`,
          authorization_servers: [origin],
          scopes_supported: ["tools:read"],
        });
      if (url.pathname.startsWith("/.well-known/oauth-authorization-server"))
        return json(res, 200, {
          issuer: origin,
          authorization_endpoint: `${origin}/authorize`,
          token_endpoint: `${origin}/token`,
          registration_endpoint: `${origin}/register`,
          response_types_supported: ["code"],
          grant_types_supported: ["authorization_code", "refresh_token"],
          code_challenge_methods_supported: ["S256"],
          token_endpoint_auth_methods_supported: ["none"],
          scopes_supported: ["tools:read"],
        });
      if (url.pathname === "/register") {
        let raw = "";
        for await (const chunk of req) raw += chunk;
        const client = JSON.parse(raw),
          id = randomUUID();
        clients.set(id, client);
        return json(res, 201, {
          ...client,
          client_id: id,
          client_id_issued_at: Math.floor(Date.now() / 1000),
        });
      }
      if (url.pathname === "/authorize") {
        const client = clients.get(url.searchParams.get("client_id")),
          redirect = url.searchParams.get("redirect_uri");
        assert.ok(client?.redirect_uris.includes(redirect));
        assert.equal(url.searchParams.get("code_challenge_method"), "S256");
        const code = randomUUID();
        codes.set(code, {
          challenge: url.searchParams.get("code_challenge"),
          redirect,
        });
        const callback = new URL(redirect);
        callback.searchParams.set("state", url.searchParams.get("state"));
        callback.searchParams.set("code", code);
        res.writeHead(302, { Location: callback.href });
        res.end();
        return;
      }
      if (url.pathname === "/token") {
        let raw = "";
        for await (const chunk of req) raw += chunk;
        const body = new URLSearchParams(raw),
          saved = codes.get(body.get("code"));
        assert.ok(saved);
        assert.equal(body.get("redirect_uri"), saved.redirect);
        assert.equal(
          createHash("sha256")
            .update(body.get("code_verifier"))
            .digest("base64url"),
          saved.challenge,
        );
        codes.delete(body.get("code"));
        exchanges++;
        return json(res, 200, {
          access_token: "synthetic-access",
          token_type: "Bearer",
          expires_in: 3600,
          scope: "tools:read",
        });
      }
      if (url.pathname === "/mcp") {
        if (req.headers.authorization !== "Bearer synthetic-access") {
          res.setHeader(
            "WWW-Authenticate",
            `Bearer resource_metadata="${origin}/.well-known/oauth-protected-resource/mcp"`,
          );
          return json(res, 401, { error: "unauthorized" });
        }
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
        let result = {};
        if (rpc.method === "initialize")
          result = {
            protocolVersion: "2025-03-26",
            capabilities: { tools: {} },
            serverInfo: { name: "oauth-test", version: "1" },
          };
        if (rpc.method === "tools/list") {
          authenticatedProbes++;
          result = {
            tools: [
              {
                name: "authorized_tool",
                description: "Synthetic authenticated tool",
                inputSchema: { type: "object", properties: {} },
              },
            ],
          };
        }
        return json(res, 200, { jsonrpc: "2.0", id: rpc.id, result });
      }
      json(res, 404, { error: "not found" });
    } catch (error) {
      json(res, 500, { error: "Synthetic OAuth validation failed" });
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
  await fs.writeFile(
    `${home}/config.yaml`,
    JSON.stringify({
      model: {
        provider: "custom",
        default: "synthetic",
        base_url: origin,
        api_key: "synthetic",
        context_length: 131072,
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
  const call = (action, input = {}) =>
    p.operations.get(`addons.mcp.${action}`)({ agentId: "default", ...input });
  try {
    await p.listAgents();
    await call("create", {
      id: "oauth-test",
      url: `${origin}/mcp`,
      auth: "oauth",
    });
    const flow = await call("authorize", { id: "oauth-test" });
    assert.equal(flow.status, "authorization_required", JSON.stringify(flow));
    const authorization = new URL(flow.authorizationUrl);
    assert.equal(authorization.origin, origin);
    const badCallback = new URL(authorization.searchParams.get("redirect_uri"));
    badCallback.searchParams.set("code", "wrong");
    badCallback.searchParams.set("state", "wrong");
    assert.equal((await fetch(badCallback)).status, 404);
    assert.equal((await fetch(authorization)).status, 200);
    let status;
    const deadline = Date.now() + 30000;
    do {
      status = await call("authStatus", { id: "oauth-test" });
      if (["approved", "error"].includes(status.status)) break;
      await new Promise((r) => setTimeout(r, 100));
    } while (Date.now() < deadline);
    assert.equal(status.status, "approved", JSON.stringify(status));
    assert.equal(exchanges, 1);
    assert.ok(authenticatedProbes > 0);
    assert.equal((await call("test", { id: "oauth-test" })).ok, true);
    await call("create", {
      id: "cancel-test",
      url: `${origin}/mcp`,
      auth: "oauth",
    });
    assert.equal(
      (await call("authorize", { id: "cancel-test" })).status,
      "authorization_required",
    );
    await call("cancelAuth", { id: "cancel-test" });
    assert.equal(
      (await call("authStatus", { id: "cancel-test" })).status,
      "none",
    );
    assert.equal(exchanges, 1);
    console.log(
      JSON.stringify({
        passed: true,
        pkce: true,
        invalidStateRejected: true,
        authenticatedTools: true,
        cancellation: true,
        exchanges,
      }),
    );
  } finally {
    await p.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
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
