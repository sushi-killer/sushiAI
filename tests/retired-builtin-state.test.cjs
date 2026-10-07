// A profile that saved state for a built-in extension the app no longer ships
// (the retired session provider, id "builtin.herdr") loads without it: no entry
// on the Extensions page, no error, and its saved rows are cleaned up.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const {
  ExtensionManager,
} = require("../electron/extensions/extension-manager.cjs");
const {
  SurfaceStateStore,
} = require("../electron/extensions/surface-state.cjs");
const { readStore, closeAppDb } = require("../electron/app-db.cjs");

const RETIRED = "builtin.herdr";
const builtin = (id) => ({
  id,
  name: id,
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  contributions: { surfaces: [], navigation: [], actions: [], commands: [] },
});

test("saved state of a retired built-in is ignored and cleaned on load", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "retired-builtin-"));
  t.after(async () => {
    closeAppDb(dir);
    await fs.rm(dir, { recursive: true, force: true });
  });
  const kept = builtin("builtin.kept");
  // The profile as an older release left it.
  const old = new ExtensionManager({
    dataDir: dir,
    builtins: [builtin(RETIRED), kept],
  });
  await old.ready;
  await new SurfaceStateStore(dir).write(RETIRED, "view", 1, "scope", {
    note: "left behind",
  });
  assert.ok(old.isEnabled(RETIRED));

  const manager = new ExtensionManager({ dataDir: dir, builtins: [kept] });
  await manager.ready;
  await new SurfaceStateStore(dir).forgetRetired(
    new Set(manager.manifests.keys()),
  );
  const snapshot = await manager.list();
  assert.deepEqual(
    snapshot.extensions.map((entry) => entry.manifest.id),
    ["builtin.kept"],
  );
  assert.equal(snapshot.diagnostic, undefined);
  assert.equal(manager.isEnabled(RETIRED), false);
  assert.equal(
    Object.hasOwn(readStore(dir, "extensions").value.extensions, RETIRED),
    false,
  );
  assert.equal(
    Object.hasOwn(readStore(dir, "extension-lock").value.packages, RETIRED),
    false,
  );
  assert.equal(Object.hasOwn(readStore(dir, "surface-state"), RETIRED), false);
});
