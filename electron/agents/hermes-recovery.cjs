const { object } = require("./registry.cjs");
function recoveryPolicy(config) {
  const policy = config?.desktop?.auto_continue || {};
  const value = policy.enabled;
  const enabled =
    value == null
      ? true
      : typeof value === "string"
        ? ["1", "true", "yes", "on"].includes(value.trim().toLowerCase())
        : typeof value === "object"
          ? Object.keys(value).length > 0
          : Boolean(value);
  return { enabled };
}
function installHermesRecovery(provider) {
  provider.descriptor.addons.push({
    id: "recovery",
    name: "Recovery",
    description: "Automatic continuation after an interrupted run",
  });
  const pending = new Map();
  for (const action of ["read", "update"])
    provider.operations.set(`addons.recovery.${action}`, async (input) => {
      object(input);
      if (
        Object.keys(input).some(
          (k) =>
            !["agentId", ...(action === "update" ? ["enabled"] : [])].includes(
              k,
            ),
        )
      )
        throw Error("Unknown recovery setting.");
      if (action === "update" && typeof input.enabled !== "boolean")
        throw Error("Recovery enabled must be a boolean.");
      const agent = await provider.agent(input);
      const request = (method, body) =>
        provider.transport(agent).request(method, "/api/config", {
          profile: agent,
          ...(body ? { body: { profile: agent, ...body } } : {}),
        });
      const read = async () => recoveryPolicy(await request("GET"));
      if (action === "read") return read();
      const work = (pending.get(agent) || Promise.resolve())
        .catch(() => {})
        .then(async () => {
          const result = await request("PUT", {
            config: { desktop: { auto_continue: { enabled: input.enabled } } },
          });
          if (result.ok !== true)
            throw Error("Hermes did not confirm recovery settings.");
          const saved = await read();
          if (saved.enabled !== input.enabled)
            throw Error(
              "Recovery settings changed while saving. Refresh and try again.",
            );
          return saved;
        });
      pending.set(agent, work);
      try {
        return await work;
      } finally {
        if (pending.get(agent) === work) pending.delete(agent);
      }
    });
}
module.exports = { installHermesRecovery, recoveryPolicy };
