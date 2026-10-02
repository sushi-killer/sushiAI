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
const preview = (id, beside, extra = {}) => ({
  id,
  kind: "extension",
  title: "Preview",
  extension: {
    extensionId: "builtin.artifacts",
    contributionId: "preview",
    instanceId: id,
    stateVersion: 1,
    beside,
    ...extra,
  },
});
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

test("resolveOpenSignal adds beside the agent pane", async () => {
  const { resolveOpenSignal } = await library;
  assert.deepEqual(resolveOpenSignal(registryOf(surface()), [agent], signal), {
    kind: "add",
    besidePanelId: "a1",
  });
});

test("resolveOpenSignal updates the panel already beside that agent", async () => {
  const { resolveOpenSignal } = await library;
  const other = preview("x2", "someone-else");
  assert.deepEqual(
    resolveOpenSignal(
      registryOf(surface()),
      [agent, other, preview("x1", "a1")],
      signal,
    ),
    { kind: "update", panelId: "x1" },
  );
  assert.equal(
    resolveOpenSignal(registryOf(surface()), [agent, other], signal).kind,
    "add",
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

test("placeBeside splits the target 50/50 and leaves other leaves alone", async () => {
  const { placeBeside } = await actions;
  const { leaf, split, leafIds } = await layoutLibrary;
  const panels = [agent, { id: "t1", kind: "terminal", title: "zsh" }];
  const layout = split(leaf("a1"), leaf("t1"), "column", 0.7);
  const workspace = { id: "w1", name: "w", cwd: "/tmp", panels, layout };
  const next = placeBeside(workspace, "a1", preview("x1", "a1"));
  assert.equal(next.panels.length, 3);
  assert.deepEqual(leafIds(next.layout), ["a1", "x1", "t1"]);
  assert.equal(next.layout.axis, "column");
  assert.equal(next.layout.ratio, 0.7);
  assert.equal(next.layout.b.id, "t1");
  const inner = next.layout.a;
  assert.equal(inner.axis, "row");
  assert.equal(inner.ratio, 0.5);
  assert.equal(inner.a.id, "a1");
  assert.equal(inner.b.id, "x1");
});

test("placeBeside leaves the workspace unchanged when the target is missing", async () => {
  const { placeBeside } = await actions;
  const { leaf } = await layoutLibrary;
  const workspace = {
    id: "w1",
    name: "w",
    cwd: "/tmp",
    panels: [agent],
    layout: leaf("a1"),
  };
  assert.equal(
    placeBeside(workspace, "nope", preview("x1", "nope")),
    workspace,
  );
  const empty = { ...workspace, layout: null };
  assert.equal(placeBeside(empty, "a1", preview("x1", "a1")), empty);
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

test("a new nonce for the same path is fresh and reopens a closed Preview", async () => {
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
  // Same path, new nonce: fresh, and with the Preview closed it opens again.
  step = detectOpenSignals(step.seen, [workspace(value("2-10"))]);
  assert.equal(step.fresh.length, 1);
  const parsed = parseOpenSignal(step.fresh[0].value);
  assert.equal(parsed.arg, "/tmp/plan.md");
  assert.deepEqual(resolveOpenSignal(registryOf(surface()), [agent], parsed), {
    kind: "add",
    besidePanelId: "a1",
  });
  // With the Preview open the same request only updates that pane.
  assert.deepEqual(
    resolveOpenSignal(
      registryOf(surface()),
      [agent, preview("x1", "a1")],
      parsed,
    ),
    { kind: "update", panelId: "x1" },
  );
});
