const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/extensions/routes.ts");

test("route ids are stable and independent from visible labels", async () => {
  const { routeFromLegacy, extensionRoute, validRoute } = await library;
  assert.deepEqual(routeFromLegacy("Code", "Dashboard"), {
    kind: "core",
    surfaceId: "core.section.dashboard",
    presentation: "section",
  });
  assert.deepEqual(routeFromLegacy("Chat"), {
    kind: "core",
    surfaceId: "core.mode.chat",
    presentation: "workspace",
  });
  // A page an extension contributes is a section of the Code shell, drawn
  // where Skills is drawn - never a presentation of its own.
  assert.deepEqual(extensionRoute("test.probe", "probe.ledger"), {
    kind: "extension",
    surfaceId: "extension.test.probe.probe.ledger",
    presentation: "section",
    extensionId: "test.probe",
    targetSurfaceId: "probe.ledger",
  });
  assert.equal(
    validRoute({
      kind: "extension",
      surfaceId: "extension.test.probe.probe.ledger",
      presentation: "page",
      extensionId: "test.probe",
      targetSurfaceId: "probe.ledger",
    }),
    true,
    "a route saved before that change still restores",
  );
  assert.equal(
    validRoute({
      kind: "core",
      surfaceId: "core.mode.code",
      presentation: "workspace",
    }),
    true,
  );
  assert.equal(
    validRoute({ surfaceId: "", presentation: "workspace" }),
    false,
    "an empty surface id is not a route",
  );
  assert.equal(
    validRoute({ surfaceId: "core.mode.code", presentation: "workspace" }),
    false,
    "a route without a kind is not restored",
  );
  assert.equal(
    validRoute({
      kind: "extension",
      surfaceId: "extension.test.probe.probe.ledger",
      presentation: "page",
    }),
    false,
    "an extension route must name its extension and surface",
  );
});

const registryLibrary = import("../src/extensions/registry.ts");
const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");
const fixture = require("./fixtures/extensions/probe/manifest.json");

async function registryWith(manifest, status = "active") {
  const { ExtensionRegistry } = await registryLibrary;
  const validated = validateExtensionManifest({
    source: { kind: "local", path: __dirname },
    ...manifest,
  });
  const registry = new ExtensionRegistry();
  const snapshot = {
    schemaVersion: 2,
    version: 1,
    extensions: [{ manifest: validated, status }],
    surfaces: validated.contributions.surfaces,
    navigation: validated.contributions.navigation,
    actions: validated.contributions.actions,
    commands: validated.contributions.commands,
  };
  registry.applySnapshot(snapshot);
  return { registry, snapshot };
}

/** probe.ledger's view puts commands on the page; a manifest that isolates
 * the surface without also keeping those commands would dangle, so callers
 * that trim contributions down to just this surface strip that list first. */
const soloLedger = () => {
  const surface = structuredClone(
    fixture.contributions.surfaces.find((s) => s.id === "probe.ledger"),
  );
  delete surface.view.views[0].actions;
  return surface;
};

const withPlacement = (placement) => ({
  ...fixture,
  contributions: {
    ...fixture.contributions,
    navigation: [
      {
        ...fixture.contributions.navigation.find(
          (n) => n.id === "probe.nav-mode",
        ),
        allowedPlacements: [placement],
        defaultPlacement: placement,
      },
    ],
  },
});

test("a mode.primary surface resolves to an app page", async () => {
  const { resolveExtensionCommand, primaryNavigation, activePage } =
    await library;
  const { registry, snapshot } = await registryWith(fixture);
  assert.deepEqual(
    resolveExtensionCommand(
      registry,
      snapshot,
      "test.probe",
      "probe.open-ledger",
    ),
    { kind: "page", extensionId: "test.probe", surfaceId: "probe.ledger" },
  );
  assert.equal(primaryNavigation(registry).length, 1);
  assert.ok(
    activePage(registry, {
      extensionId: "test.probe",
      surfaceId: "probe.ledger",
    }),
  );
});

test("a sidebar entry can open a page, and every resolver agrees", async () => {
  const {
    resolveExtensionCommand,
    resolveNavigation,
    primaryNavigation,
    activePage,
  } = await library;
  const { registry, snapshot } = await registryWith(
    withPlacement("sidebar.primary"),
  );
  const target = {
    extensionId: "test.probe",
    surfaceId: "probe.ledger",
  };
  // The surface hosts app.page, so a sidebar entry is a page control like any
  // other. All three resolvers used to disagree here, which opened the page
  // and then closed it on the next render.
  assert.deepEqual(
    resolveExtensionCommand(
      registry,
      snapshot,
      "test.probe",
      "probe.open-ledger",
    ),
    {
      kind: "page",
      ...target,
    },
  );
  assert.equal(
    resolveNavigation(registry, {
      extensionId: "test.probe",
      targetSurfaceId: "probe.ledger",
    }).kind,
    "page",
  );
  assert.ok(activePage(registry, target), "the page stays open");
  assert.equal(
    primaryNavigation(registry).length,
    0,
    "it is still not a top-bar entry",
  );
});

test("a surface with no page control is never opened as a page", async () => {
  const { resolveNavigation, activePage } = await library;
  const paneOnly = JSON.parse(JSON.stringify(fixture));
  const ledger = paneOnly.contributions.surfaces.find(
    (s) => s.id === "probe.ledger",
  );
  delete ledger.view.views[0].actions;
  ledger.allowedHosts = ["workspace.pane"];
  ledger.defaultHost = "workspace.pane";
  paneOnly.contributions.surfaces = [ledger];
  paneOnly.contributions.navigation = [
    {
      ...fixture.contributions.navigation.find(
        (n) => n.id === "probe.nav-mode",
      ),
      allowedPlacements: ["panel.picker"],
      defaultPlacement: "panel.picker",
    },
  ];
  paneOnly.contributions.actions = [];
  paneOnly.contributions.commands = [];
  const { registry } = await registryWith(paneOnly);
  const target = {
    extensionId: "test.probe",
    surfaceId: "probe.ledger",
  };
  assert.equal(
    resolveNavigation(registry, {
      extensionId: "test.probe",
      targetSurfaceId: "probe.ledger",
    }).kind,
    "pane",
  );
  assert.equal(activePage(registry, target), undefined);
});

test("unknown commands and disabled extensions resolve to unavailable", async () => {
  const { resolveExtensionCommand } = await library;
  const { registry, snapshot } = await registryWith(fixture);
  assert.equal(
    resolveExtensionCommand(registry, snapshot, "test.probe", "probe.nope")
      .kind,
    "unavailable",
  );
  const off = await registryWith(fixture, "disabled");
  assert.equal(
    resolveExtensionCommand(
      off.registry,
      off.snapshot,
      "test.probe",
      "probe.open-ledger",
    ).kind,
    "unavailable",
    "a disabled extension exposes no surface",
  );
});

test("a command cannot reach another extension's surface", async () => {
  const { resolveExtensionCommand } = await library;
  const { registry, snapshot } = await registryWith(fixture);
  assert.equal(
    resolveExtensionCommand(
      registry,
      snapshot,
      "other.extension",
      "probe.open-ledger",
    ).kind,
    "unavailable",
    "commands are addressed as (extensionId, commandId), never by name alone",
  );
});

const placementFixture = (placement, host = "workspace.pane") => ({
  ...fixture,
  contributions: {
    surfaces: [
      {
        ...soloLedger(),
        allowedHosts: [...new Set([host, "workspace.pane"])],
        defaultHost: host,
      },
    ],
    navigation: [
      {
        ...fixture.contributions.navigation.find(
          (n) => n.id === "probe.nav-mode",
        ),
        allowedPlacements: [placement],
        defaultPlacement: placement,
      },
    ],
    actions: [
      fixture.contributions.actions.find((a) => a.id === "probe.act-start"),
    ],
    commands: [
      fixture.contributions.commands.find((c) => c.id === "probe.open-ledger"),
    ],
  },
});

test("every navigation placement in the contract is selectable", async () => {
  const { navigationFor } = await library;
  for (const placement of [
    "mode.primary",
    "sidebar.primary",
    "dashboard.navigation",
    "sessions.navigation",
    "skills.navigation",
  ]) {
    const { registry } = await registryWith(placementFixture(placement));
    assert.equal(
      navigationFor(registry, placement).length,
      1,
      `${placement} yields its contribution`,
    );
    for (const other of ["mode.primary", "sidebar.primary"].filter(
      (p) => p !== placement,
    ))
      assert.equal(
        navigationFor(registry, other).length,
        0,
        `${placement} does not leak into ${other}`,
      );
  }
});

test("every action placement in the contract is selectable", async () => {
  const { actionsFor } = await library;
  for (const placement of [
    "workspace.toolbar.start",
    "workspace.toolbar.before-tidy",
    "workspace.toolbar.after-tidy",
    "workspace.toolbar.end",
    "workspace.folder.actions",
  ]) {
    const { registry } = await registryWith({
      ...fixture,
      contributions: {
        ...fixture.contributions,
        actions: [
          {
            ...fixture.contributions.actions.find(
              (a) => a.id === "probe.act-start",
            ),
            allowedPlacements: [placement],
            defaultPlacement: placement,
          },
        ],
      },
    });
    assert.equal(actionsFor(registry, placement).length, 1, placement);
  }
});

test("every section host in the contract is selectable", async () => {
  const { surfacesFor } = await library;
  for (const host of [
    "dashboard.section",
    "sessions.section",
    "skills.section",
    "settings.section",
  ]) {
    const { registry } = await registryWith(
      placementFixture("sidebar.primary", host),
    );
    assert.equal(surfacesFor(registry, host).length, 1, host);
    assert.equal(
      surfacesFor(registry, "workspace.pane").length,
      0,
      `${host} is not also offered as a pane`,
    );
  }
});

test("navigation resolves by the surface host, not by the button placement", async () => {
  const { resolveNavigation } = await library;
  const cases = [
    ["app.page", "page"],
    ["workspace.pane", "pane"],
    ["workspace.tab", "tab"],
  ];
  for (const [host, expected] of cases) {
    const { registry } = await registryWith(
      placementFixture("sidebar.primary", host),
    );
    assert.equal(
      resolveNavigation(registry, {
        extensionId: "test.probe",
        targetSurfaceId: "probe.ledger",
      }).kind,
      expected,
      `${host} opens as ${expected}`,
    );
  }
  const off = await registryWith(fixture, "disabled");
  assert.equal(
    resolveNavigation(off.registry, {
      extensionId: "test.probe",
      targetSurfaceId: "probe.ledger",
    }).kind,
    "unavailable",
  );
});

const pane = (extensionId, contributionId, id = "p1") => ({
  id,
  kind: "extension",
  title: "Specimen",
  extension: {
    extensionId,
    contributionId,
    instanceId: id,
    stateVersion: 1,
  },
});

async function paneRegistry(instancePolicy) {
  const base = JSON.parse(JSON.stringify(fixture));
  const ledger = soloLedger();
  ledger.instancePolicy = instancePolicy;
  // Only instance state may be shared by several panes; the validator enforces
  // the pairing, so the fixture has to respect it.
  if (instancePolicy === "multiple") ledger.stateScope = "instance";
  ledger.allowedHosts = ["workspace.pane", "workspace.tab"];
  ledger.defaultHost = "workspace.pane";
  base.contributions.surfaces = [ledger];
  base.contributions.navigation = [];
  base.contributions.actions = [];
  base.contributions.commands = [];
  return registryWith(base);
}

test("a singleton surface is revealed instead of opened twice", async () => {
  const { resolvePaneOpen } = await library;
  const { registry } = await paneRegistry("singleton");
  const open = pane("test.probe", "probe.ledger");
  assert.deepEqual(
    resolvePaneOpen(registry, [], {
      extensionId: "test.probe",
      surfaceId: "probe.ledger",
    }),
    { kind: "add" },
  );
  const result = resolvePaneOpen(registry, [open], {
    extensionId: "test.probe",
    surfaceId: "probe.ledger",
  });
  assert.equal(result.kind, "show");
  assert.equal(result.panel.id, "p1");
});

test("a multi-instance surface still opens another pane", async () => {
  const { resolvePaneOpen } = await library;
  const { registry } = await paneRegistry("multiple");
  const open = pane("test.probe", "probe.ledger");
  assert.deepEqual(
    resolvePaneOpen(registry, [open], {
      extensionId: "test.probe",
      surfaceId: "probe.ledger",
    }),
    { kind: "add" },
  );
});

test("panes of other surfaces never satisfy a singleton", async () => {
  const { resolvePaneOpen } = await library;
  const { registry } = await paneRegistry("singleton");
  const other = pane("user.other", "probe.ledger", "p9");
  assert.deepEqual(
    resolvePaneOpen(registry, [other], {
      extensionId: "test.probe",
      surfaceId: "probe.ledger",
    }),
    { kind: "add" },
  );
});

test("a surface that is gone reports unavailable rather than adding a pane", async () => {
  const { resolvePaneOpen } = await library;
  const { registry } = await paneRegistry("singleton");
  const result = resolvePaneOpen(registry, [], {
    extensionId: "test.probe",
    surfaceId: "probe.missing",
  });
  assert.equal(result.kind, "unavailable");
});

const surfaceLibrary = library;
const SEP = String.fromCharCode(0);

test("a project is its path, unless it lives on another machine", async () => {
  const { projectScope } = await surfaceLibrary;
  const cwd = "/Users/me/app";
  assert.equal(projectScope(cwd), cwd);
  assert.equal(
    projectScope(cwd, "/Users/me/.config/herdr/herdr.sock"),
    cwd,
    "a local daemon socket is still this machine, not part of identity",
  );
  assert.equal(
    projectScope(cwd, "ssh:build-box"),
    `ssh:build-box${SEP}${cwd}`,
    "the same path on a remote host is a different project",
  );
  assert.equal(projectScope("", "ssh:build-box"), "", "no folder, no scope");
});
