const { test } = require("node:test");
const assert = require("node:assert/strict");

const source = {
  source: "git",
  url: "https://git.example.invalid/team/first.git",
  cwd: "/tmp/projects",
  name: "first",
  home: "/tmp/home",
  folderEndpoint: "ssh:devbox",
  folderLocal: false,
};
const repository = {
  branch: "trunk",
  envExample: "FIRST_SECRET=\n",
  mcp: "{}",
  install: "npm ci",
};
const firstScan = {
  variables: [{ name: "FIRST_SECRET", secret: true, held: true }],
  servers: { first: { command: "first-server" } },
  token: "first-held-import",
  install: "npm ci",
};

function deferred() {
  let resolve;
  const promise = new Promise((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

test("an unavailable second repository replaces held imports with empty metadata", async () => {
  const { inspectProjectSource } = await import("../src/projectSource.ts");
  const bridge = {
    projectSourceInspect: async (url) => {
      if (url !== source.url) throw new Error("Repository unavailable");
      return repository;
    },
    projectInspect: async () => ({ remote: source.url }),
    projectScanSource: async () => firstScan,
  };
  let draft = await inspectProjectSource(bridge, source, () => true);
  assert.equal(draft.token, "first-held-import");
  assert.equal(draft.variables[0].held, true);
  assert.equal(draft.localFound, true);

  draft = await inspectProjectSource(
    bridge,
    { ...source, url: "https://git.example.invalid/team/private.git" },
    () => true,
  );
  assert.deepEqual(draft.variables, []);
  assert.deepEqual(draft.servers, {});
  assert.equal(draft.token, "");
  assert.equal(draft.install, "");
  assert.equal(draft.branch, "");
  assert.equal(draft.localFound, false);
  assert.match(draft.notice, /still prepare it on the selected host/);
});

test("a late repository inspection cannot populate the replacement source", async () => {
  const { inspectProjectSource } = await import("../src/projectSource.ts");
  const pending = deferred();
  let generation = 0;
  let checked = 0;
  let scanned = 0;
  const bridge = {
    projectSourceInspect: (url) =>
      url === source.url
        ? pending.promise
        : Promise.reject(new Error("Repository unavailable")),
    projectInspect: async () => {
      checked += 1;
      return { remote: source.url };
    },
    projectScanSource: async () => {
      scanned += 1;
      return firstScan;
    },
  };
  const old = inspectProjectSource(bridge, source, () => generation === 0);
  generation += 1;
  const draft = await inspectProjectSource(
    bridge,
    { ...source, url: "https://git.example.invalid/team/private.git" },
    () => generation === 1,
  );
  pending.resolve(repository);
  assert.equal(await old, null);
  assert.equal(draft.token, "");
  assert.deepEqual(draft.variables, []);
  assert.equal(checked, 0);
  assert.equal(scanned, 0);
});

test("changing source during checkout inspection stops the old secret scan", async () => {
  const { inspectProjectSource } = await import("../src/projectSource.ts");
  const checkout = deferred();
  let current = true;
  let scanned = false;
  const old = inspectProjectSource(
    {
      projectSourceInspect: async () => repository,
      projectInspect: () => checkout.promise,
      projectScanSource: async () => {
        scanned = true;
        return firstScan;
      },
    },
    source,
    () => current,
  );
  await Promise.resolve();
  await Promise.resolve();
  current = false;
  checkout.resolve({ remote: source.url });
  assert.equal(await old, null);
  assert.equal(scanned, false);
});

test("a late held-secret scan cannot replace a newly selected folder", async () => {
  const { inspectProjectSource } = await import("../src/projectSource.ts");
  const scan = deferred();
  const started = deferred();
  let generation = 0;
  const bridge = {
    projectSourceInspect: async () => repository,
    projectInspect: async (_, input) => ({
      remote:
        input.root === "/tmp/second"
          ? "git@code.example.invalid:team/second.git"
          : source.url,
    }),
    projectScanSource: (input) => {
      if (input.root === "/tmp/second")
        return Promise.resolve({
          variables: [{ name: "SECOND_VALUE", secret: false, value: "second" }],
          servers: {},
          token: "second-import",
          install: "pnpm install",
        });
      started.resolve();
      return scan.promise;
    },
  };
  const old = inspectProjectSource(bridge, source, () => generation === 0);
  await started.promise;
  generation += 1;
  const draft = await inspectProjectSource(
    bridge,
    { ...source, source: "folder", cwd: "/tmp/second" },
    () => generation === 1,
  );
  scan.resolve(firstScan);
  assert.equal(await old, null);
  assert.equal(draft.token, "second-import");
  assert.deepEqual(draft.variables, [
    { name: "SECOND_VALUE", secret: false, value: "second" },
  ]);
  assert.equal(draft.install, "pnpm install");
  assert.equal(draft.remote, "git@code.example.invalid:team/second.git");
});

test("an unrelated checkout with the same folder name cannot supply held values", async () => {
  const { inspectProjectSource } = await import("../src/projectSource.ts");
  let scanned;
  const draft = await inspectProjectSource(
    {
      projectSourceInspect: async () => repository,
      projectInspect: async () => ({
        remote: "https://other.example.invalid/team/first.git",
      }),
      projectScanSource: async (input) => {
        scanned = input;
        return {
          variables: [],
          servers: {},
          token: "metadata-only",
          install: "",
        };
      },
    },
    source,
    () => true,
  );
  assert.equal(draft.localFound, false);
  assert.equal(scanned.endpoint, undefined);
  assert.equal(scanned.local, false);
  assert.equal(scanned.example, repository.envExample);
});
