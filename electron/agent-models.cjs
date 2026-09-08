// Models offered in the chat composer. Codex keeps its live list in the CLI's
// own cache, refreshed by the CLI itself; Claude has no listing command, so its
// aliases are static and each alias always resolves to that model's latest build.
const { readFileSync } = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const CLAUDE_EFFORTS = ["low", "medium", "high", "xhigh", "max"];
const CODEX_EFFORTS = ["low", "medium", "high", "xhigh", "max", "ultra"];
const CLAUDE_MODELS = [
  { id: "fable", label: "Fable", description: "Most capable for hard work." },
  { id: "opus", label: "Opus", description: "Deep reasoning and long tasks." },
  { id: "sonnet", label: "Sonnet", description: "Balanced everyday coding." },
  { id: "haiku", label: "Haiku", description: "Fastest, for light work." },
].map((m) => ({
  ...m,
  efforts: CLAUDE_EFFORTS,
  defaultEffort: "",
  context: 200000,
}));

/** Codex's cache, shaped for the composer. Hidden and unpriced models stay out. */
function parseCodexCache(raw) {
  return (raw?.models || [])
    .filter((m) => m?.visibility === "list" && typeof m.slug === "string")
    .sort((a, b) => (a.priority ?? 999) - (b.priority ?? 999))
    .map((m) => ({
      id: m.slug,
      label: m.display_name || m.slug,
      description: m.description || "",
      efforts: (m.supported_reasoning_levels || [])
        .map((e) => e?.effort)
        .filter((e) => CODEX_EFFORTS.includes(e)),
      defaultEffort: CODEX_EFFORTS.includes(m.default_reasoning_level)
        ? m.default_reasoning_level
        : "",
      context: Number(m.context_window) || 0,
    }));
}

/** Top-level `key = "value"` in config.toml, ignoring everything under a [section]. */
function parseTomlRoot(text, key) {
  for (const line of String(text).split("\n")) {
    const row = line.trim();
    if (row.startsWith("[")) break;
    const match = row.match(new RegExp(`^${key}\\s*=\\s*"([^"]*)"`));
    if (match) return match[1];
  }
  return "";
}

function read(file) {
  try {
    return readFileSync(file, "utf8");
  } catch {
    return "";
  }
}

/** Never throws: a missing or broken cache degrades to the CLI's own default. */
function chatModels(home = os.homedir()) {
  let codex = [];
  try {
    codex = parseCodexCache(
      JSON.parse(read(path.join(home, ".codex/models_cache.json"))),
    );
  } catch {
    codex = [];
  }
  const config = read(path.join(home, ".codex/config.toml"));
  const configured = parseTomlRoot(config, "model");
  const fallback = codex.find((m) => m.id === configured);
  return {
    claude: {
      models: CLAUDE_MODELS,
      efforts: CLAUDE_EFFORTS,
      defaultModel: "",
      // Claude reports its real window per run; this is only the opening guess.
      defaultContext: 200000,
    },
    codex: {
      models: codex,
      efforts: CODEX_EFFORTS,
      defaultModel: fallback?.label || configured || "",
      defaultModelId: configured,
      defaultContext: fallback?.context || 0,
      defaultEffort: parseTomlRoot(config, "model_reasoning_effort"),
    },
  };
}

module.exports = {
  chatModels,
  parseCodexCache,
  parseTomlRoot,
  CLAUDE_EFFORTS,
  CODEX_EFFORTS,
};
