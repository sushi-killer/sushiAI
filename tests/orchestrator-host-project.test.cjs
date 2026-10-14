// Choosing a remote host in the Orchestrator keeps the workspace's project:
// its checkout on that host opens, a host without one offers to bring it,
// and a task created on another host is revealed where it runs.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const React = require("react");
const esbuild = require("esbuild");
const { renderToStaticMarkup } = require("react-dom/server");
const {
  projectOnHost,
  createdTaskTarget,
} = require("../src/orchestrator/hostProject.ts");

const PROJECT = {
  id: "p1",
  name: "app",
  git: { url: "git@github.com:me/app.git" },
  env: [],
};
const readiness = (checkout, setup = { ok: true, configured: true }) => ({
  platform: "Linux x86_64",
  checkout: { nonStandard: false, ...checkout },
  setup,
  clis: {},
  mcp: { ok: true, count: 0 },
  secrets: { ok: true, count: 0 },
  withheld: false,
});

function fakeBridge({ project = PROJECT, check }) {
  const calls = { resolve: [], check: [] };
  const bridge = {
    projectsResolve: async (query) => {
      calls.resolve.push(query);
      return project;
    },
    projectHostCheck: async (id, host) => {
      calls.check.push([id, host]);
      if (check instanceof Error) throw check;
      return check;
    },
  };
  return { bridge, calls };
}
const WHERE = { endpoint: undefined, cwd: "/Users/me/app", host: "ssh:lab" };

test("a host that has the project's checkout opens it, not another repo", async () => {
  const check = readiness({ ok: true, path: "/home/dev/app" });
  const { bridge, calls } = fakeBridge({ check });
  const placed = await projectOnHost({ bridge, ...WHERE });
  assert.equal(placed.kind, "open");
  assert.equal(placed.repo, "/home/dev/app");
  assert.equal(placed.project.id, "p1");
  assert.equal(calls.resolve[0].cwd, "/Users/me/app");
  assert.equal(calls.resolve[0].endpoint, "local");
  assert.deepEqual(calls.check, [["p1", "ssh:lab"]]);
});

test("a checkout whose install is stale still opens where it is", async () => {
  const setup = { ok: false, configured: true, stale: true };
  const check = readiness({ ok: true, path: "/opt/app" }, setup);
  const { bridge } = fakeBridge({ check });
  const placed = await projectOnHost({ bridge, ...WHERE });
  assert.equal(placed.kind, "open");
  assert.equal(placed.repo, "/opt/app");
});

test("a host without the checkout offers to bring the project", async () => {
  const check = readiness({ ok: false, path: "" });
  const { bridge } = fakeBridge({ check });
  const placed = await projectOnHost({ bridge, ...WHERE });
  assert.equal(placed.kind, "bring");
  assert.equal(placed.project.name, "app");
});

test("a folder that is no project with a remote asks for a repo path", async () => {
  for (const project of [null, { ...PROJECT, git: undefined }]) {
    const { bridge, calls } = fakeBridge({ project, check: readiness({}) });
    const placed = await projectOnHost({ bridge, ...WHERE });
    assert.equal(placed.kind, "pick");
    assert.deepEqual(calls.check, []);
  }
});

test("a host that cannot be checked asks for a repo path", async () => {
  const { bridge } = fakeBridge({ check: new Error("ssh: timed out") });
  const placed = await projectOnHost({ bridge, ...WHERE });
  assert.equal(placed.kind, "pick");
});

test("a task created on another host is revealed there; one on the panel's host is not", () => {
  const remote = { id: "t2", repo: "/home/dev/app" };
  const away = { panelHost: "local", runHost: "ssh:lab" };
  const target = createdTaskTarget(remote, away);
  assert.deepEqual(target, {
    taskId: "t2",
    repo: "/home/dev/app",
    host: "ssh:lab",
    focus: "summary",
  });
  const local = { id: "t1", repo: "/Users/me/app" };
  const here = { panelHost: "local", runHost: "local" };
  assert.equal(createdTaskTarget(local, here), null);
});

const VIEWS = path.resolve(
  __dirname,
  "../src/orchestrator/RemoteHostViews.tsx",
);

async function loadViews() {
  const result = await esbuild.build({
    entryPoints: [VIEWS],
    bundle: true,
    write: false,
    format: "cjs",
    platform: "node",
    jsx: "automatic",
    packages: "external",
    loader: { ".css": "empty" },
  });
  const module = { exports: {} };
  new Function("module", "exports", "require", result.outputFiles[0].text)(
    module,
    module.exports,
    require,
  );
  return module.exports;
}

function buttonWith(markup, text) {
  const part = markup.split("</button>").find((item) => item.includes(text));
  return part ? part.slice(part.lastIndexOf("<button")) : "";
}

test("the Bring card names the project and the host and offers its three ways out", async () => {
  const { BringProject } = await loadViews();
  const markup = renderToStaticMarkup(
    React.createElement(BringProject, {
      projectName: "app",
      hostName: "lab",
      onBring: () => {},
      onPickRepo: () => {},
      onCancel: () => {},
    }),
  );
  assert.match(markup, /clone/i);
  assert.match(buttonWith(markup, "Bring app to lab"), /^<button/, markup);
  assert.match(buttonWith(markup, "Choose another repo"), /^<button/, markup);
  assert.match(buttonWith(markup, "Cancel"), /^<button/, markup);
});

test("the panel places the workspace's project on the chosen host instead of picking another repo", () => {
  const source = fs.readFileSync(
    path.resolve(__dirname, "../src/orchestrator/OrchestratorPanel.tsx"),
    "utf8",
  );
  assert.match(source, /projectOnHost\(/);
  assert.match(source, /<BringProject\b/);
  assert.match(source, /createdTaskTarget\(/);
  assert.equal(source.includes('suggestions[0] ?? ""'), false);
});
