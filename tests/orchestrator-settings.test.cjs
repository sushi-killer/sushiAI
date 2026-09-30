const test = require("node:test");
const assert = require("node:assert/strict");

const load = () => import("../src/orchestrator/autonomy.ts");

const base = () => ({
  routes: [],
  tiers: { mechanical: "a", standard: "b", hard: "c" },
  autoAnswer: false,
  answerPolicy: true,
  experiments: { stallTimeoutSecs: 0 },
});

test("presetOf recognises Ask me and Balanced, anything else is custom", async () => {
  const { presetOf, applyPreset } = await load();
  const ask = applyPreset(base(), "ask");
  assert.equal(presetOf(ask), "ask");
  assert.equal(ask.answerPolicy, false);
  assert.equal(ask.experiments.land, false);
  const balanced = applyPreset(base(), "balanced");
  assert.equal(presetOf(balanced), "balanced");
  assert.equal(balanced.autoAnswer, false);
  assert.equal(balanced.answerPolicy, true);
  assert.equal(balanced.experiments.land, true);
  assert.equal(presetOf(base()), "custom");
  assert.equal(presetOf({ ...balanced, autoAnswer: true }), "custom");
});

test("orchd's default settings read as Balanced, not Custom", async () => {
  const { presetOf } = await load();
  // orchd Settings::default(): autoAnswer off, answerPolicy on, and
  // default_experiments() turns land on.
  const defaults = {
    ...base(),
    autoAnswer: false,
    answerPolicy: true,
    experiments: { stallTimeoutSecs: 0, loopDetect: true, land: true },
  };
  assert.equal(presetOf(defaults), "balanced");
});

test("applyPreset keeps other experiment flags and does not mutate", async () => {
  const { applyPreset } = await load();
  const s = base();
  s.experiments.stallTimeoutSecs = 900;
  const next = applyPreset(s, "balanced");
  assert.equal(next.experiments.stallTimeoutSecs, 900);
  assert.equal(s.autoAnswer, false);
  assert.equal(s.experiments.land, undefined);
});

test("countChanges counts each differing field and experiment key once", async () => {
  const { countChanges, applyPreset } = await load();
  const s = base();
  assert.equal(countChanges(s, s), 0);
  assert.equal(countChanges(s, applyPreset(s, "ask")), 2);
  assert.equal(countChanges(s, { ...s, tiers: { ...s.tiers, hard: "z" } }), 1);
  assert.equal(countChanges(s, { ...s, protectedPaths: ["x"] }), 1);
});
