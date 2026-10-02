const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/extensions/openSignal.ts");
const actions = import("../src/workspace/workspace-actions.ts");
const layoutLibrary = import("../src/layout.ts");

const surface = (over = {}) => ({
  id: "preview",
  extensionId: "builtin.artifacts",
  title: "Preview",
  allowedHosts: ["workspace.pane"],
  view: { kind: "core", viewId: "artifacts.preview" },
  ...over,
});
const registryOf = (...surfaces) => ({ availableSurfaces: () => surfaces });
const agent = { id: "a1", kind: "agent", title: "Claude", herdrId: "w3S:p1D" };
const signal = {
  paneId: "w3S:p1D",
  extensionId: "builtin.artifacts",
  surfaceId: "preview",
  nonce: "1-9",
  arg: "/tmp/a b.md",
};

test("parseOpenSignal keeps spaces in the argument", async () => {
  const { parseOpenSignal } = await library;
  assert.deepEqual(
    parseOpenSignal(
      "w3S:p1D builtin.artifacts/preview 17-42 /tmp/my notes/a b.md",
    ),
    {
      paneId: "w3S:p1D",
      extensionId: "builtin.artifacts",
      surfaceId: "preview",
      nonce: "17-42",
      arg: "/tmp/my notes/a b.md",
    },
  );
  assert.equal(parseOpenSignal("p1 ext/surf n1").arg, "");
});

test("parseOpenSignal rejects malformed values", async () => {
  const { parseOpenSignal } = await library;
  for (const value of [
    "",
    "p1",
    " ext/surf n arg",
    "p1 nosurface n arg",
    "p1 /surf n arg",
    "p1 ext/ n arg",
    "p1 a/b/c n arg",
    "p1 ext/surf",
    undefined,
    42,
  ])
    assert.equal(parseOpenSignal(value), null, String(value));
});

test("resolveOpenSignal targets the agent panel itself", async () => {
  const { resolveOpenSignal } = await library;
  const other = {
    id: "t1",
    kind: "terminal",
    title: "zsh",
    herdrId: "w3S:p2D",
  };
  assert.deepEqual(
    resolveOpenSignal(registryOf(surface()), [other, agent], signal),
    { kind: "companion", panelId: "a1" },
  );
});

test("resolveOpenSignal is unavailable for an unknown pane or host", async () => {
  const { resolveOpenSignal } = await library;
  const result = resolveOpenSignal(registryOf(surface()), [agent], {
    ...signal,
    paneId: "ssh-other:p9",
  });
  assert.equal(result.kind, "unavailable");
  assert.equal(
    resolveOpenSignal(registryOf(surface()), [], signal).kind,
    "unavailable",
  );
});

test("resolveOpenSignal is unavailable for a missing or disabled surface", async () => {
  const { resolveOpenSignal } = await library;
  // availableSurfaces() already drops a disabled extension's surfaces.
  assert.equal(
    resolveOpenSignal(registryOf(), [agent], signal).kind,
    "unavailable",
  );
  assert.equal(
    resolveOpenSignal(registryOf(surface({ id: "other" })), [agent], signal)
      .kind,
    "unavailable",
  );
});

test("resolveOpenSignal rejects declarative views and non-pane hosts", async () => {
  const { resolveOpenSignal } = await library;
  const declarative = surface({
    view: { kind: "declarative", schemaVersion: 2, document: {} },
  });
  assert.equal(
    resolveOpenSignal(registryOf(declarative), [agent], signal).kind,
    "unavailable",
  );
  const page = surface({ allowedHosts: ["app.page"] });
  assert.equal(
    resolveOpenSignal(registryOf(page), [agent], signal).kind,
    "unavailable",
  );
});

const workspaceOf = (...panels) => [
  { id: "w1", name: "w", cwd: "/tmp", panels, layout: null },
];
const target = { extensionId: "builtin.artifacts", surfaceId: "preview" };

test("first signal creates the companion, later signals update its arg, a hidden one re-opens", async () => {
  const { openCompanion, patchCompanion } = await actions;
  const t1 = { id: "t1", kind: "terminal", title: "zsh" };
  const first = openCompanion(workspaceOf(agent, t1), "a1", target, {
    arg: "/p/a.md",
  });
  assert.deepEqual(first[0].panels[0].companion, {
    ...target,
    args: { arg: "/p/a.md" },
    open: true,
  });
  assert.equal(first[0].panels[1], t1, "other panels are untouched");
  // The view stores its own state in args; a later signal keeps it.
  let next = patchCompanion(first, "a1", { args: { recent: '["/p/a.md"]' } });
  next = openCompanion(next, "a1", target, { arg: "/p/b.md" });
  assert.deepEqual(next[0].panels[0].companion.args, {
    arg: "/p/b.md",
    recent: '["/p/a.md"]',
  });
  // Hiding keeps args and history.
  const hidden = patchCompanion(next, "a1", { open: false });
  assert.equal(hidden[0].panels[0].companion.open, false);
  assert.equal(hidden[0].panels[0].companion.args.recent, '["/p/a.md"]');
  // A new signal while hidden opens it again.
  const again = openCompanion(hidden, "a1", target, { arg: "/p/c.md" });
  assert.equal(again[0].panels[0].companion.open, true);
  assert.equal(again[0].panels[0].companion.args.arg, "/p/c.md");
  assert.equal(next[0].panels[0].companion.args.arg, "/p/b.md", "pure");
});

test("patchCompanion clamps the ratio and ignores a pane without a companion", async () => {
  const { openCompanion, patchCompanion, companionRatio } = await actions;
  const base = openCompanion(workspaceOf(agent), "a1", target, {
    arg: "/x.md",
  });
  const ratioOf = (value) =>
    patchCompanion(base, "a1", { ratio: value })[0].panels[0].companion.ratio;
  assert.equal(ratioOf(0.1), 0.25);
  assert.equal(ratioOf(0.9), 0.75);
  assert.equal(ratioOf(0.6), 0.6);
  assert.equal(companionRatio(undefined), 0.5);
  assert.equal(companionRatio(Number.NaN), 0.5);
  const bare = workspaceOf(agent);
  assert.deepEqual(patchCompanion(bare, "a1", { open: true })[0], bare[0]);
});

test("a companion goes with its agent panel and survives a reopened session", async () => {
  const { openCompanion, removeClosedPanels, reopenInSlot } = await actions;
  const { leaf } = await layoutLibrary;
  const list = openCompanion(
    [{ ...workspaceOf(agent)[0], layout: leaf("a1") }],
    "a1",
    target,
    { arg: "/x.md" },
  );
  assert.deepEqual(removeClosedPanels(list, new Set(["a1"]))[0].panels, []);
  const reopened = reopenInSlot(list[0], "a1", {
    ...agent,
    id: "a2",
    herdrId: "w3S:p7D",
  });
  assert.equal(reopened.panels[0].id, "a2");
  assert.equal(reopened.panels[0].companion.args.arg, "/x.md");
});

test("detectOpenSignals treats the first value as a baseline", async () => {
  const { detectOpenSignals } = await library;
  const workspace = (value, extra = {}) => ({
    id: "w1",
    name: "w",
    cwd: "/tmp",
    panels: [],
    layout: null,
    herdrTokens: value === undefined ? {} : { sushiai_open: value },
    ...extra,
  });
  // App start: the stored value is the baseline, not a request.
  let step = detectOpenSignals({}, [workspace("p1 e/s n1 /f.md")]);
  assert.deepEqual(step.fresh, []);
  // Same value on the next snapshot: nothing.
  step = detectOpenSignals(step.seen, [workspace("p1 e/s n1 /f.md")]);
  assert.deepEqual(step.fresh, []);
  // A change is acted on once.
  step = detectOpenSignals(step.seen, [workspace("p1 e/s n2 /f.md")]);
  assert.equal(step.fresh.length, 1);
  assert.equal(step.fresh[0].value, "p1 e/s n2 /f.md");
  step = detectOpenSignals(step.seen, [workspace("p1 e/s n2 /f.md")]);
  assert.deepEqual(step.fresh, []);
  // A workspace without reported tokens yet is not baselined.
  const pending = detectOpenSignals(step.seen, [
    workspace("x", { id: "w2", herdrTokens: undefined }),
  ]);
  assert.equal("w2" in pending.seen, false);
  // A workspace that appears later gets its own baseline.
  step = detectOpenSignals(step.seen, [
    workspace("p1 e/s n2 /f.md"),
    workspace("p2 e/s n1 /v.md", { id: "w3" }),
  ]);
  assert.deepEqual(step.fresh, []);
  // No token yet, then one appears: that is a change.
  step = detectOpenSignals({}, [workspace(undefined)]);
  step = detectOpenSignals(step.seen, [workspace("p1 e/s n3 /f.md")]);
  assert.equal(step.fresh.length, 1);
});

test("a new nonce for the same path is fresh and addresses the same panel", async () => {
  const { detectOpenSignals, parseOpenSignal, resolveOpenSignal } =
    await library;
  const workspace = (value) => ({
    id: "w1",
    name: "w",
    cwd: "/tmp",
    panels: [],
    layout: null,
    herdrTokens: { sushiai_open: value },
  });
  const value = (nonce) =>
    `w3S:p1D builtin.artifacts/preview ${nonce} /tmp/plan.md`;
  let step = detectOpenSignals({}, [workspace(value("1-10"))]);
  // The same value again is not a request.
  step = detectOpenSignals(step.seen, [workspace(value("1-10"))]);
  assert.deepEqual(step.fresh, []);
  // Same path, new nonce: fresh, so a hidden companion opens again.
  step = detectOpenSignals(step.seen, [workspace(value("2-10"))]);
  assert.equal(step.fresh.length, 1);
  const parsed = parseOpenSignal(step.fresh[0].value);
  assert.equal(parsed.arg, "/tmp/plan.md");
  assert.deepEqual(resolveOpenSignal(registryOf(surface()), [agent], parsed), {
    kind: "companion",
    panelId: "a1",
  });
});

test("the argument token joins the open token, and a relative path is from the pane", async () => {
  const { detectOpenSignals, parseOpenSignal, absoluteArg } = await library;
  const workspace = (tokens) => ({ id: "w1", panels: [], herdrTokens: tokens });
  let step = detectOpenSignals({}, [workspace({})]);
  step = detectOpenSignals(step.seen, [
    workspace({
      sushiai_open: "p1 e/s n1",
      sushiai_open_arg: "artifacts/a b.md",
    }),
  ]);
  assert.equal(parseOpenSignal(step.fresh[0].value).arg, "artifacts/a b.md");
  assert.equal(
    absoluteArg("artifacts/a.md", "/w/proj/"),
    "/w/proj/artifacts/a.md",
  );
  assert.equal(absoluteArg("./a.md", "/w/proj"), "/w/proj/a.md");
  assert.equal(absoluteArg("/x/a.md", "/w/proj"), "/x/a.md");
  assert.equal(absoluteArg("a.md", undefined), "a.md");
});
