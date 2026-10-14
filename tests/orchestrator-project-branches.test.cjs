const { test } = require("node:test");
const assert = require("node:assert/strict");
const {
  OrchestratorService,
  createOrchestratorHosts,
} = require("../electron/orchestrator.cjs");
const { fakeDaemonManager } = require("./helpers/fake-daemon-manager.cjs");
const { makeStore } = require("./helpers/fake-host.cjs");

const REMOTE = "ssh:devbox-id";
const connections = {
  get: () => ({ name: "Devbox" }),
  list: () => [{ id: "devbox-id", name: "Devbox" }],
  hasShell: () => true,
  exec: async () => "git=1\nclaude=1\nclaude_login=1\n",
};

async function waitUntil(check, { timeout = 4000, interval = 5 } = {}) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  throw new Error("waitUntil: condition never became true");
}

/** Two projects on both hosts: one with a default branch, one without. */
async function branchRig(t) {
  const { projects } = await makeStore(t);
  const named = await projects.upsert({
    name: "Named",
    git: { url: "git@example.test:acme/named.git", defaultBranch: "develop" },
    targets: ["local", REMOTE],
  });
  const blank = await projects.upsert({
    name: "Blank",
    git: { url: "git@example.test:acme/blank.git", defaultBranch: "  " },
    targets: ["local", REMOTE],
  });
  for (const [project, endpoint, cwd] of [
    [named, "local", "/Users/me/named"],
    [named, REMOTE, "/home/user/sushiai/named"],
    [blank, "local", "/Users/me/blank"],
    [blank, REMOTE, "/home/user/sushiai/blank"],
  ])
    await projects.attach({ remote: project.git.url, endpoint, cwd });
  const manager = fakeDaemonManager({
    hosts: { local: {}, "devbox-id": {} },
    handlers: { "orch.settings.get": () => ({ routes: [] }) },
  });
  const service = new OrchestratorService({
    host: REMOTE,
    getManager: () => manager,
    getConnections: () => connections,
    getProjects: () => projects,
    send: () => {},
  });
  // The real wiring: the hosts wrapper sets the store's change hook.
  const hosts = createOrchestratorHosts({
    send: () => {},
    getProjects: () => projects,
    getConnections: () => connections,
    getManager: () => manager,
    enabled: false,
  });
  hosts.services.set(REMOTE, service);
  const pushes = () =>
    manager.requestsFor("orch.secrets.set").filter((c) => c.host !== "local");
  const lastSecrets = () => pushes().at(-1)?.params;
  return { projects, named, service, pushes, lastSecrets };
}

test("repoBranches maps each folder on a host to its project's default branch, leaving out blank ones", async (t) => {
  const { projects } = await branchRig(t);
  assert.deepEqual(await projects.repoBranches("local"), {
    "/Users/me/named": "develop",
  });
  assert.deepEqual(await projects.repoBranches(REMOTE), {
    "/home/user/sushiai/named": "develop",
  });
  assert.deepEqual(await projects.repoBranches("ssh:elsewhere"), {});
});

test("secrets.set sends the host's folder to default branch map as projectBranches", async (t) => {
  const { service, lastSecrets } = await branchRig(t);
  await service.refreshSecrets();
  assert.deepEqual(lastSecrets().projectBranches, {
    "/home/user/sushiai/named": "develop",
  });
});

test("a default branch edit is pushed to the daemon like any other project change", async (t) => {
  const { projects, named, service, pushes, lastSecrets } = await branchRig(t);
  await service.refreshSecrets();
  const before = pushes().length;
  await projects.upsert({
    ...named,
    git: { ...named.git, defaultBranch: "release" },
  });
  await waitUntil(
    () =>
      pushes().length > before &&
      lastSecrets().projectBranches["/home/user/sushiai/named"] === "release",
  );
  // Clearing the branch drops the folder from the map.
  await projects.upsert({ ...named, git: { ...named.git, defaultBranch: "" } });
  await waitUntil(
    () => !("/home/user/sushiai/named" in lastSecrets().projectBranches),
  );
});
