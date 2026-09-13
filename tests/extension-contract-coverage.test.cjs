const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const {
  CONTRACT,
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");

const FIXTURE = "tests/fixtures/extensions/probe/manifest.json";
const raw = JSON.parse(fs.readFileSync(FIXTURE, "utf8"));
const probe = validateExtensionManifest({
  ...raw,
  source: { kind: "local", path: "/tmp/probe" },
});
const { surfaces, navigation, actions, commands } = probe.contributions;
// Only views[0] renders, so only views[0] counts as covered.
const views = surfaces.map((surface) => surface.view.document.views[0]);
const named = (icon) => (icon?.kind === "named" ? [icon.name] : []);
const collect = (items, pick) => new Set(items.flatMap(pick));

/** What the fixture demonstrably reaches, counted the way the app reads it:
 * a host is covered by being somebody's defaultHost, not by sitting in an
 * allowedHosts list nothing mounts from. */
const used = {
  // Every host but one is reached by being a surface's defaultHost.
  // workspace.pane is the exception: nothing declares it as a default, a
  // surface becomes pane-mountable by listing it, and page and tab win first
  // (routes.ts resolveNavigation), so listing it is not the same as reaching
  // it.
  HOSTS: collect(surfaces, (s) => [
    s.defaultHost,
    ...(s.allowedHosts.includes("workspace.pane") &&
    s.defaultHost !== "app.page" &&
    s.defaultHost !== "workspace.tab"
      ? ["workspace.pane"]
      : []),
  ]),
  PLACEMENTS: collect(navigation, (n) => [n.defaultPlacement]),
  ACTION_PLACEMENTS: collect(actions, (a) => [a.defaultPlacement]),
  INSTANCE_POLICIES: collect(surfaces, (s) => [s.instancePolicy]),
  STATE_SCOPES: collect(surfaces, (s) => [s.stateScope]),
  ICONS: collect([...surfaces, ...navigation, ...actions], (item) =>
    named(item.icon),
  ),
  FIELD_TYPES: collect(surfaces, (s) =>
    s.view.document.fields.map((field) => field.type),
  ),
  LAYOUTS: collect(views, (view) => [view.layout]),
  TONES: collect(surfaces, (s) =>
    s.view.document.fields.flatMap((field) =>
      (field.options || []).map((option) => option.tone),
    ),
  ),
  FILTER_OPS: collect(views, (view) => (view.filter || []).map((r) => r.op)),
  SORT_DIRS: collect(views, (view) => (view.sort || []).map((r) => r.dir)),
  PSEUDO_FIELDS: collect(views, (view) => [
    ...(view.meta || []),
    ...(view.groupable || []),
  ]),
  TOKEN_ACCENT: collect(surfaces, (s) => [s.tokens?.accent]),
  TOKEN_DENSITY: collect(surfaces, (s) => [s.tokens?.density]),
  TOKEN_RADIUS: collect(surfaces, (s) => [s.tokens?.radius]),
  TOKEN_ELEVATION: collect(surfaces, (s) => [s.tokens?.elevation]),
};

test("the probe fixture exercises every value the contract allows", () => {
  assert.deepEqual(
    Object.keys(CONTRACT).filter((key) => !(key in used)),
    [],
    "a new closed set was added to CONTRACT; count it here or the coverage claim is stale",
  );
  for (const [key, allowed] of Object.entries(CONTRACT)) {
    const missing = [...allowed].filter((value) => !used[key].has(value));
    assert.deepEqual(
      missing,
      [],
      `${key}: ${missing.join(", ")} appears nowhere in ${FIXTURE}. Widening the contract means widening the fixture.`,
    );
  }
});

test("tones reach the summary tiles as well as the select options", () => {
  // Two render paths: a chip coloured from its option, and a tile coloured by
  // the manifest. One covering the other would hide a regression in either.
  const tiles = collect(views, (view) =>
    (view.summary || []).map((tile) => tile.tone),
  );
  assert.deepEqual(
    [...CONTRACT.TONES].filter((t) => !tiles.has(t)),
    [],
  );
});

test("the fixture stands on every default rather than restating it", () => {
  const surface = (id) => raw.contributions.surfaces.find((s) => s.id === id);
  const view = (id) => surface(id).view.views?.[0];
  assert.equal(surface("probe.settings").stateScope, undefined);
  assert.equal(surface("probe.settings").instancePolicy, undefined);
  assert.equal(surface("probe.notes").stateVersion, undefined);
  assert.equal(surface("probe.table").icon, undefined);
  assert.equal(view("probe.notes").layout, undefined);
  assert.equal(view("probe.notes-board").filter[0].op, undefined);
  assert.equal(view("probe.ledger").sort[2].dir, undefined);
  assert.equal(
    raw.contributions.navigation.find((n) => n.id === "probe.nav-sidebar-board")
      .order,
    undefined,
  );
  assert.ok(
    surface("probe.ledger")
      .view.data.fields.find((f) => f.id === "shape")
      .options.some((option) => option.tone === undefined),
  );
  // And the validator fills each one in.
  const filled = (id) => surfaces.find((s) => s.id === id);
  assert.equal(filled("probe.settings").stateScope, "instance");
  assert.equal(filled("probe.settings").instancePolicy, "singleton");
  assert.equal(filled("probe.notes").stateVersion, 1);
  assert.equal(views[surfaces.indexOf(filled("probe.notes"))].layout, "list");
});

test("order is clamped, not trusted", () => {
  const late = navigation.find((n) => n.id === "probe.nav-sidebar-ledger");
  assert.equal(late.order, 1000, "2000 in the manifest clamps to the cap");
  assert.equal(
    navigation.find((n) => n.id === "probe.nav-sidebar-board").order,
    0,
  );
});

test("the combinations the contract permits are each present once", () => {
  // Value coverage is not pair coverage: these are the pairings the validator
  // allows, and each has its own branch to get wrong.
  const has = (predicate, what) =>
    assert.ok(surfaces.some(predicate) || views.some(predicate), what);
  has(
    (s) => s.instancePolicy === "multiple" && s.stateScope === "instance",
    "several panes, state per pane",
  );
  has(
    (s) => s.stateId !== s.id && s.stateScope === "project",
    "a borrower at project scope",
  );
  has(
    (s) => s.stateId !== s.id && s.stateScope === "global",
    "a borrower at global scope",
  );
  has((s) => s.aggregate && s.defaultHost.endsWith(".page"), "aggregate page");
  has(
    (s) =>
      s.view.document.views[0].layout === "board" &&
      s.view.document.views[0].columns,
    "a board with fixed columns",
  );
  has(
    (s) =>
      s.view.document.views[0].layout === "board" &&
      !s.view.document.views[0].columns &&
      s.view.document.views[0].groupable?.length,
    "a board that only groups",
  );
  has((s) => s.defaultHost === "workspace.tab", "the tab host");
  assert.ok(
    raw.contributions.surfaces.some((s) => s.view.schemaVersion === 1),
    "the v1 collection schema still has a reader",
  );
  assert.ok(
    [...surfaces, ...navigation].some((item) => item.icon?.kind === "svg"),
    "an extension shipping its own geometry",
  );
});

test("the types the app compiles against list the same values", () => {
  // Two copies of one closed set drift silently: the validator accepts a value
  // the renderer has no branch for, and nothing says so until it is on screen.
  const types = fs.readFileSync("src/extensions/types.ts", "utf8");
  const union = (name) => {
    const at = types.indexOf(`export type ${name} =`);
    assert.notEqual(at, -1, `${name} is gone from types.ts`);
    return new Set(
      [...types.slice(at, types.indexOf(";", at)).matchAll(/"([^"]+)"/g)].map(
        (match) => match[1],
      ),
    );
  };
  for (const [name, allowed] of [
    ["NavigationPlacement", CONTRACT.PLACEMENTS],
    ["WorkspaceActionPlacement", CONTRACT.ACTION_PLACEMENTS],
    ["FieldType", CONTRACT.FIELD_TYPES],
    ["Tone", CONTRACT.TONES],
  ])
    assert.deepEqual(
      [...union(name)].sort(),
      [...allowed].sort(),
      `${name} and the validator disagree about what is allowed`,
    );
});

test("the renderer knows every icon the manifest may name", () => {
  // The map lives in a .tsx the test runner cannot import, and a name missing
  // from it falls back silently rather than failing.
  const source = fs.readFileSync("src/extensions/ExtensionSlots.tsx", "utf8");
  const map = source.slice(
    source.indexOf("const ICONS = {"),
    source.indexOf("} as const;"),
  );
  const missing = [...CONTRACT.ICONS].filter(
    (name) => !map.includes(`"${name}":`) && !map.includes(`\n  ${name}:`),
  );
  assert.deepEqual(missing, [], "these icon names render as the fallback");
});
