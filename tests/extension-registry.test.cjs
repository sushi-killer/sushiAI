const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const library = import("../src/extensions/registry.ts");

const fixture = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "fixtures/extensions/probe/manifest.json"),
    "utf8",
  ),
);

const snapshot = (overrides = {}) => {
  const extension = {
    ...structuredClone(fixture),
    source: { kind: "npm", package: "@example/probe", version: "0.1.0" },
  };
  const surface = {
    ...structuredClone(
      fixture.contributions.surfaces.find((s) => s.id === "probe.tab"),
    ),
    extensionId: extension.id,
  };
  return {
    schemaVersion: 2,
    version: 1,
    extensions: [{ manifest: extension, status: "active" }],
    surfaces: [surface],
    navigation: [],
    actions: [],
    commands: [],
    ...overrides,
  };
};

test("registry accepts sanitized snapshots and creates a stable extension panel reference", async () => {
  const { ExtensionRegistry } = await library;
  const registry = new ExtensionRegistry();
  registry.applySnapshot(snapshot());
  assert.equal(registry.isExtensionActive("test.probe"), true);
  assert.equal(registry.availableSurfaces()[0].id, "probe.tab");
  const panel = registry.createPanel("test.probe", "probe.tab", "panel-1");
  assert.deepEqual(panel.extension, {
    extensionId: "test.probe",
    contributionId: "probe.tab",
    instanceId: "panel-1",
    stateVersion: 1,
  });
});

test("registry keeps disabled surfaces for unavailable placeholders", async () => {
  const { ExtensionRegistry } = await library;
  const registry = new ExtensionRegistry();
  registry.applySnapshot(
    snapshot({
      extensions: [
        { manifest: snapshot().extensions[0].manifest, status: "disabled" },
      ],
    }),
  );
  assert.equal(registry.isExtensionActive("test.probe"), false);
  assert.equal(
    registry.resolveSurface({
      id: "panel-1",
      kind: "extension",
      title: "Specimen tab",
      extension: {
        extensionId: "test.probe",
        contributionId: "probe.tab",
        instanceId: "panel-1",
        stateVersion: 1,
      },
    }).id,
    "probe.tab",
  );
  assert.throws(
    () => registry.createPanel("test.probe", "probe.tab", "panel-1"),
    /Unavailable/,
  );
});

test("registry rejects duplicate contributions and unknown owners", async () => {
  const { ExtensionRegistry } = await library;
  const registry = new ExtensionRegistry();
  assert.throws(
    () =>
      registry.applySnapshot(
        snapshot({
          surfaces: [snapshot().surfaces[0], snapshot().surfaces[0]],
        }),
      ),
    /Duplicate surface/,
  );
  assert.throws(
    () =>
      registry.applySnapshot(
        snapshot({
          surfaces: [{ ...snapshot().surfaces[0], extensionId: "missing" }],
        }),
      ),
    /unknown extension/,
  );
});

test("registry permits the same local contribution id in different extensions", async () => {
  const { ExtensionRegistry } = await library;
  const registry = new ExtensionRegistry();
  const base = snapshot();
  const second = {
    ...base.extensions[0],
    manifest: { ...structuredClone(fixture), id: "test.other" },
  };
  registry.applySnapshot({
    ...base,
    extensions: [base.extensions[0], second],
    surfaces: [
      base.surfaces[0],
      { ...base.surfaces[0], extensionId: "test.other" },
    ],
  });
  assert.equal(registry.availableSurfaces().length, 2);
});

test("equal order breaks the same way for every placement", async () => {
  const { ExtensionRegistry } = await library;
  const registry = new ExtensionRegistry();
  const base = snapshot().extensions[0].manifest;
  const other = { ...structuredClone(base), id: "test.other" };
  const entry = (extensionId, id, order) => ({
    id,
    extensionId,
    targetSurfaceId: "probe.tab",
    allowedPlacements: ["sidebar.primary"],
    defaultPlacement: "sidebar.primary",
    label: "Same label everywhere",
    icon: { kind: "named", name: "plug" },
    order,
  });
  registry.applySnapshot(
    snapshot({
      extensions: [
        { manifest: base, status: "active" },
        { manifest: other, status: "active" },
      ],
      navigation: [
        entry("test.probe", "b", 0),
        entry("test.other", "a", 0),
        entry("test.probe", "a", 0),
        entry("test.probe", "z", -1),
      ],
    }),
  );
  // One rule, not one per host: order, then whose it is, then which of theirs.
  // Sharing a label used to decide it in the sidebar and the id elsewhere, so
  // the same two entries came out in a different order in each place.
  assert.deepEqual(
    registry
      .availableNavigation()
      .map((item) => `${item.extensionId}:${item.id}`),
    ["test.probe:z", "test.other:a", "test.probe:a", "test.probe:b"],
  );
});
