const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const model = import("../src/orchestrator/blueprintModel.ts");
const json = import("../src/orchestrator/settingsJson.ts");

const routes = [
  { id: "claude-sonnet", label: "Sonnet", harness: "claude", model: "sonnet" },
  { id: "codex", label: "Codex", harness: "codex" },
  {
    id: "codex-luna",
    label: "Codex Luna",
    harness: "codex",
    model: "gpt-5.6-luna",
  },
];

function stage(id, overrides = {}) {
  return {
    id,
    label: id[0].toUpperCase() + id.slice(1),
    kind: "main",
    enabled: true,
    optional: false,
    ownerOnly: false,
    ...overrides,
  };
}

function payload(overrides = {}) {
  return {
    presetId: null,
    presets: [],
    stages: [
      stage("brief"),
      stage("plan", { routeId: "claude-sonnet", model: "sonnet" }),
      stage("brief-check", { optional: true, routeId: "codex" }),
      stage("base-checks", { ownerOnly: true }),
      stage("implement", {
        routeId: "claude-sonnet",
        maxAttempts: 3,
        state: "failed",
        durationMs: 125_000,
        costUsd: 0.42,
      }),
      stage("checks", { ownerOnly: true, state: "running" }),
      stage("held-out", { optional: true }),
      stage("plan-review"),
      stage("review", { routeId: "codex" }),
      stage("judge", { routeId: "codex", routeLabel: "Codex" }),
      stage("land", { optional: true, enabled: false }),
      stage("advisor", {
        kind: "side",
        optional: true,
        routeId: "claude-sonnet",
      }),
    ],
    ...overrides,
  };
}

function settings(overrides = {}) {
  return {
    routes,
    chatTools: [],
    tiers: {
      mechanical: "codex",
      standard: "claude-sonnet",
      hard: "claude-sonnet",
    },
    review: "codex",
    planner: "claude-sonnet",
    maxAttempts: 3,
    experiments: { advisor: false, heldOut: false, land: false },
    briefCheckRoute: "codex",
    ...overrides,
  };
}

test("stages map to nodes in payload order, unknown ids included", async () => {
  const { blueprintNodes } = await model;
  const nodes = blueprintNodes(payload(), routes);
  assert.deepEqual(
    nodes.map((n) => n.id),
    payload().stages.map((s) => s.id),
  );
  const unknown = nodes.find((n) => n.id === "plan-review");
  assert.equal(unknown.label, "Plan-review");
  const bare = blueprintNodes(
    payload({ stages: [{ ...stage("mystery"), label: "" }] }),
    routes,
  );
  assert.equal(bare[0].label, "mystery");
});

test("node states, labels and flags", async () => {
  const { blueprintNodes } = await model;
  const by = Object.fromEntries(
    blueprintNodes(payload(), routes).map((n) => [n.id, n]),
  );
  assert.equal(by.implement.state, "failed");
  assert.equal(by.checks.state, "current");
  assert.equal(by.land.state, "disabled");
  assert.equal(by.brief.state, "pending");
  assert.equal(by.implement.time, "2m");
  assert.equal(by.implement.cost, "$0.42");
  assert.equal(by.implement.model, "Sonnet (sonnet)");
  assert.equal(by.plan.model, "Sonnet (sonnet)");
  assert.equal(by.review.model, "Codex");
  for (const id of ["checks", "base-checks"]) {
    assert.equal(by[id].ownerOnly, true);
    assert.equal(by[id].canToggle, false);
  }
  assert.deepEqual(
    Object.values(by)
      .filter((n) => n.canToggle)
      .map((n) => n.id),
    ["brief-check", "held-out", "land", "advisor"],
  );
  assert.deepEqual(
    Object.values(by)
      .filter((n) => n.routeEditable)
      .map((n) => n.id),
    ["plan", "brief-check", "implement", "review", "judge"],
  );
  assert.deepEqual(
    Object.values(by)
      .filter((n) => n.maxAttempts !== undefined)
      .map((n) => n.id),
    ["implement"],
  );
});

test("an auto brief check and judge show the model the engine resolved", async () => {
  const { blueprintNodes } = await model;
  const nodes = blueprintNodes(
    payload({
      stages: [
        stage("brief-check", { routeId: "auto", model: "sonnet" }),
        stage("judge", { routeId: "auto", model: "sonnet" }),
      ],
    }),
    routes,
  );
  assert.deepEqual(
    nodes.map((n) => n.model),
    ["Auto: sonnet", "Auto: sonnet"],
  );
});

test("an unresolved judge does not fall back to the preset route label", async () => {
  const { blueprintNodes } = await model;
  const [node] = blueprintNodes(
    payload({
      stages: [stage("judge", { routeId: "codex-luna", model: undefined })],
    }),
    routes,
  );
  assert.equal(node.routeLabel, undefined);
  assert.equal(node.model, undefined);
});

test("a resolved judge label without a model does not borrow the named route model", async () => {
  const { blueprintNodes } = await model;
  const [node] = blueprintNodes(
    payload({
      stages: [
        stage("judge", {
          routeId: "codex-luna",
          routeLabel: "No-model route",
          model: undefined,
        }),
      ],
    }),
    routes,
  );
  assert.equal(node.model, "No-model route");
});

test("an auto route uses its resolved label and model", async () => {
  const { blueprintNodes } = await model;
  const [node] = blueprintNodes(
    payload({
      stages: [
        stage("judge", {
          routeId: "auto",
          routeLabel: "Codex Luna",
          model: "gpt-5.6-luna",
        }),
      ],
    }),
    routes,
  );
  assert.equal(node.model, "Auto: Codex Luna (gpt-5.6-luna)");
});

test("route formatting omits a model that repeats the label", async () => {
  const { formatRouteLabel } = await model;
  assert.equal(formatRouteLabel("Codex", "Codex"), "Codex");
  assert.equal(formatRouteLabel("Codex", undefined, true), "Auto: Codex");
});

test("routed nodes show the route label and model", async () => {
  const { blueprintNodes } = await model;
  const [node] = blueprintNodes(
    payload({
      stages: [
        stage("implement", {
          routeId: "codex-luna",
          routeLabel: "Codex Luna",
          model: "gpt-5.6-luna",
        }),
      ],
    }),
    routes,
  );
  assert.equal(node.model, "Codex Luna (gpt-5.6-luna)");
});

test("the question branch goes to the overseer or to you", async () => {
  const { blueprintNodes } = await model;
  const held = blueprintNodes(
    payload({ branch: { from: "implement", to: "overseer" } }),
    routes,
  ).at(-1);
  assert.equal(held.kind, "branch");
  assert.equal(held.label, "Overseer");
  assert.equal(held.from, "implement");
  assert.equal(held.fromLabel, "Implement");
  const owner = blueprintNodes(
    payload({ branch: { from: "review", to: "owner" } }),
    routes,
  ).at(-1);
  assert.equal(owner.label, "You");
  assert.equal(owner.fromLabel, "Review");
});

test("stageEdit without a preset changes only the globals", async () => {
  const { stageEdit } = await model;
  const base = settings();
  const edit = (id, change) => stageEdit(base, "/r", null, id, change);
  assert.equal(
    edit("advisor", { kind: "enabled", value: true }).experiments.advisor,
    true,
  );
  assert.equal(
    edit("held-out", { kind: "enabled", value: true }).experiments.heldOut,
    true,
  );
  assert.equal(
    edit("held-out", { kind: "enabled", value: true }).experiments
      .groundedChecks,
    undefined,
  );
  assert.equal(
    edit("land", { kind: "enabled", value: true }).experiments.land,
    true,
  );
  assert.equal(
    edit("brief-check", { kind: "enabled", value: false }).briefCheckRoute,
    "",
  );
  const off = stageEdit(
    settings({ briefCheckRoute: "" }),
    "/r",
    null,
    "brief-check",
    { kind: "enabled", value: true },
  );
  assert.equal(off.briefCheckRoute, "claude-sonnet");
  assert.equal(
    edit("brief-check", { kind: "enabled", value: true, route: "codex" })
      .briefCheckRoute,
    "codex",
  );
  assert.equal(
    edit("plan", { kind: "route", value: "codex" }).planner,
    "codex",
  );
  assert.equal(edit("review", { kind: "route", value: "auto" }).review, "auto");
  assert.equal(
    edit("implement", { kind: "route", value: "codex" }).tiers.standard,
    "codex",
  );
  assert.equal(
    edit("implement", {
      kind: "route",
      value: "codex-luna",
      tier: "mechanical",
    }).tiers.mechanical,
    "codex-luna",
  );
  assert.equal(
    edit("judge", { kind: "route", value: "codex-luna" }).experiments
      .judgeRoute,
    "codex-luna",
  );
  assert.equal(
    edit("implement", { kind: "maxAttempts", value: 5 }).maxAttempts,
    5,
  );
  const next = edit("review", { kind: "route", value: "auto" });
  assert.equal(next.presets, undefined);
  assert.equal(next.repoPresets, undefined);
  assert.equal(base.review, "codex");
});

test("stageEdit on a custom preset edits only that preset", async () => {
  const { stageEdit } = await model;
  const custom = {
    id: "mine",
    label: "Mine",
    builtin: false,
    values: { advisor: false, maxAttempts: 2 },
  };
  const base = settings({
    presets: [custom],
    repoPresets: { "/r": "mine" },
  });
  const next = stageEdit(base, "/r", "mine", "advisor", {
    kind: "enabled",
    value: true,
  });
  assert.deepEqual(next.presets, [
    { ...custom, values: { advisor: true, maxAttempts: 2 } },
  ]);
  assert.deepEqual(next.experiments, base.experiments);
  assert.equal(next.maxAttempts, base.maxAttempts);
  assert.deepEqual(next.repoPresets, { "/r": "mine" });
  const tier = stageEdit(base, "/r", "mine", "implement", {
    kind: "route",
    value: "codex",
    tier: "mechanical",
  });
  const allTiers = stageEdit(
    stageEdit(tier, "/r", "mine", "implement", {
      kind: "route",
      value: "codex-luna",
      tier: "standard",
    }),
    "/r",
    "mine",
    "implement",
    { kind: "route", value: "codex", tier: "hard" },
  );
  const withJudge = stageEdit(allTiers, "/r", "mine", "judge", {
    kind: "route",
    value: "codex-luna",
  });
  assert.deepEqual(withJudge.presets[0].values, {
    advisor: false,
    maxAttempts: 2,
    tierRoutes: {
      mechanical: "codex",
      standard: "codex-luna",
      hard: "codex",
    },
    judge: "codex-luna",
  });
  assert.equal(tier.tiers.standard, "claude-sonnet");
  const held = stageEdit(base, "/r", "mine", "held-out", {
    kind: "enabled",
    value: false,
  });
  assert.deepEqual(held.presets[0].values, {
    advisor: false,
    maxAttempts: 2,
    heldOut: false,
  });
  assert.deepEqual(held.experiments, base.experiments);
});

test("stageEdit on a built-in appends one custom copy and points the repo at it", async () => {
  const { stageEdit } = await model;
  const fast = {
    id: "fast",
    label: "Fast",
    builtin: true,
    values: { briefCheck: false, maxAttempts: 2 },
  };
  const base = settings();
  const next = stageEdit(
    base,
    "/r",
    "fast",
    "judge",
    { kind: "route", value: "codex-luna" },
    [fast],
  );
  assert.equal(next.presets.length, 1);
  const copy = next.presets[0];
  assert.equal(copy.builtin, false);
  assert.notEqual(copy.id, "fast");
  assert.equal(copy.label, "Fast (custom)");
  assert.deepEqual(copy.values, {
    briefCheck: false,
    maxAttempts: 2,
    judge: "codex-luna",
  });
  assert.deepEqual(next.repoPresets, { "/r": copy.id });
  assert.deepEqual(next.experiments, base.experiments);
  assert.equal(next.maxAttempts, base.maxAttempts);
  // Editing again edits the copy, not a second one.
  const again = stageEdit(next, "/r", copy.id, "implement", {
    kind: "maxAttempts",
    value: 6,
  });
  assert.equal(again.presets.length, 1);
  assert.equal(again.presets[0].values.maxAttempts, 6);
  // A second copy of the same built-in gets its own id.
  const second = stageEdit(
    next,
    "/r2",
    "fast",
    "advisor",
    {
      kind: "enabled",
      value: false,
    },
    [fast],
  );
  assert.equal(second.presets.length, 2);
  assert.notEqual(second.presets[1].id, copy.id);
});

test("a preset leaves land to the global setting", async () => {
  const { stageEdit } = await model;
  const base = settings();
  assert.equal(
    stageEdit(base, "/r", "fast", "land", { kind: "enabled", value: true }),
    base,
  );
});

test("presets and repoPresets survive the Settings JSON round trip", async () => {
  const { formatSettingsJson, parseSettingsJson } = await json;
  const withPresets = settings({
    presets: [
      { id: "mine", label: "Mine", builtin: false, values: { advisor: true } },
    ],
    repoPresets: { "/r": "mine" },
  });
  const parsed = parseSettingsJson(formatSettingsJson(withPresets));
  assert.equal(parsed.ok, true);
  assert.deepEqual(parsed.settings, withPresets);
  const source = fs.readFileSync(
    path.join(__dirname, "../src/orchestrator/OrchestratorSettings.tsx"),
    "utf8",
  );
  assert.match(source, /orchestratorClient\.settingsSet\(next\)/);
});

test("only a task without an implement attempt can pick a preset", async () => {
  const { canPickTaskPreset } = await model;
  assert.equal(canPickTaskPreset({ attempts: [] }), true);
  assert.equal(canPickTaskPreset({ attempts: [{ stage: "plan" }] }), true);
  assert.equal(
    canPickTaskPreset({
      attempts: [{ stage: "plan" }, { stage: "implement" }],
    }),
    false,
  );
});
