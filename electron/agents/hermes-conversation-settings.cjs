const { object, text } = require("./registry.cjs");
const efforts = [
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
  "ultra",
];
function installConversationSettings(provider) {
  provider.descriptor.capabilities.push("conversation-settings");
  const pending = new Set();
  for (const action of ["read", "reasoning", "model", "options"])
    provider.operations.set(
      `conversations.settings.${action}`,
      async (input) => {
        object(input);
        const fields = [
          "agentId",
          "conversationId",
          ...(action === "reasoning"
            ? ["value"]
            : action === "model"
              ? ["model", "provider", "confirm"]
              : []),
        ];
        if (Object.keys(input).some((k) => !fields.includes(k)))
          throw Error("Unknown conversation setting.");
        const s = provider.get(input),
          transport = provider.transport(s.agentId),
          runtime = s.runtime;
        if (action === "options") {
          const result = await transport.request("GET", "/api/model/options", {
            profile: s.agentId,
            query: { explicit_only: 1, include_unconfigured: 0 },
          });
          if (!Array.isArray(result.providers))
            throw Error("Hermes did not return model suggestions.");
          return {
            defaultProvider:
              typeof result.provider === "string" ? result.provider : "",
            providers: result.providers
              .filter((p) => typeof p.name === "string")
              .map((p) => ({
                name: p.name,
                models: [
                  ...new Set(
                    (Array.isArray(p.models) ? p.models : [])
                      .map((m) => (typeof m === "string" ? m : m?.id))
                      .filter(
                        (id) => typeof id === "string" && id.length <= 300,
                      ),
                  ),
                ],
              })),
          };
        }
        const rpc = (method, params) =>
          transport.rpc(s.agentId, method, { ...params, session_id: runtime });
        if (pending.has(s))
          throw Error("Wait for the current settings change.");
        if (
          action !== "read" &&
          ["sending", "running", "waiting"].includes(s.state.status)
        )
          throw Error("Stop or finish this turn before changing its settings.");
        // Native config setters fall back to defaults when a runtime is missing.
        // Validate the owned binding first; never use a durable ID as a runtime ID.
        pending.add(s);
        s.settingsPending = true;
        try {
          await rpc("session.status", {});
          if (s.runtime !== runtime)
            throw Error("The conversation reconnected. Reopen its settings.");
          if (action === "read") {
            const r = await rpc("config.get", { key: "reasoning" });
            return {
              reasoning: r.value,
              model: s.state.info.model || "",
              provider: s.state.info.provider || "",
            };
          }
          let params;
          if (action === "reasoning") {
            if (!efforts.includes(input.value))
              throw Error("Invalid reasoning effort.");
            params = { key: "reasoning", value: input.value, scope: "session" };
          } else {
            const model = text(input.model, "model", 300),
              engine = input.provider
                ? text(input.provider, "model provider", 100)
                : "";
            if (
              !/^[a-zA-Z0-9][a-zA-Z0-9_.:/@+-]*$/.test(model) ||
              (engine && !/^[a-zA-Z0-9][a-zA-Z0-9_.:-]*$/.test(engine))
            )
              throw Error(
                "Use a model ID and provider name without command flags.",
              );
            if (
              input.confirm !== undefined &&
              typeof input.confirm !== "boolean"
            )
              throw Error("Invalid model confirmation.");
            params = {
              key: "model",
              value: `${model}${engine ? ` --provider ${engine}` : ""} --session`,
              confirm_expensive_model: input.confirm === true,
            };
          }
          const previousReasoning =
            action === "model"
              ? (await rpc("config.get", { key: "reasoning" })).value
              : null;
          const r = await rpc("config.set", params);
          if (r.confirm_required)
            return {
              confirmRequired: true,
              message:
                r.confirm_message || r.warning || "Confirm this model change.",
            };
          if (action === "reasoning" && r.value !== input.value)
            throw Error("Hermes did not confirm the reasoning change.");
          let reasoningWarning = "";
          if (action === "model" && efforts.includes(previousReasoning)) {
            try {
              const kept = await rpc("config.set", {
                key: "reasoning",
                value: previousReasoning,
                scope: "session",
              });
              if (kept.value !== previousReasoning)
                throw Error("Unconfirmed reasoning setting");
            } catch {
              reasoningWarning =
                "The model changed, but the previous reasoning setting could not be restored. Save your reasoning preference again.";
            }
          }
          return {
            ok: true,
            value: r.value,
            warning:
              [
                r.warning ? String(r.warning).slice(0, 2000) : "",
                reasoningWarning,
              ]
                .filter(Boolean)
                .join(" ") || null,
          };
        } finally {
          pending.delete(s);
          s.settingsPending = false;
        }
      },
    );
}
module.exports = { installConversationSettings };
