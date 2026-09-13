const { test } = require("node:test");
const assert = require("node:assert/strict");

const fs = require("node:fs");
const path = require("node:path");
const library = require("../electron/extensions/manifest.cjs");

const fixture = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "fixtures/extensions/probe/manifest.json"),
    "utf8",
  ),
);

const manifest = (overrides = {}) => {
  const base = {
    ...structuredClone(fixture),
    source: { kind: "npm", package: "@example/probe", version: "1.2.3" },
  };
  return {
    ...base,
    ...overrides,
    contributions: {
      ...base.contributions,
      ...(overrides.contributions || {}),
    },
  };
};

test("extension manifest accepts app-wide npm packages and surface hosts", async () => {
  const { validateExtensionManifest, extensionSourceLabel } = library;
  const result = validateExtensionManifest(manifest());
  assert.equal(result.scope, "app");
  assert.equal(result.source.kind, "npm");
  assert.deepEqual(result.contributions.surfaces[0].allowedHosts, [
    "app.page",
    "workspace.pane",
  ]);
  assert.equal(extensionSourceLabel(result.source), "npm:@example/probe@1.2.3");
});

test("extension manifest rejects project scope, unsupported API and invalid placement", async () => {
  const { validateExtensionManifest } = library;
  assert.throws(
    () => validateExtensionManifest(manifest({ scope: "project" })),
    /Only app-wide extensions/,
  );
  assert.throws(
    () => validateExtensionManifest(manifest({ apiVersion: 2 })),
    /Unsupported extension API version/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        manifest({
          contributions: {
            surfaces: [
              {
                ...manifest().contributions.surfaces[0],
                defaultHost: "workspace.tab",
              },
            ],
          },
        }),
      ),
    /not in its own allowedHosts/,
  );
});

test("extension manifest rejects floating Git refs and accepts resolved commits", async () => {
  const { validateExtensionSource } = library;
  assert.throws(
    () =>
      validateExtensionSource({
        kind: "git",
        url: "https://example.test/probe.git",
        requestedRef: "main",
        resolvedCommit: "0123456789abcdef",
      }),
    /tag or commit/,
  );
  const source = validateExtensionSource({
    kind: "git",
    url: "https://example.test/probe.git",
    requestedRef: "v1.2.3",
    resolvedCommit: "0123456789abcdef",
  });
  assert.equal(source.resolvedCommit, "0123456789abcdef");
});

test("external extensions cannot request core views or dangling contributions", async () => {
  const { validateExtensionManifest } = library;
  assert.throws(
    () =>
      validateExtensionManifest(
        manifest({
          contributions: {
            surfaces: [
              {
                ...manifest().contributions.surfaces[0],
                view: { kind: "core", viewId: "workspace" },
              },
            ],
          },
        }),
      ),
    /Only built-in extensions may request core views/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        manifest({
          contributions: {
            navigation: [
              {
                ...manifest().contributions.navigation[0],
                targetSurfaceId: "missing",
              },
            ],
          },
        }),
      ),
    /unknown extension surface/,
  );
});

test("the probe fixture passes the manifest validator end to end", () => {
  const { validateExtensionManifest } = library;
  // The folder scanner stamps the source; the fixture on disk never claims one.
  const result = validateExtensionManifest(manifest());
  const surfaceIds = new Set(
    result.contributions.surfaces.map((surface) => surface.id),
  );
  assert.ok(surfaceIds.size, "fixture contributes at least one surface");
  for (const item of result.contributions.navigation)
    assert.ok(
      surfaceIds.has(item.targetSurfaceId),
      `navigation ${item.id} targets a known surface`,
    );
  for (const command of result.contributions.commands)
    assert.ok(
      !command.surfaceId || surfaceIds.has(command.surfaceId),
      `command ${command.id} targets a known surface`,
    );
  for (const contribution of [
    ...result.contributions.surfaces,
    ...result.contributions.navigation,
    ...result.contributions.actions,
    ...result.contributions.commands,
  ])
    assert.equal(
      contribution.extensionId,
      result.id,
      "every contribution is namespaced to its extension",
    );
});

const withNavIcon = (icon) => {
  const base = manifest();
  base.contributions.navigation[0].icon = icon;
  return base;
};

test("an extension may ship its own icon as plain path geometry", () => {
  const { validateExtensionManifest } = library;
  const result = validateExtensionManifest(
    withNavIcon({
      kind: "svg",
      viewBox: " 0 0 24 24 ",
      paths: ["M4 7h10", { d: "M17 8l2 2 4-4", fill: "currentColor" }],
    }),
  );
  assert.deepEqual(result.contributions.navigation[0].icon, {
    kind: "svg",
    viewBox: "0 0 24 24",
    paths: [
      { d: "M4 7h10", fill: "none" },
      { d: "M17 8l2 2 4-4", fill: "currentColor" },
    ],
  });
});

test("a named icon must be one the app actually bundles", () => {
  const { validateExtensionManifest } = library;
  assert.equal(
    validateExtensionManifest(withNavIcon("list-check")).contributions
      .navigation[0].icon.name,
    "list-check",
  );
  assert.throws(
    () => validateExtensionManifest(withNavIcon("skull")),
    /is not a bundled icon/,
  );
});

test("an icon cannot smuggle markup, script or a remote reference", () => {
  const { validateExtensionManifest } = library;
  const refused = [
    {
      kind: "svg",
      viewBox: "0 0 24 24",
      paths: ["</path><script>x()</script>"],
    },
    { kind: "svg", viewBox: "0 0 24 24", paths: ["url(http://evil/x.png)"] },
    { kind: "svg", viewBox: "0 0 24 24", paths: ['M0 0" onload="alert(1)'] },
    { kind: "svg", viewBox: "0 0 24 24", paths: ["M4 7h10;expression(1)"] },
  ];
  for (const icon of refused)
    assert.throws(
      () => validateExtensionManifest(withNavIcon(icon)),
      /plain path data only/,
      `refused: ${JSON.stringify(icon.paths)}`,
    );
});

test("an icon cannot be unbounded or malformed", () => {
  const { validateExtensionManifest } = library;
  assert.throws(
    () =>
      validateExtensionManifest(
        withNavIcon({ kind: "svg", viewBox: "0 0 24", paths: ["M4 7h10"] }),
      ),
    /four numbers/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        withNavIcon({ kind: "svg", viewBox: "0 0 24 24", paths: [] }),
      ),
    /1-12 paths/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        withNavIcon({
          kind: "svg",
          viewBox: "0 0 24 24",
          paths: Array.from({ length: 13 }, () => "M4 7h10"),
        }),
      ),
    /1-12 paths/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        withNavIcon({
          kind: "svg",
          viewBox: "0 0 24 24",
          paths: [`M4 ${"7".repeat(5000)}`],
        }),
      ),
    /plain path data only/,
  );
  assert.throws(
    () => validateExtensionManifest(withNavIcon({ kind: "image", url: "x" })),
    /Unsupported extension navigation.icon/,
  );
});

test("a surface state scope defaults, but is never guessed", () => {
  const { validateExtensionManifest } = library;
  const settings = structuredClone(
    fixture.contributions.surfaces.find((s) => s.id === "probe.settings"),
  );
  const base = manifest({
    contributions: {
      surfaces: [settings],
      navigation: [],
      actions: [],
      commands: [],
    },
  });
  assert.equal(
    validateExtensionManifest(base).contributions.surfaces[0].stateScope,
    "instance",
    "omitted means per pane",
  );
  assert.throws(
    () =>
      validateExtensionManifest({
        ...base,
        contributions: {
          ...base.contributions,
          surfaces: [
            {
              ...base.contributions.surfaces[0],
              stateScope: "project",
              instancePolicy: "multiple",
            },
          ],
        },
      }),
    /must be "instancePolicy": "singleton"/,
    "several panes cannot share one slice of project state",
  );
  base.contributions.surfaces[0].instancePolicy = "singleton";
  base.contributions.surfaces[0].stateScope = "project";
  assert.equal(
    validateExtensionManifest(base).contributions.surfaces[0].stateScope,
    "project",
  );
  // Coercing an unrecognised value is how a typo silently stores a person's
  // tasks in the wrong place, so it is refused and the message says what is
  // allowed and which surface is at fault.
  for (const [field, value] of [
    ["stateScope", "Project"],
    ["stateScope", "everything"],
    ["instancePolicy", "many"],
  ]) {
    const broken = JSON.parse(JSON.stringify(base));
    // The singleton pairing rule would fire first and mask what is being
    // tested here, which is that the value itself is refused.
    broken.contributions.surfaces[0].instancePolicy = "singleton";
    broken.contributions.surfaces[0][field] = value;
    assert.throws(
      () => validateExtensionManifest(broken),
      (error) =>
        error.message.includes(`surfaces[probe.settings].${field}`) &&
        error.message.includes(JSON.stringify(value)) &&
        error.message.includes("Use one of:"),
      `${field}: ${value}`,
    );
  }
  const badVersion = JSON.parse(JSON.stringify(base));
  badVersion.contributions.surfaces[0].instancePolicy = "singleton";
  badVersion.contributions.surfaces[0].stateVersion = 1.5;
  assert.throws(
    () => validateExtensionManifest(badVersion),
    /stateVersion must be a whole number/,
  );
});

test("an error names the surface it came from", () => {
  const { validateExtensionManifest } = library;
  const base = manifest();
  base.contributions.surfaces.push({
    ...base.contributions.surfaces[0],
    id: "probe.second",
    allowedHosts: ["workspace.pane", "nonsense"],
  });
  assert.throws(
    () => validateExtensionManifest(base),
    (error) =>
      error.message.includes("surfaces[probe.second].allowedHosts") &&
      error.message.includes('"nonsense"') &&
      error.message.includes("workspace.pane"),
    "with two surfaces you must be able to tell which one failed",
  );
});

/** The two surfaces a cross-project overview needs: one people edit, one that
 * reads every project's copy of it. */
const withOverview = (changes = {}, view = {}) => {
  const owner = structuredClone(
    fixture.contributions.surfaces.find((s) => s.id === "probe.ledger"),
  );
  // Its view puts commands on the page that this minimal manifest does not
  // declare; irrelevant to the stateId rules under test here.
  delete owner.view.views[0].actions;
  const base = manifest({
    contributions: {
      surfaces: [owner],
      navigation: [],
      actions: [],
      commands: [],
    },
  });
  base.contributions.surfaces.push({
    id: "probe.overview",
    title: "Specimens",
    allowedHosts: ["app.page"],
    defaultHost: "app.page",
    instancePolicy: "singleton",
    stateId: "probe.ledger",
    stateVersion: 2,
    stateScope: "project",
    aggregate: true,
    view: {
      kind: "declarative",
      schemaVersion: 2,
      title: "Specimens",
      itemLabel: "specimen",
      data: {
        fields: [
          { id: "name", type: "text", label: "Specimen" },
          { id: "catalogued", type: "boolean", label: "Catalogued" },
        ],
      },
      views: [
        {
          layout: "board",
          primary: "name",
          toggle: "catalogued",
          meta: [],
          groupable: ["$project"],
          defaultGroup: "$project",
        },
      ],
      ...view,
    },
    ...changes,
  });
  return base;
};

test("an aggregate surface is a read-only page over every project", () => {
  const { validateExtensionManifest } = library;
  const ok =
    validateExtensionManifest(withOverview()).contributions.surfaces[1];
  assert.equal(ok.aggregate, true);
  assert.equal(ok.stateId, "probe.ledger");
  assert.deepEqual(ok.view.document.views[0].groupable, ["$project"]);
  // Records belong to one project each; an overview must not be able to add or
  // toggle without knowing which project it would be writing to.
  assert.equal(ok.view.document.allowAdd, false);
  assert.equal(ok.view.document.allowToggle, false);

  assert.throws(
    () => validateExtensionManifest(withOverview({ stateScope: "global" })),
    /there is nothing to aggregate otherwise/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        withOverview({
          allowedHosts: ["workspace.pane"],
          defaultHost: "workspace.pane",
        }),
      ),
    /not in a workspace pane/,
  );
  assert.throws(
    () => validateExtensionManifest(withOverview({ aggregate: false })),
    /\$project is only available on an aggregate surface/,
  );
  // Aggregating its own slice can only ever show an empty page: the validator
  // forces it read-only, so nothing writes there.
  assert.throws(
    () =>
      validateExtensionManifest(withOverview({ stateId: "probe.overview" })),
    /the page would always be empty/,
  );
});

test("a borrowed slice must be real, matching and read-only", () => {
  const { validateExtensionManifest } = library;
  const surfaces =
    validateExtensionManifest(withOverview()).contributions.surfaces;
  assert.equal(
    surfaces[0].stateId,
    "probe.ledger",
    "a surface owns its own id",
  );
  assert.equal(surfaces[1].stateId, "probe.ledger");

  assert.throws(
    () => validateExtensionManifest(withOverview({ stateId: "probe.nowhere" })),
    /is not a surface in this extension/,
  );
  assert.throws(
    () => validateExtensionManifest(withOverview({ stateVersion: 3 })),
    /keeps version 2 state, not 3/,
  );
  assert.throws(
    () =>
      validateExtensionManifest(
        withOverview(
          { aggregate: false, stateScope: "instance" },
          {
            views: [
              {
                layout: "list",
                primary: "name",
                toggle: "catalogued",
                meta: [],
              },
            ],
          },
        ),
      ),
    /instance state is keyed per pane/,
  );
  // The reason the rule exists: two editable views of one slice each write
  // their whole list, so the slower one's edit vanishes.
  assert.throws(
    () =>
      validateExtensionManifest(
        withOverview(
          { aggregate: false },
          {
            views: [
              {
                layout: "list",
                primary: "name",
                toggle: "catalogued",
                meta: [],
              },
            ],
          },
        ),
      ),
    /so it must not write them/,
  );
  const readOnly = withOverview(
    { aggregate: false },
    {
      allowAdd: false,
      allowToggle: false,
      views: [
        { layout: "list", primary: "name", toggle: "catalogued", meta: [] },
      ],
    },
  );
  assert.equal(
    validateExtensionManifest(readOnly).contributions.surfaces[1].stateId,
    "probe.ledger",
    "a read-only second view of one list is allowed",
  );
  const stray = withOverview(
    { aggregate: false },
    {
      allowAdd: false,
      allowToggle: false,
      data: {
        fields: [
          { id: "name", type: "text", label: "Specimen" },
          { id: "catalogued", type: "boolean", label: "Catalogued" },
          { id: "owner", type: "text", label: "Owner" },
        ],
      },
      views: [
        {
          layout: "list",
          primary: "name",
          toggle: "catalogued",
          meta: ["owner"],
        },
      ],
    },
  );
  assert.throws(() => validateExtensionManifest(stray), /has no "owner" field/);
});
