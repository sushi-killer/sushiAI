const { test } = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const React = require("react");
const esbuild = require("esbuild");
const { renderToStaticMarkup } = require("react-dom/server");

const src = (file) => path.resolve(__dirname, "../src", file);

// JSX sources cannot be type-stripped, so the renderer is bundled in memory.
// The core view table is replaced by a stub that records what a view receives.
const seen = { props: null };
globalThis.__coreViewProbe = seen;
async function load(entry) {
  const result = await esbuild.build({
    entryPoints: [src(entry)],
    bundle: true,
    write: false,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    packages: "external",
    loader: { ".css": "empty" },
    plugins: [
      {
        name: "stub-core-views",
        setup(build) {
          build.onResolve({ filter: /coreViews\.ts$/ }, () => ({
            path: "core-views-stub",
            namespace: "stub",
          }));
          build.onLoad({ filter: /.*/, namespace: "stub" }, () => ({
            contents: `module.exports = { coreViews: { "probe.view": { View: (props) => { globalThis.__coreViewProbe.props = props; return null; } } } };`,
            loader: "js",
            resolveDir: __dirname,
          }));
        },
      },
    ],
  });
  const module = { exports: {} };
  new Function("module", "exports", "require", result.outputFiles[0].text)(
    module,
    module.exports,
    require,
  );
  return module.exports;
}

const decode = (path, size, quiet) => {
  const grid = Array.from({ length: size }, () => Array(size).fill(false));
  for (const match of path.matchAll(/M(\d+) (\d+)h(\d+)v1h-\d+z/g)) {
    const [x, y, run] = match.slice(1).map(Number);
    for (let i = 0; i < run; i += 1) grid[y - quiet][x - quiet + i] = true;
  }
  return grid;
};

test("QR matrix survives the SVG path round trip and has valid finder patterns", async () => {
  const { qrMatrix, qrPath, QR_QUIET } =
    await import("../src/extensions/qrMatrix.ts");
  for (const text of [
    "A",
    "https://example.test/pair?code=ABC-123",
    "Café ☕ 日本",
  ]) {
    const matrix = qrMatrix(text);
    assert.ok(matrix, text);
    assert.equal(matrix.length, matrix[0].length);
    assert.deepEqual(decode(qrPath(matrix), matrix.length, QR_QUIET), matrix);
    // Three 7x7 finder squares: dark ring, light ring, dark 3x3 core.
    const n = matrix.length;
    for (const [r, c] of [
      [0, 0],
      [0, n - 7],
      [n - 7, 0],
    ]) {
      for (let i = 0; i < 7; i += 1)
        for (let j = 0; j < 7; j += 1) {
          const ring = Math.max(Math.abs(i - 3), Math.abs(j - 3));
          assert.equal(matrix[r + i][c + j], ring !== 2, `${text} ${r},${c}`);
        }
    }
  }
  const a = qrMatrix("pairing-one");
  const b = qrMatrix("pairing-two");
  assert.notDeepEqual(a, b);
  assert.equal(qrMatrix("x".repeat(3000)), null);
});

test("QR bytes are UTF-8 so a scanner reads back the original text", async () => {
  const { qrMatrix } = await import("../src/extensions/qrMatrix.ts");
  const qrcode = (await import("qrcode-generator")).default;
  assert.deepEqual(qrcode.stringToBytes("é"), [0xc3, 0xa9]);
  assert.ok(qrMatrix("é"));
});

const view = {
  kind: "companion",
  fields: [
    { id: "service", label: "Service", type: "status" },
    { id: "code", label: "Pairing code", type: "qr" },
    { id: "note", label: "Note", type: "text" },
    { id: "gone", label: "Empty", type: "text" },
  ],
  actions: [
    { id: "add", label: "Add device", method: "device.add" },
    { id: "sync", label: "Sync", method: "sync.run" },
  ],
};

test("a companion panel draws status tone, a QR SVG, text and buttons", async () => {
  const { CompanionPanel } = await load("extensions/CompanionView.tsx");
  const { INITIAL_COMPANION_STATE } =
    await import("../src/extensions/companionModel.ts");
  const html = renderToStaticMarkup(
    React.createElement(CompanionPanel, {
      view,
      onAction() {},
      state: {
        ...INITIAL_COMPANION_STATE,
        loaded: true,
        values: {
          service: { text: "Connected", tone: "ok" },
          code: "pair:EXAMPLE-1",
          note: "Hello <b>there</b>",
          gone: null,
        },
      },
    }),
  );
  assert.match(html, /class="companion-status" data-tone="ok">Connected</);
  assert.match(html, /<svg class="companion-qr"[^>]*aria-label="Pairing code"/);
  assert.match(html, /<path d="M\d+ \d+h\d+v1h-\d+z/);
  assert.match(html, /Hello &lt;b&gt;there&lt;\/b&gt;/);
  // A field without a value is left out, not drawn as a dash.
  assert.doesNotMatch(html, /companion-empty/);
  assert.equal((html.match(/companion-field/g) || []).length, 3);
  assert.match(html, /<button class="primary"[^>]*>Add device</);
  assert.match(html, /<button class="secondary"/);
  assert.doesNotMatch(html, /disabled/);

  const busy = renderToStaticMarkup(
    React.createElement(CompanionPanel, {
      view,
      onAction() {},
      state: { ...INITIAL_COMPANION_STATE, busy: "add" },
    }),
  );
  assert.match(
    busy,
    /aria-busy="true"[^>]*disabled|disabled[^>]*aria-busy="true"/,
  );
  assert.match(busy, /Working/);
  assert.equal((busy.match(/disabled/g) || []).length, 2);
});

function fakeBridge() {
  const calls = [];
  const listeners = new Set();
  const pending = [];
  return {
    calls,
    pending,
    emit: (change) => listeners.forEach((listener) => listener(change)),
    listeners,
    companionRead(extensionId, surfaceId) {
      calls.push(["read", extensionId, surfaceId]);
      return new Promise((resolve, reject) =>
        pending.push({ kind: "read", resolve, reject }),
      );
    },
    companionAction(extensionId, surfaceId, actionId) {
      calls.push(["action", extensionId, surfaceId, actionId]);
      return new Promise((resolve, reject) =>
        pending.push({ kind: "action", resolve, reject }),
      );
    },
    onCompanionChanged(callback) {
      listeners.add(callback);
      return () => listeners.delete(callback);
    },
  };
}
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("a button shows busy, then the new values; view.changed refreshes", async () => {
  const { createCompanionController } =
    await import("../src/extensions/companionModel.ts");
  const bridge = fakeBridge();
  const states = [];
  const controller = createCompanionController(
    bridge,
    "fixture.ext",
    "fixture.page",
    (state) => states.push(state),
  );
  controller.start();
  bridge.pending.shift().resolve({ values: { note: "first" } });
  await tick();
  assert.equal(states.at(-1).values.note, "first");
  assert.equal(states.at(-1).loaded, true);

  const running = controller.run("add");
  assert.equal(states.at(-1).busy, "add");
  // A second click while busy is ignored.
  await controller.run("sync");
  assert.equal(bridge.calls.filter((call) => call[0] === "action").length, 1);
  bridge.pending.shift().resolve({
    values: { note: "second", code: "pair:2" },
    message: "Device added",
  });
  await running;
  assert.equal(states.at(-1).busy, null);
  assert.deepEqual(states.at(-1).values, { note: "second", code: "pair:2" });
  assert.equal(states.at(-1).message, "Device added");

  // Only this surface's change triggers a read.
  bridge.emit({ extensionId: "fixture.ext", surfaceId: "other" });
  bridge.emit({ extensionId: "other.ext", surfaceId: "fixture.page" });
  assert.equal(bridge.pending.length, 0);
  bridge.emit({ extensionId: "fixture.ext", surfaceId: "fixture.page" });
  assert.equal(bridge.pending.length, 1);
  bridge.pending.shift().resolve({ values: { note: "third" } });
  await tick();
  assert.equal(states.at(-1).values.note, "third");

  controller.stop();
  assert.equal(bridge.listeners.size, 0);
});

test("an older read never overwrites a newer one, and failures surface", async () => {
  const { createCompanionController } =
    await import("../src/extensions/companionModel.ts");
  const bridge = fakeBridge();
  const states = [];
  const controller = createCompanionController(bridge, "e", "s", (state) =>
    states.push(state),
  );
  controller.start();
  bridge.emit({ extensionId: "e", surfaceId: "s" });
  const [older, newer] = bridge.pending.splice(0);
  newer.resolve({ values: { note: "new" } });
  await tick();
  older.resolve({ values: { note: "old" } });
  await tick();
  assert.equal(states.at(-1).values.note, "new");

  const failing = controller.run("add");
  bridge.pending.shift().reject(new Error("companion is not running"));
  await failing;
  assert.equal(states.at(-1).busy, null);
  assert.equal(states.at(-1).error, "companion is not running");
});

const surface = (extensionId, id, kind = "companion") => ({
  id,
  extensionId,
  title: `${id} title`,
  allowedHosts: ["settings.page"],
  defaultHost: "settings.page",
  instancePolicy: "singleton",
  stateId: id,
  stateVersion: 1,
  stateScope: "global",
  view: kind === "companion" ? view : { kind: "core", viewId: "probe.view" },
});

test("settings.page tabs are ordered by extension id then surface id", async () => {
  const { ExtensionRegistry } = await import("../src/extensions/registry.ts");
  const { ExtensionSettingsTabs, settingsPageKey, settingsPageSurfaces } =
    await load("extensions/ExtensionSlots.tsx");
  const registry = new ExtensionRegistry();
  const extension = (id, status = "active") => ({
    manifest: { id, name: id, version: "1.0.0", contributions: {} },
    status,
  });
  registry.applySnapshot({
    schemaVersion: 2,
    version: 1,
    problems: [],
    extensions: [
      extension("zeta.ext"),
      extension("alpha.ext"),
      extension("off.ext", "disabled"),
    ],
    surfaces: [
      surface("zeta.ext", "a.page"),
      surface("alpha.ext", "b.page"),
      surface("alpha.ext", "a.page"),
      surface("off.ext", "c.page"),
    ],
    navigation: [],
    actions: [],
    commands: [],
  });
  assert.deepEqual(settingsPageSurfaces(registry).map(settingsPageKey), [
    "page:alpha.ext:a.page",
    "page:alpha.ext:b.page",
    "page:zeta.ext:a.page",
  ]);
  const html = renderToStaticMarkup(
    React.createElement(ExtensionSettingsTabs, {
      registry,
      current: "page:alpha.ext:b.page",
      onSelect() {},
    }),
  );
  assert.deepEqual(
    [...html.matchAll(/<\/svg>([^<]+)<\/button>/g)].map((m) => m[1]),
    ["a.page title", "b.page title", "a.page title"],
  );
  assert.equal((html.match(/aria-selected="true"/g) || []).length, 1);
});

test("a core view in a pane gets the panel's args and saves new ones", async () => {
  const { ExtensionRegistry } = await import("../src/extensions/registry.ts");
  const { ExtensionSurface } = await load("extensions/SurfaceRenderer.tsx");
  const registry = new ExtensionRegistry();
  registry.applySnapshot({
    schemaVersion: 2,
    version: 1,
    problems: [],
    extensions: [
      {
        manifest: { id: "fixture.ext", name: "Fixture", contributions: {} },
        status: "active",
      },
    ],
    surfaces: [surface("fixture.ext", "fixture.pane", "core")],
    navigation: [],
    actions: [],
    commands: [],
  });
  const saved = [];
  seen.props = null;
  renderToStaticMarkup(
    React.createElement(ExtensionSurface, {
      panel: {
        id: "p1",
        kind: "extension",
        extension: {
          extensionId: "fixture.ext",
          contributionId: "fixture.pane",
          instanceId: "i1",
          stateVersion: 1,
          args: { repo: "/work/example", view: "plan" },
        },
      },
      cwd: "/work/example",
      connection: "host-1",
      registry,
      onArgs: (args) => saved.push(args),
    }),
  );
  assert.deepEqual(seen.props.args, { repo: "/work/example", view: "plan" });
  assert.equal(seen.props.cwd, "/work/example");
  assert.equal(seen.props.connection, "host-1");
  seen.props.onArgs({ repo: "/work/example", view: "tasks" });
  assert.deepEqual(saved, [{ repo: "/work/example", view: "tasks" }]);
});
