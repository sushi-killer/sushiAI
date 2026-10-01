const test = require("node:test");
const assert = require("node:assert/strict");

test("prepare times are remembered per project and host", async (t) => {
  const store = new Map();
  global.window = {
    localStorage: {
      getItem: (key) => store.get(key) ?? null,
      setItem: (key, value) => store.set(key, value),
    },
  };
  t.after(() => delete global.window);
  const { checkoutPath, readPrepareTimes, rememberPrepareTimes } =
    await import("../src/projectPrepare.ts");
  assert.equal(readPrepareTimes("p1", "ssh:a"), undefined);
  rememberPrepareTimes("p1", "ssh:a", [
    { id: "clone", state: "done", seconds: 9 },
  ]);
  assert.deepEqual(readPrepareTimes("p1", "ssh:a"), [
    { id: "clone", state: "done", seconds: 9 },
  ]);
  assert.equal(readPrepareTimes("p1", "ssh:b"), undefined);
  store.set("sushiai.project.prepare-times:p1:ssh:c", "not json");
  assert.equal(readPrepareTimes("p1", "ssh:c"), undefined);

  assert.equal(
    checkoutPath({
      checkout: { ok: true, path: "/home/u/sushiai/app", nonStandard: false },
    }),
    "~/sushiai/app",
  );
  assert.equal(
    checkoutPath({
      checkout: { ok: true, path: "/srv/app", nonStandard: true },
    }),
    "/srv/app",
  );
});

test("the folder name is the same in the app and in the main process, and 'My App!' is my-app", async () => {
  const { projectSlug, repoSlug, tildePath, localTarget } =
    await import("../src/projectPrepare.ts");
  const { projectSlug: mainSlug } = require("../electron/project-hosts.cjs");
  for (const name of [
    "My App!",
    "  Acme  App ",
    "ozon_seller",
    "***",
    "Ünï",
    "a/b",
  ]) {
    assert.equal(projectSlug(name), mainSlug(name), name);
  }
  assert.equal(projectSlug("My App!"), "my-app");
  assert.equal(projectSlug("***"), "project");

  assert.equal(repoSlug("git@example.test:acme/app.git"), "acme/app");
  assert.equal(repoSlug("https://example.test/acme/app/"), "acme/app");
  assert.equal(tildePath("/home/u/x", "/home/u"), "~/x");
  assert.equal(tildePath("/home/u", "/home/u"), "~");
  assert.equal(tildePath("/srv/x", "/home/u"), "/srv/x");

  // A folder opened on another host is never a path on This Mac.
  assert.equal(
    localTarget({
      reuseFolder: false,
      cwd: "/home/user/work/app",
      home: "/Users/me",
      name: "My App!",
    }),
    "/Users/me/sushiai/my-app",
  );
  assert.equal(
    localTarget({
      reuseFolder: true,
      cwd: "/Users/me/code/app",
      home: "/Users/me",
      name: "x",
    }),
    "/Users/me/code/app",
  );
});
