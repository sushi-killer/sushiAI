const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/orchestrator/settingsJson.ts");

const base = {
  routes: [{ id: "codex", label: "Codex", harness: "codex" }],
  tiers: { mechanical: "codex", standard: "codex", hard: "codex" },
  experiments: { loopDetect: true },
};

test("formatSettingsJson and parseSettingsJson round-trip every key, known or not", async () => {
  const { formatSettingsJson, parseSettingsJson } = await library;
  const settings = {
    ...base,
    briefCheckRoute: "codex",
    afterLand: [{ repo: "/r", run: "make" }],
    aFutureKey: { nested: [1, 2, 3] },
  };
  const text = formatSettingsJson(settings);
  assert.match(text, /\n {2}"routes"/);
  const parsed = parseSettingsJson(text);
  assert.equal(parsed.ok, true);
  assert.deepEqual(parsed.settings, settings);
});

test("parseSettingsJson reports the parse error instead of throwing", async () => {
  const { parseSettingsJson } = await library;
  const parsed = parseSettingsJson('{"routes": [');
  assert.equal(parsed.ok, false);
  assert.ok(parsed.error.length > 0);
});

test("parseSettingsJson rejects what the panel could not draw", async () => {
  const { parseSettingsJson } = await library;
  for (const text of ["[]", "null", "3", '"x"'])
    assert.deepEqual(parseSettingsJson(text), {
      ok: false,
      error: "Settings must be a JSON object.",
    });
  assert.deepEqual(parseSettingsJson(JSON.stringify({ ...base, routes: {} })), {
    ok: false,
    error: '"routes" must be a list.',
  });
  assert.deepEqual(parseSettingsJson(JSON.stringify({ ...base, tiers: [] })), {
    ok: false,
    error: '"tiers" must be an object.',
  });
  const { experiments, ...rest } = base;
  assert.equal(experiments.loopDetect, true);
  assert.deepEqual(parseSettingsJson(JSON.stringify(rest)), {
    ok: false,
    error: '"experiments" must be an object.',
  });
});
