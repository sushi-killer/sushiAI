const { createHmac, randomBytes } = require("node:crypto");
const { text, object } = require("./registry.cjs");

function serverId(value) {
  const id = text(value, "server name", 128);
  if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(id))
    throw Error(
      "Use letters, numbers, dots, hyphens or underscores for the server name.",
    );
  return id;
}
function publicServer(row) {
  return {
    id: row.name,
    name: row.name,
    transport: row.transport,
    url: row.url,
    command: row.command,
    args: row.args || [],
    environmentKeys: Object.keys(row.env || {}),
    auth: row.auth || "none",
    enabled: row.enabled !== false,
    tools: row.tools,
  };
}
function requireOK(result) {
  if (result?.ok !== true)
    throw Error(
      typeof result?.error === "string"
        ? result.error.slice(0, 2000)
        : "Hermes could not complete this operation.",
    );
  return result;
}
function installHermesMcp(provider) {
  const installs = new Map();
  const revisionKey = randomBytes(32);
  const revision = (agent, id, row) =>
    createHmac("sha256", revisionKey)
      .update(JSON.stringify([agent, id, row]))
      .digest("hex");
  const flows = new Map(),
    locks = new Map();
  const descriptor = {
    id: "mcp",
    name: "Connections",
    description: "MCP servers and authentication",
    operations: [],
  };
  const key = (agent, id) => JSON.stringify([agent, id]);
  const endpoint = (id) => `/api/mcp/servers/${encodeURIComponent(id)}`;
  function add(action, fields, mutates, fn) {
    const name = `addons.mcp.${action}`;
    descriptor.operations.push({ name, mutates });
    provider.operations.set(name, async (input) => {
      object(input);
      if (
        Object.keys(input).some(
          (field) => !["agentId", ...fields].includes(field),
        )
      )
        throw Error("Unknown connection input.");
      const agent = await provider.agent(input);
      const request = (method, path, body) =>
        provider.transport(agent).request(method, path, {
          profile: agent,
          ...(body ? { body: { ...body, profile: agent } } : {}),
        });
      const ctx = { agent, request };
      if (!mutates) return fn(ctx, input);
      const prior = locks.get(agent) || Promise.resolve();
      const work = prior.catch(() => {}).then(() => fn(ctx, input));
      locks.set(agent, work);
      try {
        return await work;
      } finally {
        if (locks.get(agent) === work) locks.delete(agent);
      }
    });
  }
  const snapshot = (result) => ({
    status: result.status,
    authorizationUrl: result.authorization_url || null,
    error: result.error || null,
    tools: result.tools || [],
  });
  const catalog = async (c) => {
    const r = await c.request("GET", "/api/mcp/catalog");
    if (!Array.isArray(r.entries))
      throw Error("Hermes did not return the connection catalog.");
    return r;
  };
  add("catalog", [], false, async (c) => {
    const r = await catalog(c);
    return {
      entries: r.entries.map((e) => ({
        name: e.name,
        description: e.description,
        transport: e.transport,
        authType: e.auth_type,
        requiredEnv: e.required_env || [],
        command: e.command,
        args: e.args || [],
        url: e.url,
        installUrl: e.install_url,
        installRef: e.install_ref,
        bootstrap: e.bootstrap || [],
        postInstall: e.post_install || "",
        installed: !!e.installed,
        enabled: !!e.enabled,
      })),
      diagnostics: r.diagnostics || [],
    };
  });
  add("install", ["id", "env", "enabled"], true, async (c, i) => {
    const id = serverId(i.id),
      entry = (await catalog(c)).entries.find((e) => e.name === id);
    if (!entry)
      throw Error("This connection is no longer in the Hermes catalog.");
    const ongoing = installs.get(key(c.agent, id));
    if (ongoing) {
      const status = await c.request(
        "GET",
        `/api/actions/${encodeURIComponent(ongoing)}/status`,
      );
      if (status.running) return { status: "running" };
    }
    if (entry.installed) return { status: "installed" };
    if (i.enabled !== undefined && typeof i.enabled !== "boolean")
      throw Error("Invalid enabled state.");
    const env = i.env === undefined ? {} : object(i.env),
      allowed = new Set((entry.required_env || []).map((v) => v.name));
    for (const [k, v] of Object.entries(env))
      if (
        !allowed.has(k) ||
        typeof v !== "string" ||
        v.length > 16000 ||
        v.includes("\0")
      )
        throw Error("Invalid catalog credential.");
    for (const field of entry.required_env || [])
      if (field.required && !env[field.name]?.trim())
        throw Error(`Provide ${field.prompt || field.name}.`);
    const result = requireOK(
      await c.request("POST", "/api/mcp/catalog/install", {
        name: id,
        env,
        enable: i.enabled !== false,
      }),
    );
    if (result.background) {
      const action = text(result.action, "installation action", 160);
      if (!/^mcp-install-[a-z0-9-]+$/.test(action))
        throw Error("Unexpected installation action.");
      installs.set(key(c.agent, id), action);
      return { status: "running" };
    }
    installs.delete(key(c.agent, id));
    return { status: "installed" };
  });
  add("installStatus", ["id"], false, async (c, i) => {
    const id = serverId(i.id),
      action = installs.get(key(c.agent, id));
    if (!action) return { status: "none" };
    const r = await c.request(
      "GET",
      `/api/actions/${encodeURIComponent(action)}/status`,
    );
    return {
      status: r.running
        ? "running"
        : r.exit_code === 0
          ? "installed"
          : r.exit_code == null
            ? "unknown"
            : "failed",
      exitCode: r.exit_code,
      lines: Array.isArray(r.lines)
        ? r.lines.slice(-100).map((v) => String(v).slice(0, 2000))
        : [],
    };
  });
  add(
    "toolPolicy",
    ["id", "include", "exclude", "prompts", "resources"],
    true,
    async (c, i) => {
      const id = serverId(i.id);
      const rows = await c.request("GET", "/api/mcp/servers");
      if (!rows.servers?.some((s) => s.name === id))
        throw Error("Connection no longer exists.");
      const names = (value) => {
        if (
          !Array.isArray(value) ||
          value.length > 500 ||
          value.some(
            (v) =>
              typeof v !== "string" ||
              !v.trim() ||
              v.length > 200 ||
              v.includes("\0"),
          )
        )
          throw Error("Tool filters must be a list of names or patterns.");
        return [...new Set(value)];
      };
      const include = i.include === null ? null : names(i.include),
        exclude = names(i.exclude);
      if (typeof i.prompts !== "boolean" || typeof i.resources !== "boolean")
        throw Error("Invalid resource settings.");
      requireOK(
        await c.request("PUT", "/api/config", {
          config: {
            mcp_servers: {
              [id]: {
                tools: {
                  include,
                  exclude,
                  prompts: i.prompts,
                  resources: i.resources,
                },
              },
            },
          },
        }),
      );
      return { ok: true };
    },
  );
  add("list", [], false, async (c) => {
    const r = await c.request("GET", "/api/mcp/servers");
    if (!Array.isArray(r.servers)) throw Error("Invalid connection list.");
    return { servers: r.servers.map(publicServer) };
  });
  add(
    "create",
    ["id", "url", "command", "args", "env", "auth", "bearerToken"],
    true,
    async (c, i) => {
      const name = serverId(i.id),
        body = { name, auth: i.auth || "none" };
      if (!["none", "oauth", "header"].includes(body.auth))
        throw Error("Invalid authentication type.");
      if (Boolean(i.url) === Boolean(i.command))
        throw Error("Provide a server URL or command.");
      if (i.url) {
        const url = new URL(text(i.url, "server URL", 4000));
        if (
          !["http:", "https:"].includes(url.protocol) ||
          url.username ||
          url.password
        )
          throw Error("Use an HTTP or HTTPS URL without embedded credentials.");
        body.url = url.href;
        if (i.args?.length || (i.env && Object.keys(i.env).length))
          throw Error(
            "Arguments and environment apply only to command servers.",
          );
      } else {
        body.command = text(i.command, "server command", 4000);
        if (body.auth !== "none" || i.bearerToken)
          throw Error("Command servers use environment credentials.");
        if (i.args !== undefined) {
          if (
            !Array.isArray(i.args) ||
            i.args.length > 100 ||
            i.args.some(
              (a) =>
                typeof a !== "string" || a.length > 4000 || a.includes("\0"),
            )
          )
            throw Error("Arguments must be a list of strings.");
          body.args = i.args;
        }
        if (i.env !== undefined) {
          object(i.env);
          if (Object.keys(i.env).length > 64)
            throw Error("Too many environment variables.");
          for (const [k, v] of Object.entries(i.env))
            if (
              !/^[A-Za-z_][A-Za-z0-9_]*$/.test(k) ||
              typeof v !== "string" ||
              v.length > 16000 ||
              v.includes("\0")
            )
              throw Error("Invalid environment variable.");
          body.env = i.env;
        }
      }
      if (body.auth === "header")
        body.bearer_token = text(i.bearerToken, "bearer token", 16000);
      else if (i.bearerToken)
        throw Error("Bearer token requires header authentication.");
      return publicServer(await c.request("POST", "/api/mcp/servers", body));
    },
  );
  // Use Hermes's validated full-map save. Secret values stay in main; edits carry
  // only explicit replacements and a revision of the selected entry.
  add("read", ["id"], false, async (c, i) => {
    const id = serverId(i.id);
    const config = await c.request("GET", "/api/config");
    const row = config.mcp_servers?.[id];
    if (!row || typeof row !== "object")
      throw Error("Connection no longer exists.");
    const listed = await c.request("GET", "/api/mcp/servers");
    const summary = listed.servers?.find((s) => s.name === id);
    if (!summary) throw Error("Connection no longer exists.");
    return { ...publicServer(summary), revision: revision(c.agent, id, row) };
  });
  add(
    "update",
    ["id", "revision", "url", "command", "args", "env", "removeEnv"],
    true,
    async (c, i) => {
      const id = serverId(i.id);
      const config = await c.request("GET", "/api/config");
      const servers = object(config.mcp_servers),
        row = servers[id];
      if (!row || typeof row !== "object")
        throw Error("Connection no longer exists.");
      if (
        text(i.revision, "connection revision", 64) !==
        revision(c.agent, id, row)
      )
        throw Error(
          "Connection changed since you opened it. Reopen the editor before saving.",
        );
      const flow = flows.get(key(c.agent, id));
      if (flow && flow.expires > Date.now()) {
        const status = await c.request(
          "GET",
          `/api/mcp/oauth/flows/${encodeURIComponent(flow.id)}`,
        );
        if (
          !["approved", "error", "cancelled", "expired"].includes(status.status)
        )
          throw Error(
            "Finish or cancel sign-in before editing this connection.",
          );
      }
      const edited = { ...row };
      if (row.url) {
        if (
          i.command !== undefined ||
          i.args !== undefined ||
          i.env !== undefined ||
          i.removeEnv !== undefined
        )
          throw Error(
            "Arguments and environment apply only to command servers.",
          );
        const url = new URL(text(i.url, "server URL", 4000));
        if (
          !["http:", "https:"].includes(url.protocol) ||
          url.username ||
          url.password
        )
          throw Error("Use an HTTP or HTTPS URL without embedded credentials.");
        edited.url = url.href;
      } else if (row.command) {
        if (i.url !== undefined)
          throw Error("This connection uses a local command.");
        edited.command = text(i.command, "server command", 4000);
        if (
          !Array.isArray(i.args) ||
          i.args.length > 100 ||
          i.args.some(
            (a) => typeof a !== "string" || a.length > 4000 || a.includes("\0"),
          )
        )
          throw Error("Arguments must be a list of strings.");
        edited.args = i.args;
        const env = object(i.env || {}),
          remove = i.removeEnv || [];
        const validKey = (k) =>
          /^[A-Za-z_][A-Za-z0-9_]*$/.test(k) &&
          !["__proto__", "constructor", "prototype"].includes(k);
        if (
          Object.keys(env).length > 64 ||
          Object.entries(env).some(
            ([k, v]) =>
              !validKey(k) ||
              typeof v !== "string" ||
              v.length > 16000 ||
              v.includes("\0"),
          )
        )
          throw Error("Invalid environment variable.");
        if (
          !Array.isArray(remove) ||
          remove.length > 64 ||
          remove.some(
            (k) =>
              typeof k !== "string" || !validKey(k) || Object.hasOwn(env, k),
          )
        )
          throw Error("Choose separate variables to replace and remove.");
        edited.env = { ...row.env, ...env };
        for (const name of remove) delete edited.env[name];
      } else throw Error("This connection has an unsupported transport.");
      requireOK(
        await c.request("PUT", "/api/mcp/servers", {
          servers: { ...servers, [id]: edited },
        }),
      );
      return { ok: true };
    },
  );
  add("toggle", ["id", "enabled"], true, async (c, i) => {
    if (typeof i.enabled !== "boolean") throw Error("Invalid enabled state.");
    return requireOK(
      await c.request("PUT", `${endpoint(serverId(i.id))}/enabled`, {
        enabled: i.enabled,
      }),
    );
  });
  add("remove", ["id"], true, async (c, i) =>
    requireOK(await c.request("DELETE", endpoint(serverId(i.id)))),
  );
  add("test", ["id"], false, async (c, i) => {
    const r = await c.request("POST", `${endpoint(serverId(i.id))}/test`);
    return {
      ok: r.ok === true,
      error: typeof r.error === "string" ? r.error.slice(0, 2000) : null,
      tools: Array.isArray(r.tools) ? r.tools : [],
      prompts: r.prompts || 0,
      resources: r.resources || 0,
    };
  });
  add("authorize", ["id"], true, async (c, i) => {
    const id = serverId(i.id),
      k = key(c.agent, id);
    const existing = flows.get(k);
    if (existing && existing.expires > Date.now()) {
      const r = await c.request(
        "GET",
        `/api/mcp/oauth/flows/${encodeURIComponent(existing.id)}`,
      );
      if (!["approved", "error"].includes(r.status)) return snapshot(r);
    }
    for (const [k, v] of flows) if (v.expires < Date.now()) flows.delete(k);
    if (flows.size >= 64)
      throw Error(
        "Too many authentication flows. Cancel an existing one first.",
      );
    const r = await c.request("POST", `${endpoint(id)}/auth`);
    flows.set(k, {
      id: text(r.flow_id, "authentication flow", 200),
      expires: Date.now() + 15 * 60 * 1000,
    });
    return snapshot(r);
  });
  for (const action of ["authStatus", "cancelAuth"])
    add(action, ["id"], action === "cancelAuth", async (c, i) => {
      const k = key(c.agent, serverId(i.id)),
        flow = flows.get(k);
      if (!flow) return { status: "none" };
      if (flow.expires < Date.now()) {
        flows.delete(k);
        return { status: "expired" };
      }
      const r = await c.request(
        action === "authStatus" ? "GET" : "DELETE",
        `/api/mcp/oauth/flows/${encodeURIComponent(flow.id)}`,
      );
      if (action === "cancelAuth") flows.delete(k);
      return snapshot(r);
    });
  provider.descriptor.addons.push(descriptor);
}
module.exports = { installHermesMcp };
