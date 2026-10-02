const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const { appDb, readStore } = require("../electron/app-db.cjs");
const { ModelProviders } = require("../electron/model-providers.cjs");

/** A fake safeStorage that "encrypts" by reversing the string, so tests can
 * tell ciphertext apart from plaintext without touching the real Keychain. */
function fakeSafeStorage({ available = true } = {}) {
  return {
    isEncryptionAvailable: () => available,
    getSelectedStorageBackend: () => "keychain_access",
    encryptString: (value) =>
      Buffer.from([...value].reverse().join(""), "utf8"),
    decryptString: (buf) => [...buf.toString("utf8")].reverse().join(""),
  };
}

function fakeFetch(responses) {
  return async (url) => {
    const match = responses.find(([re]) => re.test(url));
    if (!match) throw new Error(`Unexpected fetch: ${url}`);
    const [, response] = match;
    return response;
  };
}

async function fixture(options = {}) {
  const userDataDir = await fs.mkdtemp("/tmp/sushiai-providers-");
  return {
    userDataDir,
    providers: new ModelProviders({
      userDataDir,
      safeStorage: options.safeStorage || fakeSafeStorage(),
      fetchImpl: options.fetchImpl || fakeFetch([]),
    }),
    cleanup: () => fs.rm(userDataDir, { recursive: true, force: true }),
  };
}

test("adds an OpenRouter preset provider with a fixed base URL", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    assert.equal(provider.baseUrl, "https://openrouter.ai/api");
    assert.equal(provider.label, "OpenRouter");
    const listed = await f.providers.listProviders();
    assert.equal(listed.length, 1);
    assert.equal(listed[0].hasKey, false);
  } finally {
    await f.cleanup();
  }
});

test("rejects a custom provider without an https base URL", async () => {
  const f = await fixture();
  try {
    await assert.rejects(
      f.providers.upsertProvider({
        kind: "custom",
        label: "Local",
        baseUrl: "http://insecure.example.com",
      }),
      /https/,
    );
  } finally {
    await f.cleanup();
  }
});

test("stores keys encrypted and never returns the raw key from listing", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    await f.providers.setProviderKey(provider.id, "sk-or-secret-value");
    const raw = JSON.stringify(readStore(f.userDataDir, "secrets"));
    assert.equal(raw.includes("sk-or-secret-value"), false);
    const listed = await f.providers.listProviders();
    assert.equal(listed[0].hasKey, true);
    assert.equal(listed[0].keyPlaintext, false);
    assert.equal(JSON.stringify(listed).includes("secret-value"), false);
  } finally {
    await f.cleanup();
  }
});

test("falls back to a marked plaintext key when encryption is unavailable", async () => {
  const f = await fixture({
    safeStorage: fakeSafeStorage({ available: false }),
  });
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    const result = await f.providers.setProviderKey(provider.id, "plain-key");
    assert.equal(result.keyPlaintext, true);
    const listed = await f.providers.listProviders();
    assert.equal(listed[0].keyPlaintext, true);
  } finally {
    await f.cleanup();
  }
});

test("stores Claude accounts encrypted and stages subscription tokens separately", async () => {
  const f = await fixture();
  try {
    const account = await f.providers.upsertClaudeAccount({
      label: "Work subscription",
      kind: "subscription",
    });
    await f.providers.setClaudeAccountValue(account.id, "invented-oauth-token");
    const listed = await f.providers.listClaudeAccounts();
    assert.equal(listed[0].hasValue, true);
    assert.equal(listed[0].hint, "inv…oken");
    assert.equal(
      JSON.stringify(listed).includes("invented-oauth-token"),
      false,
    );
    const staged = await f.providers.stageClaudeAccount(
      account.id,
      f.userDataDir,
    );
    assert.equal(staged.kind, "subscription");
    assert.equal(
      await fs.readFile(staged.tokenPath, "utf8"),
      "invented-oauth-token",
    );
    assert.equal((await fs.stat(staged.tokenPath)).mode & 0o777, 0o600);
    assert.equal(
      (await f.providers.resolveClaudeAccount(account.id)).value,
      "invented-oauth-token",
    );
  } finally {
    await f.cleanup();
  }
});

test("refuses Claude account values when secure storage is unavailable", async () => {
  const f = await fixture({
    safeStorage: fakeSafeStorage({ available: false }),
  });
  try {
    const account = await f.providers.upsertClaudeAccount({
      label: "Work subscription",
      kind: "subscription",
    });
    await assert.rejects(
      f.providers.setClaudeAccountValue(account.id, "invented-token"),
      /Secure storage is unavailable/,
    );
  } finally {
    await f.cleanup();
  }
});

test("stages a --settings file that feeds the key via apiKeyHelper, never ANTHROPIC_API_KEY (interactive Claude Code drops unapproved env keys)", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "opencode-go" });
    await f.providers.setProviderKey(provider.id, "zen-key");
    const profile = await f.providers.upsertProfile({
      providerId: provider.id,
      modelId: "some/model",
      label: "Go model",
    });
    const settingsPath = await f.providers.stageSettings(
      profile.id,
      f.userDataDir,
    );
    const staged = JSON.parse(await fs.readFile(settingsPath, "utf8"));
    const keyPath = settingsPath.replace(/\.json$/, ".key");
    assert.equal(staged.apiKeyHelper, `cat '${keyPath}'`);
    assert.equal(await fs.readFile(keyPath, "utf8"), "zen-key");
    assert.equal(JSON.stringify(staged).includes("zen-key"), false);
    const resolved = await f.providers.resolveEnv(profile.id);
    assert.deepEqual(staged.env, resolved.settings);
    assert.deepEqual(resolved.settings, {
      ANTHROPIC_BASE_URL: "https://opencode.ai/zen/go",
      ANTHROPIC_MODEL: "some/model",
      ANTHROPIC_DEFAULT_OPUS_MODEL: "some/model",
      ANTHROPIC_DEFAULT_SONNET_MODEL: "some/model",
      ANTHROPIC_DEFAULT_HAIKU_MODEL: "some/model",
    });
    assert.equal(resolved.model, "some/model");
  } finally {
    await f.cleanup();
  }
});

test("appends the [1m] model-id suffix for a Claude-family model at a million-token context window, instead of declaring it", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    await f.providers.setProviderKey(provider.id, "or-key");
    const profile = await f.providers.upsertProfile({
      providerId: provider.id,
      modelId: "~anthropic/claude-sonnet-latest",
      contextWindow: 1_000_000,
    });
    const resolved = await f.providers.resolveEnv(profile.id);
    assert.equal(resolved.model, "~anthropic/claude-sonnet-latest[1m]");
    assert.equal(
      resolved.settings.ANTHROPIC_MODEL,
      "~anthropic/claude-sonnet-latest[1m]",
    );
    assert.equal(resolved.settings.CLAUDE_CODE_MAX_CONTEXT_TOKENS, undefined);
  } finally {
    await f.cleanup();
  }
});

test("declares CLAUDE_CODE_MAX_CONTEXT_TOKENS for a non-Claude model's context window instead of suffixing the id", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "opencode-go" });
    await f.providers.setProviderKey(provider.id, "zen-key");
    const profile = await f.providers.upsertProfile({
      providerId: provider.id,
      modelId: "deepseek/deepseek-v4-pro",
      contextWindow: 128000,
    });
    const resolved = await f.providers.resolveEnv(profile.id);
    assert.equal(resolved.model, "deepseek/deepseek-v4-pro");
    assert.equal(resolved.settings.CLAUDE_CODE_MAX_CONTEXT_TOKENS, "128000");
  } finally {
    await f.cleanup();
  }
});

test("resolveEnv fails clearly when no key was saved yet", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    const profile = await f.providers.upsertProfile({
      providerId: provider.id,
      modelId: "openai/gpt-5.1",
    });
    await assert.rejects(
      f.providers.resolveEnv(profile.id),
      /No API key saved/,
    );
  } finally {
    await f.cleanup();
  }
});

test("deleting a provider cascades to its model profiles", async () => {
  const f = await fixture();
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    const profile = await f.providers.upsertProfile({
      providerId: provider.id,
      modelId: "openai/gpt-5.1",
    });
    await f.providers.deleteProvider(provider.id);
    const profiles = await f.providers.listProfiles();
    assert.equal(
      profiles.find((p) => p.id === profile.id),
      undefined,
    );
  } finally {
    await f.cleanup();
  }
});

test("testConnection reports distinct, provider-labeled codes for no-key, unauthorized and ok", async () => {
  const f = await fixture({
    fetchImpl: fakeFetch([
      [/openrouter\.ai\/api\/v1\/models/, { ok: false, status: 401 }],
    ]),
  });
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    const noKey = await f.providers.testConnection(provider.id);
    assert.equal(noKey.code, "no_key");
    assert.match(noKey.message, /OpenRouter/);

    await f.providers.setProviderKey(provider.id, "bad-key");
    const unauthorized = await f.providers.testConnection(provider.id);
    assert.equal(unauthorized.code, "unauthorized");
    assert.match(unauthorized.message, /OpenRouter/);
  } finally {
    await f.cleanup();
  }
});

test("fetchModels filters out :batch model IDs from OpenCode Go's live catalog", async () => {
  const f = await fixture({
    fetchImpl: fakeFetch([
      [
        /opencode\.ai\/zen\/go\/v1\/models/,
        {
          ok: true,
          json: async () => ({
            data: [
              {
                id: "deepseek/deepseek-v4-pro",
                name: "DeepSeek V4 Pro",
                context_length: 400000,
              },
              {
                id: "deepseek/deepseek-v4-pro:batch",
                name: "DeepSeek V4 Pro batch",
              },
            ],
          }),
        },
      ],
    ]),
  });
  try {
    const provider = await f.providers.upsertProvider({ kind: "opencode-go" });
    const models = await f.providers.fetchModels(provider.id);
    assert.deepEqual(models, [
      {
        id: "deepseek/deepseek-v4-pro",
        label: "DeepSeek V4 Pro",
        context: 400000,
      },
    ]);
  } finally {
    await f.cleanup();
  }
});

test("fetchModels returns OpenRouter's fixed Claude-alias list without hitting the network", async () => {
  const f = await fixture({
    fetchImpl: async () => {
      throw new Error("must not call the network for OpenRouter's model list");
    },
  });
  try {
    const provider = await f.providers.upsertProvider({ kind: "openrouter" });
    const models = await f.providers.fetchModels(provider.id);
    assert.ok(models.length > 0);
    assert.ok(models.every((m) => m.id.startsWith("~anthropic/claude-")));
  } finally {
    await f.cleanup();
  }
});

test("Claude account resolver tags only legitimate absence and preserves configuration errors", async () => {
  const f = await fixture();
  try {
    await assert.rejects(f.providers.resolveClaudeAccount("missing"), {
      code: "ACCOUNT_NOT_CONFIGURED",
    });
    const account = await f.providers.upsertClaudeAccount({
      kind: "apiKey",
      label: "Work",
    });
    await assert.rejects(f.providers.resolveClaudeAccount(account.id), {
      code: "ACCOUNT_NOT_CONFIGURED",
    });
    // An unreadable stored row is skipped, so the account has no value.
    appDb(f.userDataDir)
      .prepare("INSERT INTO store(name, key, value) VALUES(?, ?, ?)")
      .run("secrets", `claude-account:${account.id}`, "{broken");
    await assert.rejects(f.providers.resolveClaudeAccount(account.id), {
      code: "ACCOUNT_NOT_CONFIGURED",
    });
    await f.providers.setClaudeAccountValue(account.id, "synthetic-key");
    f.providers.safeStorage.decryptString = () => {
      throw new Error("Decrypt failed");
    };
    await assert.rejects(
      f.providers.resolveClaudeAccount(account.id),
      /Decrypt failed/,
    );
  } finally {
    await f.cleanup();
  }
});
