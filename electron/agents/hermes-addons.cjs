"use strict";

// Fixed native routes only. The resolved agent owns every request and resource.
const CONTENT_LIMIT = 200000;
const str = (max = 200, min = 1, pattern) => ({
  type: "string",
  minLength: min,
  maxLength: max,
  ...(pattern ? { pattern } : {}),
});
const bool = { type: "boolean" };
const ID = str(200, 1, "^[A-Za-z0-9][A-Za-z0-9_.-]*$");
const MEMORY_ID = str(100, 1, "^memory:(memory|profile):[0-9]{1,9}$");
const CONTENT = str(CONTENT_LIMIT, 0);
const schema = (properties = {}, required = []) => ({
  type: "object",
  additionalProperties: false,
  properties: { agentId: ID, ...properties },
  required: ["agentId", ...required],
});

function fail(message, code = "INVALID_ADDON_INPUT", details) {
  const error = new Error(message);
  error.code = code;
  if (details !== undefined) error.details = details;
  throw error;
}
function validate(value, rule, path = "input") {
  if (rule.type === "object") {
    if (
      !value ||
      typeof value !== "object" ||
      Array.isArray(value) ||
      ![Object.prototype, null].includes(Object.getPrototypeOf(value))
    )
      fail(`${path} must be an object.`);
    for (const key of Object.keys(value)) {
      if (!Object.hasOwn(rule.properties, key)) fail(`Unknown ${path}.${key}.`);
      validate(value[key], rule.properties[key], `${path}.${key}`);
    }
    for (const key of rule.required || [])
      if (!Object.hasOwn(value, key)) fail(`${path}.${key} is required.`);
    if (rule.minProperties && !Object.keys(value).length)
      fail(`${path} must not be empty.`);
  } else if (rule.type === "string") {
    if (
      typeof value !== "string" ||
      value.length < rule.minLength ||
      value.length > rule.maxLength ||
      value.includes("\0") ||
      (rule.minLength && !value.trim()) ||
      (rule.pattern && !new RegExp(rule.pattern).test(value))
    )
      fail(`Invalid ${path}.`);
  } else if (rule.type === "boolean" && typeof value !== "boolean")
    fail(`${path} must be boolean.`);
  else if (
    rule.type === "integer" &&
    (!Number.isInteger(value) || value < rule.minimum || value > rule.maximum)
  )
    fail(`Invalid ${path}.`);
}
function checked(result, flag = "ok", applied) {
  if (!result || typeof result !== "object")
    fail("Invalid Hermes result.", "HERMES_INVALID_RESULT");
  if(result.confirm_required===true && !result.error && result.success!==false && !applied)return result;
  if (
    result.error ||
    result.ok === false ||
    result.success === false ||
    (applied && result.applied?.[applied] !== true)
  )
    fail(
      result.message ||
        result.error?.message ||
        (typeof result.error === "string"
          ? result.error
          : "Hermes rejected the change."),
      "HERMES_REJECTED",
      result,
    );
  if (result.confirm_required === true) return result;
  if (flag && result[flag] !== true)
    fail("Hermes did not confirm the change.", "HERMES_INVALID_RESULT", result);
  return result;
}
function array(value) {
  if (!Array.isArray(value))
    fail("Invalid Hermes list.", "HERMES_INVALID_RESULT");
  return value;
}
const pick = (value, keys) =>
  Object.fromEntries(
    keys.filter((k) => value[k] !== undefined).map((k) => [k, value[k]]),
  );
const resource = (agentId, kind, id, data) => ({ agentId, kind, id, data });
function scheduleResource(p, row) {
  checked(row, null);
  validate(row.id, ID, "schedule ID");
  return resource(p, "schedule", row.id, {
    ...pick(row, [
      "name",
      "prompt",
      "enabled",
      "schedule",
      "deliver",
      "model",
      "provider",
      "state",
    ]),
    scheduleExpression:
      row.schedule?.expr ||
      (row.schedule?.kind === "interval"
        ? `every ${row.schedule.minutes}m`
        : row.schedule?.run_at) ||
      "",
    scheduleDisplay: row.schedule_display ?? null,
    nextRunAt: row.next_run_at ?? null,
    lastRunAt: row.last_run_at ?? null,
    lastError: row.last_error ?? null,
  });
}
const scheduleFields = {
  name: str(200, 0),
  prompt: CONTENT,
  schedule: str(1000),
  deliver: str(1000, 0),
  model: str(500, 0),
  provider: str(200, 0),
};

function installHermesAddons(provider) {
  if (
    !(provider.operations instanceof Map) ||
    !provider.descriptor ||
    !Array.isArray(provider.descriptor.addons)
  )
    throw new TypeError(
      "Expected a Hermes provider with operations and addons.",
    );
  const operations = new Map(),
    descriptors = new Map(),
    locks = new Map(),
    triggers = new Map();
  // Serialize this adapter's writes per profile: positional memory IDs can shift
  // when a different entry is deleted. Native writers still require a server CAS.
  async function locked(profile, fn) {
    const prior = locks.get(profile) || Promise.resolve();
    const run = prior.catch(() => {}).then(fn);
    locks.set(profile, run);
    try {
      return await run;
    } finally {
      if (locks.get(profile) === run) locks.delete(profile);
    }
  }
  function add(group, action, fields, required, mutates, fn) {
    const name = `addons.${group}.${action}`,
      inputSchema = schema(fields, required);
    if (!descriptors.has(group))
      descriptors.set(group, {
        id: group,
        title: group[0].toUpperCase() + group.slice(1),
        resourceKind:
          {
            skills: "skill",
            tools: "toolset",
            models: "model",
            schedules: "schedule",
          }[group] || group,
        operations: [],
      });
    descriptors.get(group).operations.push({ name, mutates, inputSchema });
    operations.set(name, async (input) => {
      validate(input, inputSchema);
      const p = await provider.agent(input);
      if (p !== input.agentId)
        fail(
          "Resolved agent does not match the requested profile.",
          "PROFILE_MISMATCH",
        );
      const t = provider.transport(p);
      const ctx = {
        p,
        rest: (method, path, body, query, timeoutMs) =>
          t.request(method, path, {
            profile: p,
            ...(timeoutMs ? { timeoutMs } : {}),
            ...(query ? { query } : {}),
            ...(body ? { body } : {}),
          }),
        rpc: (method, params = {}) =>
          t.rpc(p, method, { ...params, profile: p }),
      };
      return mutates ? locked(p, () => fn(ctx, input)) : fn(ctx, input);
    });
  }
  const describe = (c) => c.rpc("profiles.describe", { name: c.p });
  const detail = async (c, id) =>
    checked(await c.rest("GET", "/api/learning/node", null, { id }));
  const skillRead = (c, id) =>
    c.rest("GET", "/api/skills/content", null, { name: id });
  function expected(actual, wanted) {
    if (typeof actual !== "string")
      fail("Hermes did not return editable content.", "HERMES_INVALID_RESULT");
    if (actual !== wanted)
      fail(
        "Content changed. Reload this resource before saving.",
        "CONTENT_CONFLICT",
      );
  }
  const editFields = { id: ID, content: CONTENT, expectedContent: CONTENT };
  add("profile", "read", {}, [], false, async (c) => {
    const r = await describe(c);
    return resource(
      c.p,
      "profile",
      c.p,
      pick(r, ["name", "description", "model", "toolsets_pinned"]),
    );
  });
  add(
    "profile",
    "update",
    { description: str(4000, 0) },
    ["description"],
    true,
    async (c, i) => {
      checked(
        await c.rpc("profiles.configure", {
          name: c.p,
          description: i.description,
        }),
        "ok",
        "description",
      );
      return { ok: true };
    },
  );
  add("soul", "read", {}, [], false, async (c) =>
    resource(c.p, "soul", "soul", { content: (await describe(c)).soul }),
  );
  add(
    "soul",
    "update",
    { content: CONTENT, expectedContent: CONTENT },
    ["content", "expectedContent"],
    true,
    async (c, i) => {
      expected((await describe(c)).soul, i.expectedContent);
      checked(
        await c.rpc("profiles.configure", { name: c.p, soul: i.content }),
        "ok",
        "soul",
      );
      return {
        ok: true,
        resource: resource(c.p, "soul", "soul", { content: i.content }),
      };
    },
  );
  add("memory", "graph", {}, [], false, async (c) => {
    const graph = await c.rest("GET", "/api/learning/graph");
    return {
      agentId: c.p,
      resources: array(graph.nodes)
        .filter((n) => n.kind === "memory")
        .map((n) => {
          validate(n.id, MEMORY_ID, "memory ID");
          return resource(c.p, "memory", n.id, {
            title: n.label,
            source: n.memorySource,
            timestamp: n.timestamp,
          });
        }),
      edges: array(graph.edges).map((e) => pick(e, ["source", "target"])),
      clusters: array(graph.clusters).map((v) =>
        pick(v, ["category", "count"]),
      ),
    };
  });
  add("memory", "read", { id: MEMORY_ID }, ["id"], false, async (c, i) => {
    const r = await detail(c, i.id);
    return resource(c.p, "memory", i.id, {
      title: r.label,
      source: i.id.split(":")[1],
      content: r.content,
    });
  });
  for (const action of ["update", "delete"])
    add(
      "memory",
      action,
      {
        id: MEMORY_ID,
        expectedContent: CONTENT,
        ...(action === "update" ? { content: str(CONTENT_LIMIT) } : {}),
      },
      ["id", "expectedContent", ...(action === "update" ? ["content"] : [])],
      true,
      async (c, i) => {
        expected((await detail(c, i.id)).content, i.expectedContent);
        const r = checked(
          await c.rest(
            action === "update" ? "PUT" : "DELETE",
            "/api/learning/node",
            {
              id: i.id,
              profile: c.p,
              ...(action === "update" ? { content: i.content } : {}),
            },
          ),
        );
        return { ok: true, message: r.message, refreshRequired: true };
      },
    );
  add("skills", "list", {}, [], false, async (c) => ({
    resources: array(await c.rest("GET", "/api/skills")).map((r) =>
      resource(
        c.p,
        "skill",
        r.name,
        pick(r, [
          "name",
          "description",
          "category",
          "enabled",
          "usage",
          "provenance",
        ]),
      ),
    ),
  }));
  add("skills", "read", { id: ID }, ["id"], false, async (c, i) =>
    resource(c.p, "skill", i.id, {
      name: i.id,
      content: (await skillRead(c, i.id)).content,
    }),
  );
  add(
    "skills",
    "create",
    { id: ID, content: str(CONTENT_LIMIT), category: ID },
    ["id", "content"],
    true,
    async (c, i) => {
      checked(
        await c.rest("POST", "/api/skills", {
          name: i.id,
          content: i.content,
          profile: c.p,
          ...(i.category ? { category: i.category } : {}),
        }),
        "success",
      );
      return { ok: true };
    },
  );
  add(
    "skills",
    "update",
    editFields,
    ["id", "content", "expectedContent"],
    true,
    async (c, i) => {
      expected((await skillRead(c, i.id)).content, i.expectedContent);
      checked(
        await c.rest("PUT", "/api/skills/content", {
          name: i.id,
          content: i.content,
          profile: c.p,
        }),
        "success",
      );
      return { ok: true };
    },
  );
  add(
    "skills",
    "toggle",
    { id: ID, enabled: bool },
    ["id", "enabled"],
    true,
    async (c, i) => {
      checked(
        await c.rest("PUT", "/api/skills/toggle", {
          name: i.id,
          enabled: i.enabled,
          profile: c.p,
        }),
      );
      return { ok: true };
    },
  );
  add(
    "skills",
    "archive",
    { id: ID, expectedContent: CONTENT },
    ["id", "expectedContent"],
    true,
    async (c, i) => {
      expected((await skillRead(c, i.id)).content, i.expectedContent);
      const r = checked(
        await c.rest("DELETE", "/api/learning/node", {
          id: i.id,
          profile: c.p,
        }),
      );
      return { ok: true, message: r.message };
    },
  );
  add("tools", "list", {}, [], false, async (c) => ({
    resources: array((await describe(c)).toolsets).map((r) =>
      resource(c.p, "toolset", r.name, {
        ...pick(r, ["name", "label", "description", "enabled"]),
        toolCount: r.tool_count,
      }),
    ),
  }));
  add("tools", "details", {}, [], false, async (c) => {
    const r = await c.rpc("tools.show");
    return {
      agentId: c.p,
      total: r.total,
      sections: array(r.sections).map((s) => ({
        name: s.name,
        tools: array(s.tools).map((v) => pick(v, ["name", "description"])),
      })),
    };
  });
  add(
    "tools",
    "toggle",
    { id: ID, enabled: bool },
    ["id", "enabled"],
    true,
    async (c, i) => {
      const r = checked(
        await c.rpc("tools.configure", {
          action: i.enabled ? "enable" : "disable",
          names: [i.id],
        }),
        null,
      );
      if (
        !array(r.changed).includes(i.id) ||
        array(r.unknown).length ||
        array(r.missing_servers).length
      )
        fail("Hermes did not apply the toolset change.", "HERMES_REJECTED", r);
      return { ok: true, enabledToolsets: r.enabled_toolsets, reset: r.reset };
    },
  );
  add("models","reasoning",{},[],false,async c=>{
    const r=await c.rpc("config.get",{key:"reasoning"});return {value:r.value,display:r.display};
  });
  add("models","setReasoning",{value:str(20)},["value"],true,async(c,i)=>{
    if(!["none","minimal","low","medium","high","xhigh","max","ultra"].includes(i.value))fail("Invalid reasoning effort.");
    const r=await c.rpc("config.set",{key:"reasoning",value:i.value,scope:"global"});
    if(r.value!==i.value)fail("Hermes did not confirm the reasoning setting.");
    return {ok:true,value:r.value};
  });
  add("models", "current", {}, [], false, async (c) =>
    resource(
      c.p,
      "model",
      "main",
      pick(await c.rest("GET", "/api/model/info"), [
        "model",
        "provider",
        "capabilities",
        "effective_context_length",
      ]),
    ),
  );
  add(
    "models",
    "options",
    { refresh: bool, includeUnconfigured: bool },
    [],
    false,
    async (c, i) => {
      const r = await c.rest("GET", "/api/model/options", null, {
        explicit_only: 1,
        ...(i.refresh ? { refresh: 1 } : {}),
        ...(i.includeUnconfigured ? { include_unconfigured: 1 } : {}),
      });
      return {
        agentId: c.p,
        providers: array(r.providers).map((p) => ({
          name: p.name,
          models: p.models || [],
          baseUrl: p.api_url,
          ...pick(p, [
            "pricing",
            "capabilities",
            "unavailable_models",
            "free_tier",
          ]),
        })),
      };
    },
  );
  add(
    "models",
    "set",
    {
      provider: str(200),
      model: str(500),
      baseUrl: str(2000, 0),
      confirm: bool,
    },
    ["provider", "model"],
    true,
    async (c, i) => {
      if (i.baseUrl) {
        let url;
        try {
          url = new URL(i.baseUrl);
        } catch {
          fail("Invalid baseUrl.");
        }
        if (
          !["http:", "https:"].includes(url.protocol) ||
          url.username ||
          url.password
        )
          fail("Invalid baseUrl.");
      }
      const r = checked(
        await c.rest("POST", "/api/model/set", {
          scope: "main",
          provider: i.provider,
          model: i.model,
          ...(i.baseUrl !== undefined ? { base_url: i.baseUrl } : {}),
          confirm_expensive_model: i.confirm === true,
          profile: c.p,
        }),
      );
      return {
        ok: r.confirm_required ? false : true,
        confirmRequired: r.confirm_required === true,
        confirmMessage: r.confirm_message,
        ...pick(r, ["provider", "model"]),
        cronModelImpact: r.cron_model_impact,
      };
    },
  );
  add("schedules", "list", {}, [], false, async (c) => ({
    resources: array(
      await c.rest("GET", "/api/cron/jobs", null, { profile: c.p }),
    ).map((r) => scheduleResource(c.p, r)),
  }));
  const jobPath = (id) => `/api/cron/jobs/${encodeURIComponent(id)}`;
  add("schedules", "read", { id: ID }, ["id"], false, async (c, i) =>
    scheduleResource(c.p, await c.rest("GET", jobPath(i.id))),
  );
  add(
    "schedules",
    "create",
    scheduleFields,
    ["prompt", "schedule"],
    true,
    async (c, i) => ({
      ok: true,
      resource: scheduleResource(
        c.p,
        await c.rest(
          "POST",
          "/api/cron/jobs",
          pick(i, Object.keys(scheduleFields)),
        ),
      ),
    }),
  );
  add(
    "schedules",
    "update",
    {
      id: ID,
      updates: {
        type: "object",
        additionalProperties: false,
        minProperties: 1,
        properties: { ...scheduleFields, enabled: bool },
        required: [],
      },
    },
    ["id", "updates"],
    true,
    async (c, i) => ({
      ok: true,
      resource: scheduleResource(
        c.p,
        await c.rest("PUT", jobPath(i.id), { updates: i.updates }),
      ),
    }),
  );
  add("schedules", "delete", { id: ID }, ["id"], true, async (c, i) => {
    checked(await c.rest("DELETE", jobPath(i.id)));
    return { ok: true };
  });
  for (const action of ["pause", "resume"])
    add("schedules", action, { id: ID }, ["id"], true, async (c, i) => ({
      ok: true,
      resource: scheduleResource(
        c.p,
        await c.rest("POST", `${jobPath(i.id)}/${action}`),
      ),
    }));
  add("schedules", "trigger", { id: ID }, ["id"], true, async (c, i) => {
    const key = JSON.stringify([c.p, i.id]),
      existing = triggers.get(key);
    if (existing?.status === "running") return { status: "running" };
    const state = { status: "running", startedAt: Date.now() };
    triggers.set(key, state);
    void c
      .rest("POST", `${jobPath(i.id)}/trigger`, undefined, undefined, 600000)
      .then((result) => {
        state.resource = scheduleResource(c.p, result);
        state.status = result.last_error ? "failed" : "completed";
        state.error = result.last_error || null;
      })
      .catch((error) => {
        state.status = error.code === "REQUEST_TIMEOUT" ? "unknown" : "failed";
        state.error =
          state.status === "unknown"
            ? "The request timed out. Check run history before starting again."
            : error.message;
      });
    return { status: "running" };
  });
  add("schedules", "triggerStatus", { id: ID }, ["id"], false, async (c, i) =>
    structuredClone(
      triggers.get(JSON.stringify([c.p, i.id])) || { status: "none" },
    ),
  );
  add(
    "schedules",
    "runs",
    { id: ID, limit: { type: "integer", minimum: 1, maximum: 100 } },
    ["id"],
    false,
    async (c, i) => {
      const r = await c.rest("GET", `${jobPath(i.id)}/runs`, null, {
        limit: i.limit ?? 20,
      });
      return {
        agentId: c.p,
        runs: array(r.runs).map((v) =>
          pick(v, [
            "id",
            "title",
            "source",
            "model",
            "started_at",
            "last_active",
            "message_count",
            "ended_at",
            "is_active",
          ]),
        ),
      };
    },
  );
  for (const name of operations.keys())
    if (provider.operations.has(name))
      fail(`Operation already installed: ${name}`);
  for (const id of descriptors.keys())
    if (provider.descriptor.addons.some((d) => d.id === id))
      fail(`Addon already installed: ${id}`);
  for (const [name, fn] of operations) provider.operations.set(name, fn);
  provider.descriptor.addons.push(...descriptors.values());
  return provider;
}

module.exports = { installHermesAddons };
