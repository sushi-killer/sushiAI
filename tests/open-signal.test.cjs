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
const agent = { id: "a1", kind: "agent", title: "Claude", sessionId: "s1" };
const shell = { id: "t1", kind: "terminal", title: "zsh", sessionId: "s2" };
const workspaces = [
  { id: "w-local", name: "app", cwd: "/w/app", panels: [shell, agent] },
  {
    id: "w-remote",
    name: "app",
    cwd: "/w/app",
    connection: "ssh:box",
    panels: [{ id: "r1", kind: "agent", title: "Codex", sessionId: "s1" }],
  },
];
const event = (params = {}, host = "local", method = "session.open") => ({
  method,
  host,
  generation: 1,
  params: {
    id: "s1",
    target: "artifacts/preview",
    arg: "/w/app/artifacts/a b.md",
    nonce: "n1",
    ...params,
  },
});

test("parseOpenEvent maps a session.open notification and the short extension name", async () => {
  const { parseOpenEvent } = await library;
  assert.deepEqual(parseOpenEvent(event()), {
    host: "local",
    sessionId: "s1",
    extensionId: "builtin.artifacts",
    surfaceId: "preview",
    nonce: "n1",
    arg: "/w/app/artifacts/a b.md",
  });
  assert.equal(
    parseOpenEvent(event({ target: "acme.tools/view" })).extensionId,
    "acme.tools",
  );
});

test("parseOpenEvent ignores other events and rejects malformed ones", async () => {
  const { parseOpenEvent } = await library;
  assert.equal(parseOpenEvent(event({}, "local", "session.ask")), null);
  for (const params of [
    { target: "nosurface" },
    { target: "/surf" },
    { target: "ext/" },
    { target: "a/b/c" },
    { nonce: "" },
    { id: "" },
    { arg: 4 },
  ])
    assert.equal(parseOpenEvent(event(params)), null, JSON.stringify(params));
});

test("resolveOpenSignal finds the panel by session id and host", async () => {
  const { parseOpenEvent, resolveOpenSignal } = await library;
  const registry = registryOf(surface());
  const local = resolveOpenSignal(
    registry,
    workspaces,
    parseOpenEvent(event()),
  );
  assert.equal(local.kind, "companion");
  assert.equal(local.panel.id, "a1");
  const remote = resolveOpenSignal(
    registry,
    workspaces,
    parseOpenEvent(event({}, "box")),
  );
  assert.equal(remote.kind, "companion");
  assert.equal(remote.panel.id, "r1", "same session id, other host");
});

test("resolveOpenSignal rejects an unknown session, host, surface or kind of surface", async () => {
  const { parseOpenEvent, resolveOpenSignal } = await library;
  const resolve = (registry, params, host) =>
    resolveOpenSignal(
      registry,
      workspaces,
      parseOpenEvent(event(params, host)),
    );
  const ok = registryOf(surface());
  assert.equal(resolve(ok, { id: "nope" }).kind, "rejected");
  assert.equal(resolve(ok, {}, "other-host").kind, "rejected");
  assert.equal(resolve(registryOf(), {}).kind, "rejected");
  assert.equal(resolve(ok, { target: "artifacts/other" }).kind, "rejected");
  assert.equal(resolve(ok, { target: "acme/tools" }).kind, "rejected");
  const declarative = surface({
    view: { kind: "declarative", schemaVersion: 2, document: {} },
  });
  assert.equal(resolve(registryOf(declarative), {}).kind, "rejected");
  const page = surface({ allowedHosts: ["app.page"] });
  assert.equal(resolve(registryOf(page), {}).kind, "rejected");
});

test("rememberNonce reports a repeated nonce once and stays bounded", async () => {
  const { rememberNonce } = await library;
  let step = rememberNonce([], "n1");
  assert.equal(step.fresh, true);
  step = rememberNonce(step.seen, "n1");
  assert.equal(step.fresh, false);
  step = rememberNonce(step.seen, "n2");
  assert.equal(step.fresh, true);
  let seen = [];
  for (let i = 0; i < 1000; i++) seen = rememberNonce(seen, `n${i}`).seen;
  assert.ok(seen.length <= 256);
  assert.equal(rememberNonce(seen, "n999").fresh, false);
});

test("the open handler opens beside the right panel once per nonce and ignores unknown targets", async () => {
  const { createOpenHandler } = await library;
  const opened = [];
  const warned = [];
  const handle = createOpenHandler(
    () => ({ workspaces, registry: registryOf(surface()) }),
    (...call) => opened.push(call),
    (message) => warned.push(message),
  );
  const target = { extensionId: "builtin.artifacts", surfaceId: "preview" };
  handle(event());
  assert.deepEqual(opened, [
    ["a1", target, { arg: "/w/app/artifacts/a b.md" }],
  ]);
  // The same nonce again, and an unrelated event: nothing.
  handle(event());
  handle(event({}, "local", "session.ask"));
  assert.equal(opened.length, 1);
  // A new nonce for the same path opens again (a hidden Preview comes back).
  handle(event({ nonce: "n2" }));
  assert.equal(opened.length, 2);
  // The remote host's session of the same id is another panel.
  handle(event({ nonce: "n3" }, "box"));
  assert.equal(opened[2][0], "r1");
  // An unknown target is dropped with a warning only.
  handle(event({ nonce: "n4", target: "nope/nothing" }));
  assert.equal(opened.length, 3);
  assert.equal(warned.length, 1);
  assert.match(warned[0], /builtin\.nope\/nothing/);
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
    sessionId: "s7",
  });
  assert.equal(reopened.panels[0].id, "a2");
  assert.equal(reopened.panels[0].companion.args.arg, "/x.md");
});
