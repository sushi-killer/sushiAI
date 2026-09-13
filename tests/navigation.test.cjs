const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/app/navigation.ts");

const tasks = { extensionId: "test.probe", surfaceId: "probe.ledger" };
const tasksSection = { kind: "extension", ...tasks };
const core = (id) => ({ kind: "core", id });
const savedRoute = {
  kind: "extension",
  surfaceId: "extension.test.probe.probe.ledger",
  presentation: "section",
  extensionId: tasks.extensionId,
  targetSurfaceId: tasks.surfaceId,
};

test("navigation restores the 0.0.6 storage shape, which never wrote a route", async () => {
  const { restoreNavigation } = await library;
  assert.deepEqual(
    restoreNavigation({ mode: "Chat", section: "Skills" }),
    { mode: "Chat", section: core("Skills") },
    "mode and section survive an upgrade from a release without routes",
  );
  assert.deepEqual(restoreNavigation(null), { mode: "Code", section: null });
  assert.equal(
    restoreNavigation({ mode: "Code", section: "Garbage" }).section,
    null,
    "a section no screen renders is dropped instead of blanking the shell",
  );
});

test("navigation restores and re-emits an extension route", async () => {
  const { restoreNavigation, routeOf } = await library;
  const state = restoreNavigation({
    mode: "Code",
    section: "",
    route: savedRoute,
  });
  assert.deepEqual(state.section, tasksSection);
  assert.deepEqual(routeOf(state), savedRoute);
});

test("a page an extension contributed is a section like any other", async () => {
  const { navigationReducer, restoreNavigation } = await library;
  const onExtensionPage = restoreNavigation({
    mode: "Code",
    section: "Skills",
    route: savedRoute,
  });
  // The shell reads `mode` and one `section`; there is no separate slot an
  // extension page could sit in, so no call site can forget to clear it.
  assert.equal(onExtensionPage.section.kind, "extension");

  for (const action of [
    { type: "setMode", mode: "Agent" },
    { type: "toggleSection", section: core("Dashboard") },
    { type: "showWorkspace" },
  ]) {
    const next = navigationReducer(onExtensionPage, action);
    assert.notEqual(
      next.section?.kind,
      "extension",
      `${action.type} leaves no extension page open`,
    );
  }

  assert.deepEqual(
    navigationReducer(onExtensionPage, { type: "showWorkspace" }),
    { mode: "Code", section: null },
  );
  assert.equal(
    navigationReducer(onExtensionPage, { type: "setMode", mode: "Agent" })
      .section,
    null,
    "switching mode leaves the section pages",
  );
});

test("toggling the same section twice returns to the workspace", async () => {
  const { navigationReducer } = await library;
  const start = { mode: "Code", section: null };
  const opened = navigationReducer(start, {
    type: "toggleSection",
    section: core("Routines"),
  });
  assert.deepEqual(opened.section, core("Routines"));
  assert.equal(
    navigationReducer(opened, {
      type: "toggleSection",
      section: core("Routines"),
    }).section,
    null,
  );
  assert.deepEqual(
    navigationReducer(opened, {
      type: "toggleSection",
      section: core("Skills"),
    }).section,
    core("Skills"),
  );
  // Same rule for a contributed page: clicking its entry again goes back.
  const onPage = navigationReducer(start, {
    type: "toggleSection",
    section: tasksSection,
  });
  assert.deepEqual(onPage.section, tasksSection);
  assert.equal(
    navigationReducer(onPage, { type: "toggleSection", section: tasksSection })
      .section,
    null,
  );
});

test("any page opens in the Code shell, whoever contributed it", async () => {
  const { navigationReducer, routeOf } = await library;
  const next = navigationReducer(
    { mode: "Code", section: core("Skills") },
    { type: "toggleSection", section: tasksSection },
  );
  assert.deepEqual(next, { mode: "Code", section: tasksSection });
  assert.equal(
    routeOf(next).surfaceId,
    "extension.test.probe.probe.ledger",
    "the route id is built from ids, not from the visible label",
  );
  // Agent and Chat portal their own list into the sidebar, and opening a page
  // unmounts the view that does the portalling. Keeping the mode would leave
  // the sidebar blank and the workspace toolbar hidden.
  for (const mode of ["Agent", "Chat"])
    for (const section of [tasksSection, core("Skills")])
      assert.equal(
        navigationReducer(
          { mode, section: null },
          {
            type: "toggleSection",
            section,
          },
        ).mode,
        "Code",
        `${section.kind} page opened from ${mode}`,
      );
  // Closing one does not drag you into Code.
  assert.equal(
    navigationReducer(
      { mode: "Chat", section: core("Skills") },
      { type: "toggleSection", section: core("Skills") },
    ).mode,
    "Chat",
  );
});

test("idempotent actions return the same object so effects do not re-run", async () => {
  const { navigationReducer } = await library;
  const workspace = { mode: "Code", section: null };
  assert.equal(
    navigationReducer(workspace, { type: "showWorkspace" }),
    workspace,
  );
});

test("a route pointing at a surface that is gone closes itself", async () => {
  const { shouldCloseExtension } = await library;
  const onPage = { mode: "Code", section: tasksSection };
  assert.equal(
    shouldCloseExtension(onPage, () => true),
    false,
    "an available surface stays open",
  );
  assert.equal(
    shouldCloseExtension(onPage, () => false),
    true,
    "a disabled or uninstalled extension does not strand the shell",
  );
  assert.equal(
    shouldCloseExtension(
      { mode: "Code", section: core("Skills") },
      () => false,
    ),
    false,
    "a core section is never closed by a missing surface",
  );
});

test("core routes round-trip through the saved shape", async () => {
  const { restoreNavigation, routeOf } = await library;
  for (const [mode, name] of [
    ["Code", ""],
    ["Agent", ""],
    ["Chat", ""],
    ["Code", "Dashboard"],
    ["Code", "Skills"],
  ]) {
    const state = { mode, section: name ? core(name) : null };
    const route = routeOf(state);
    assert.equal(route.kind, "core");
    const restored = restoreNavigation({ mode, section: name, route });
    assert.deepEqual(restored, state, `${mode}/${name || "workspace"}`);
  }
});
