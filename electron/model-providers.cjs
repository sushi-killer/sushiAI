// Model providers let Claude Code run against an Anthropic-compatible
// endpoint other than api.anthropic.com — OpenRouter's "Anthropic Skin" and
// OpenCode Go both speak that protocol, so no proxy is needed: the CLI just
// needs ANTHROPIC_BASE_URL/ANTHROPIC_MODEL plus the key. Those must travel
// to Claude Code as its own `--settings <file>`, not as plain process env: a
// cached subscription login otherwise wins silently, which is what a
// plain-env attempt at this looked like from the outside (it just launches
// Sonnet on the real Anthropic API, no error).
//
// Two files in userData, kept apart on purpose: `providers.json` holds
// non-secret metadata (readable, gitignored by virtue of living outside the
// repo), `secrets.json` holds only encrypted key material. The key itself
// never crosses back out of this module — callers get `hasKey`/`keyHint`.
const { randomUUID } = require("node:crypto");
const fs = require("node:fs/promises");
const path = require("node:path");

const PRESETS = {
  // OpenRouter's Anthropic-compatible endpoint is a "skin" over Claude
  // itself, not a gateway to its wider catalog: it only understands these
  // ~anthropic/claude-*-latest names. Anything else (an OpenRouter catalog
  // id like "openai/gpt-5.1") is not recognized and the request silently
  // falls back to plain Sonnet — so this list is fixed, not live-fetched.
  openrouter: {
    label: "OpenRouter",
    baseUrl: "https://openrouter.ai/api",
    staticModels: [
      { id: "~anthropic/claude-fable-latest", label: "Fable" },
      { id: "~anthropic/claude-fable-latest[1m]", label: "Fable (1M context)" },
      { id: "~anthropic/claude-opus-latest", label: "Opus" },
      { id: "~anthropic/claude-opus-latest[1m]", label: "Opus (1M context)" },
      { id: "~anthropic/claude-sonnet-latest", label: "Sonnet" },
      { id: "~anthropic/claude-sonnet-latest[1m]", label: "Sonnet (1M context)" },
      { id: "~anthropic/claude-haiku-latest", label: "Haiku" },
    ],
  },
  // OpenCode Go's endpoint properly translates arbitrary open models, so its
  // live catalog is the real, useful list.
  "opencode-go": {
    label: "OpenCode Go",
    baseUrl: "https://opencode.ai/zen/go",
    modelsUrl: "https://opencode.ai/zen/go/v1/models",
  },
};
const MODEL_RE = /^[\w./:~[\]-]{1,128}$/;
const CONTEXT_WINDOWS = [128000, 200000, 256000, 512000, 1000000];
const MILLION = 1_000_000;
const isClaudeFamily = (modelId) => modelId.toLowerCase().includes("claude");

function keyHint(key) {
  const tail = key.slice(-4);
  return key.length > 8 ? `${key.slice(0, 3)}…${tail}` : "…" + tail;
}

class ModelProviders {
  constructor({ userDataDir, safeStorage, fetchImpl = fetch }) {
    this.providersFile = path.join(userDataDir, "providers.json");
    this.secretsFile = path.join(userDataDir, "secrets.json");
    this.safeStorage = safeStorage;
    this.fetchImpl = fetchImpl;
  }

  async #readJson(file) {
    try {
      return JSON.parse(await fs.readFile(file, "utf8"));
    } catch {
      return {};
    }
  }
  async #writeJson(file, value) {
    await fs.mkdir(path.dirname(file), { recursive: true });
    await fs.writeFile(file, JSON.stringify(value, null, 2), { mode: 0o600 });
  }

  #encryptedBackend() {
    if (!this.safeStorage?.isEncryptionAvailable?.()) return null;
    // basic_text on Linux obfuscates with a hardcoded password — not real
    // encryption, so it's treated the same as "unavailable".
    const backend = this.safeStorage.getSelectedStorageBackend?.();
    if (backend === "basic_text") return null;
    return "safeStorage";
  }

  async listProviders() {
    const providers = await this.#readJson(this.providersFile);
    const secrets = await this.#readJson(this.secretsFile);
    return Object.values(providers).map((p) => {
      const secret = secrets[p.id];
      return {
        ...p,
        hasKey: !!secret,
        keyPlaintext: secret?.backend === "plain",
      };
    });
  }

  async upsertProvider({ id, kind, label, baseUrl }) {
    if (!PRESETS[kind] && kind !== "custom")
      throw new Error("Unknown provider kind.");
    const providers = await this.#readJson(this.providersFile);
    const preset = PRESETS[kind];
    const providerId = id && providers[id] ? id : randomUUID();
    const resolvedBaseUrl = preset ? preset.baseUrl : String(baseUrl || "");
    if (!/^https:\/\//.test(resolvedBaseUrl))
      throw new Error("Base URL must start with https://");
    providers[providerId] = {
      id: providerId,
      kind,
      label: String(label || preset?.label || "Custom provider").slice(
        0,
        80,
      ),
      baseUrl: resolvedBaseUrl,
    };
    await this.#writeJson(this.providersFile, providers);
    return providers[providerId];
  }

  async deleteProvider(id) {
    const providers = await this.#readJson(this.providersFile);
    delete providers[id];
    await this.#writeJson(this.providersFile, providers);
    const secrets = await this.#readJson(this.secretsFile);
    delete secrets[id];
    await this.#writeJson(this.secretsFile, secrets);
    const profiles = await this.#readJson(this.profilesFileFor());
    for (const [profileId, profile] of Object.entries(profiles))
      if (profile.providerId === id) delete profiles[profileId];
    await this.#writeJson(this.profilesFileFor(), profiles);
  }

  async setProviderKey(id, key) {
    if (typeof key !== "string" || !key.trim())
      throw new Error("Enter an API key.");
    const providers = await this.#readJson(this.providersFile);
    if (!providers[id]) throw new Error("Unknown provider.");
    const secrets = await this.#readJson(this.secretsFile);
    const backend = this.#encryptedBackend();
    const trimmed = key.trim();
    secrets[id] = backend
      ? { v: 1, backend, ct: this.safeStorage.encryptString(trimmed).toString("base64") }
      : { v: 1, backend: "plain", ct: Buffer.from(trimmed, "utf8").toString("base64") };
    await this.#writeJson(this.secretsFile, secrets);
    return { hasKey: true, keyPlaintext: !backend, keyHint: keyHint(trimmed) };
  }

  async clearProviderKey(id) {
    const secrets = await this.#readJson(this.secretsFile);
    delete secrets[id];
    await this.#writeJson(this.secretsFile, secrets);
  }

  async #keyFor(id) {
    const secrets = await this.#readJson(this.secretsFile);
    const secret = secrets[id];
    if (!secret) return null;
    const buf = Buffer.from(secret.ct, "base64");
    if (secret.backend === "plain") return buf.toString("utf8");
    try {
      return this.safeStorage.decryptString(buf);
    } catch {
      // Blob was encrypted under a different signing identity/machine;
      // treat like no key rather than crash the spawn.
      return null;
    }
  }

  profilesFileFor() {
    return path.join(path.dirname(this.providersFile), "model-profiles.json");
  }

  async listProfiles() {
    const profiles = await this.#readJson(this.profilesFileFor());
    return Object.values(profiles);
  }

  async upsertProfile({ id, providerId, modelId, label, effort, contextWindow }) {
    const providers = await this.#readJson(this.providersFile);
    if (!providers[providerId]) throw new Error("Unknown provider.");
    if (!MODEL_RE.test(String(modelId || "")))
      throw new Error("Invalid model ID.");
    const profiles = await this.#readJson(this.profilesFileFor());
    const profileId = id && profiles[id] ? id : randomUUID();
    profiles[profileId] = {
      id: profileId,
      providerId,
      modelId,
      label: String(label || modelId).slice(0, 80),
      effort: effort || undefined,
      contextWindow: Number(contextWindow) > 0 ? Number(contextWindow) : undefined,
    };
    await this.#writeJson(this.profilesFileFor(), profiles);
    return profiles[profileId];
  }

  async deleteProfile(id) {
    const profiles = await this.#readJson(this.profilesFileFor());
    delete profiles[id];
    await this.#writeJson(this.profilesFileFor(), profiles);
  }

  /**
   * The settings document for `claude --settings <file>`: the one injection
   * point that actually wins over a cached subscription login (plain process
   * env does not). Caller writes the returned `settings` object to a temp
   * JSON file as `{"env": settings}`.
   */
  async resolveEnv(profileId) {
    const profiles = await this.#readJson(this.profilesFileFor());
    const profile = profiles[profileId];
    if (!profile) throw new Error("Model profile not found.");
    const providers = await this.#readJson(this.providersFile);
    const provider = providers[profile.providerId];
    if (!provider) throw new Error("Provider for this model profile is gone.");
    const key = await this.#keyFor(provider.id);
    if (!key)
      throw new Error(
        `No API key saved for ${provider.label}. Add one in Preferences → Providers.`,
      );
    const claudeFamily = isClaudeFamily(profile.modelId);
    // The 1M window is a model-id suffix for a Claude-family model (Claude
    // Code strips it before sending and adds the matching anthropic-beta);
    // for anything else the window is only settable by declaring it, since
    // Claude Code otherwise assumes 200K for a model it doesn't recognize.
    const wantsMillion = profile.contextWindow >= MILLION;
    const launchModelId =
      claudeFamily && wantsMillion && !profile.modelId.endsWith("[1m]")
        ? `${profile.modelId}[1m]`
        : profile.modelId;
    // No ANTHROPIC_API_KEY/AUTH_TOKEN here: the key goes through
    // apiKeyHelper (see stageSettings).
    const settings = {
      ANTHROPIC_BASE_URL: provider.baseUrl,
      ANTHROPIC_MODEL: launchModelId,
      // Pinned on every alias, not just the resolved one: background and
      // subagent turns go to the haiku alias regardless of the main model.
      ANTHROPIC_DEFAULT_OPUS_MODEL: launchModelId,
      ANTHROPIC_DEFAULT_SONNET_MODEL: launchModelId,
      ANTHROPIC_DEFAULT_HAIKU_MODEL: launchModelId,
    };
    if (profile.contextWindow && !claudeFamily)
      settings.CLAUDE_CODE_MAX_CONTEXT_TOKENS = String(profile.contextWindow);
    return { settings, key, model: launchModelId, label: profile.label };
  }

  /**
   * Writes `<dir>/sushiai-model-<uuid>.json` for `claude --settings` and the
   * `.key` file next to it. The key is fed through apiKeyHelper, not
   * ANTHROPIC_API_KEY: interactive Claude Code only uses an env API key the
   * user approved once ("Detected a custom API key…"), and a key that got
   * rejected there, or never answered, is silently dropped. The provider then
   * returns "401 Missing API key" forever, while `claude -p` works because it
   * skips that check. apiKeyHelper has no approval step and sends the key as
   * both x-api-key and Bearer, which covers OpenCode Go and OpenRouter.
   */
  async stageSettings(profileId, dir) {
    const { settings, key } = await this.resolveEnv(profileId);
    const base = path.join(dir, `sushiai-model-${randomUUID()}`);
    await fs.writeFile(`${base}.key`, key, { mode: 0o600 });
    // ponytail: assumes `dir` has no single quote (os.tmpdir() doesn't).
    const document = { apiKeyHelper: `cat '${base}.key'`, env: settings };
    await fs.writeFile(`${base}.json`, JSON.stringify(document), { mode: 0o600 });
    return `${base}.json`;
  }

  async testConnection(id) {
    const providers = await this.#readJson(this.providersFile);
    const provider = providers[id];
    if (!provider) return { ok: false, code: "unknown", message: "Unknown provider." };
    const key = await this.#keyFor(id);
    if (!key)
      return {
        ok: false,
        code: "no_key",
        message: `Add an API key for ${provider.label} first.`,
      };
    const url = `${provider.baseUrl.replace(/\/$/, "")}/v1/models`;
    try {
      const response = await this.fetchImpl(url, {
        headers: { Authorization: `Bearer ${key}` },
      });
      if (response.status === 401 || response.status === 403)
        return {
          ok: false,
          code: "unauthorized",
          message: `${provider.label} rejected this key.`,
        };
      if (!response.ok)
        return {
          ok: false,
          code: "network",
          message: `${provider.label} returned an error (${response.status}).`,
        };
      return { ok: true, code: "ok", message: `Connected to ${provider.label}.` };
    } catch {
      return {
        ok: false,
        code: "network",
        message: `Could not reach ${provider.label}.`,
      };
    }
  }

  /** Live model catalog for a provider. Never throws: an empty list just
   * falls back to a manual model-ID field in the UI. */
  async fetchModels(id) {
    const providers = await this.#readJson(this.providersFile);
    const provider = providers[id];
    const preset = provider && PRESETS[provider.kind];
    if (!preset) return [];
    if (preset.staticModels) return preset.staticModels;
    if (!preset.modelsUrl) return [];
    try {
      const key = await this.#keyFor(id);
      const response = await this.fetchImpl(preset.modelsUrl, {
        headers: key ? { Authorization: `Bearer ${key}` } : {},
      });
      if (!response.ok) return [];
      const body = await response.json();
      const list = Array.isArray(body?.data) ? body.data : [];
      return list
        .map((m) => ({
          id: String(m.id || ""),
          label: String(m.name || m.id || ""),
          context: Number(m.context_length || m.context_window) || undefined,
        }))
        .filter((m) => m.id && !m.id.endsWith(":batch"));
    } catch {
      return [];
    }
  }
}

module.exports = { ModelProviders, PRESETS, CONTEXT_WINDOWS, MODEL_RE };
