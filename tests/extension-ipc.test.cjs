const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const { registerExtensionIpc } = require("../electron/ipc/extensions.cjs");
const {
  validateExtensionManifest,
} = require("../electron/extensions/manifest.cjs");

const fixture = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, "fixtures/extensions/probe/manifest.json"),
    "utf8",
  ),
);

const findSurface = (id) =>
  structuredClone(
    fixture.contributions.surfaces.find((surface) => surface.id === id),
  );

// The surface people edit, and the aggregate page that reads every project's
// copy of it - the fixture already models exactly this relationship.
const board = findSurface("probe.ledger");
// Its view puts commands on the page that this manifest does not declare;
// irrelevant to state addressing, which is all these tests check.
delete board.view.views[0].actions;
const overview = findSurface("probe.board");

/** The IPC layer with a manifest behind it and a state store that only records
 * what it was asked to do, so the assertions are about the checks, not storage. */
function wire({ surfaces = [board, overview], enabled = true } = {}) {
  const manifest = validateExtensionManifest({
    id: "user.specimen",
    name: "Specimens",
    version: "1.0.0",
    apiVersion: 1,
    source: { kind: "npm", package: "@example/specimen", version: "1.0.0" },
    contributions: {
      surfaces,
      navigation: [],
      actions: [],
      commands: [],
    },
  });
  const find = (extensionId, surfaceId) =>
    extensionId === "user.specimen" && enabled
      ? manifest.contributions.surfaces.find((s) => s.id === surfaceId)
      : undefined;
  const calls = [];
  const handlers = new Map();
  registerExtensionIpc({
    handle: (channel, fn) => handlers.set(channel, fn),
    getExtensions: () => ({
      activeSurface: find,
      aggregatesOver: (extensionId, stateId) =>
        enabled &&
        manifest.contributions.surfaces.some(
          (s) => s.aggregate && s.stateId === stateId,
        ),
    }),
    getSurfaceState: () => ({
      read: (...args) => (calls.push(["read", ...args]), null),
      write: (...args) => (calls.push(["write", ...args]), undefined),
      aggregate: (...args) => (calls.push(["aggregate", ...args]), []),
    }),
    announce: (change) => calls.push(["announce", change]),
  });
  const call = (channel, ...args) => handlers.get(channel)(...args);
  return { call, calls };
}

const write = (call, value, scope = "/a") =>
  call(
    "extensions-state-write",
    "user.specimen",
    "probe.ledger",
    2,
    scope,
    value,
  );

test("state is addressed against the manifest, not against what was sent", async () => {
  const { call, calls } = wire();
  await write(call, [{ id: "1", name: "One", catalogued: true }]);
  assert.deepEqual(calls[0].slice(0, 5), [
    "write",
    "user.specimen",
    "probe.ledger",
    2,
    "/a",
  ]);
  assert.deepEqual(calls[1], [
    "announce",
    {
      extensionId: "user.specimen",
      surfaceId: "probe.ledger",
      version: 2,
      scope: "/a",
    },
  ]);

  await assert.rejects(
    async () =>
      call("extensions-state-read", "user.specimen", "probe.nope", 2, "/a"),
    /No active extension surface/,
  );
  await assert.rejects(
    async () =>
      call("extensions-state-read", "user.specimen", "probe.ledger", 3, "/a"),
    /keeps version 2 state/,
  );
  await assert.rejects(
    async () => write(call, [{ id: "1" }], "global"),
    /does not keep global state/,
  );
  const off = wire({ enabled: false });
  await assert.rejects(
    async () =>
      off.call(
        "extensions-state-read",
        "user.specimen",
        "probe.ledger",
        2,
        "/a",
      ),
    /No active extension surface/,
    "a disabled extension cannot read its own state",
  );
});

test("records are checked against the fields the manifest declared", async () => {
  const { call } = wire();
  const refused = [
    [[{ id: "1", nope: "x" }], /nope is not a field/],
    [[{ id: "1", catalogued: "yes" }], /catalogued is true or false/],
    [[{ id: "1", name: 4 }], /name is text/],
    [
      [{ id: "1", collected: "12.09.2026" }],
      /collected is a date as YYYY-MM-DD/,
    ],
    [
      [{ id: "1", tone: "later" }],
      /tone must be one of neutral, info, ok, warning, danger, muted/,
    ],
    [[{ id: "1" }, { id: "1" }], /appears twice/],
    [[{ id: "" }], /needs an id/],
    [[{}], /needs an id/],
    ["not a list", /stores a list of records/],
    [[null], /stores objects/],
    [
      Array.from({ length: 1001 }, (_, i) => ({ id: String(i) })),
      /at most 1000/,
    ],
  ];
  for (const [value, message] of refused)
    await assert.rejects(
      async () => write(call, value),
      message,
      String(message),
    );

  // Clearing a slice is how a surface forgets a project, not a malformed write.
  await call(
    "extensions-state-write",
    "user.specimen",
    "probe.ledger",
    2,
    "/a",
    null,
  );
});

test("a borrowed slice is validated against the surface that owns it", async () => {
  const { call, calls } = wire();
  // The renderer addresses by stateId, so the overview's reads land on the
  // ledger's slice and are checked against the ledger's fields.
  await call("extensions-state-aggregate", "user.specimen", "probe.ledger", 2);
  assert.deepEqual(calls[0], ["aggregate", "user.specimen", "probe.ledger", 2]);

  await assert.rejects(
    async () =>
      call("extensions-state-aggregate", "user.specimen", "probe.ledger", 3),
    /keeps version 2 state/,
  );

  // Reading every project at once opens only where an aggregate was declared.
  const alone = wire({ surfaces: [board] });
  await assert.rejects(
    async () =>
      alone.call(
        "extensions-state-aggregate",
        "user.specimen",
        "probe.ledger",
        2,
      ),
    /No surface aggregates/,
  );

  // The IPC guard against aggregating per-pane state is the second line; the
  // manifest never gets far enough to ask.
  assert.throws(
    () =>
      wire({
        surfaces: [
          { ...board, stateScope: "instance", instancePolicy: "multiple" },
          { ...overview, stateScope: "instance" },
        ],
      }),
    /aggregate needs "stateScope": "project"/,
  );
});
