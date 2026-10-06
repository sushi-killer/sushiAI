// A remote host's orchestrator over the real daemon manager and ssh connector,
// with a fake `ssh host sushiai proxy` (tests/fixtures/fake-orch-proxy.cjs):
// tasks are created through the proxy, events come back per host, secrets are
// pushed before the task, and no `-L` socket forward exists anywhere.
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { createDaemonManager } = require("../electron/daemon/manager.cjs");
const { createSshConnector } = require("../electron/daemon/ssh.cjs");
const { createOrchestratorHosts } = require("../electron/orchestrator.cjs");
const {
  parsePreflight,
  PREFLIGHT_SCRIPT,
} = require("../electron/host-setup.cjs");

const cleanups = [];
test.afterEach(async () => {
  while (cleanups.length) await cleanups.pop()();
});

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check, ms = 5000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const value = await check();
    if (value) return value;
    await sleep(10);
  }
  throw new Error("timed out");
};

const PROFILE = { id: "host-1", host: "user@devbox", name: "Devbox" };
const ENDPOINT = `ssh:${PROFILE.id}`;

function fakeProxy(mode = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "fop-"));
  cleanups.push(() => fs.rmSync(dir, { recursive: true, force: true }));
  const script = path.join(dir, "ssh");
  fs.writeFileSync(
    script,
    `#!${process.execPath}\nprocess.env.FAKE_PROXY_DIR=${JSON.stringify(dir)};\nrequire(${JSON.stringify(path.join(__dirname, "fixtures/fake-orch-proxy.cjs"))});\n`,
    { mode: 0o755 },
  );
  fs.writeFileSync(path.join(dir, "mode.json"), JSON.stringify(mode));
  const lines = (name) => {
    try {
      return fs
        .readFileSync(path.join(dir, name), "utf8")
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    } catch {
      return [];
    }
  };
  return {
    script,
    requests: () => lines("requests.log"),
    spawns: () => lines("args.log"),
  };
}

function rig(proxy) {
  const manager = createDaemonManager({
    connectors: {
      [PROFILE.id]: createSshConnector({
        profile: PROFILE,
        ssh: proxy.script,
        args: () => ["-T", "-o", "BatchMode=yes"],
        helloTimeoutMs: 3000,
      }),
    },
    backoffMinMs: 20,
    backoffMaxMs: 200,
    random: () => 1,
  });
  cleanups.push(() => manager.close());
  const relayed = [];
  const notices = [];
  const connections = {
    get: () => PROFILE,
    list: () => [PROFILE],
    hasShell: () => true,
    exec: async () => "git=1\nclaude=1\nclaude_login=1\n",
  };
  const hosts = createOrchestratorHosts({
    send: (channel, value) => relayed.push([channel, value]),
    notify: (notice) => notices.push(notice),
    getConnections: () => connections,
    getManager: () => manager,
    getProjects: () => ({
      agentEnvironments: async () => ({}),
      mcpEnvironments: async () => ({}),
      resolveFolder: async () => null,
    }),
    enabled: false,
  });
  return { manager, hosts, relayed, notices };
}

test("a remote task is created through the proxy, its events come back tagged with the host, and no socket is forwarded", async () => {
  const proxy = fakeProxy();
  const { manager, hosts, relayed, notices } = rig(proxy);
  await hosts.setEnabled(true);
  // Saved hosts connect on their first request, not at launch.
  assert.equal(proxy.spawns().length, 0);

  const task = await hosts.call(
    "task.create",
    { repo: "/home/dev/app", title: "Add export" },
    ENDPOINT,
  );
  assert.equal(task.id, "task-1");
  assert.equal(task.host, ENDPOINT);
  assert.equal(manager.states()[0].state, "ready");

  // The module's event arrives with the host.
  await until(() => relayed.length > 0);
  assert.equal(relayed[0][0], "orchestrator-event");
  assert.equal(relayed[0][1].task.host, ENDPOINT);
  assert.equal(relayed[0][1].host, ENDPOINT);

  const listed = await hosts.call("task.list", {}, ENDPOINT);
  assert.deepEqual(
    listed.map((item) => [item.id, item.host]),
    [["task-1", ENDPOINT]],
  );

  // Secrets went first: the daemon holds them in memory only.
  const methods = proxy.requests().map((request) => request.method);
  assert.ok(methods.indexOf("orch.secrets.set") >= 0);
  assert.ok(
    methods.indexOf("orch.secrets.set") < methods.indexOf("orch.task.create"),
  );
  // One proxy command per connection; never a -L forward.
  assert.equal(proxy.spawns().length, 1);
  const args = proxy.spawns().flat();
  assert.equal(args.includes("-L"), false);
  assert.ok(args.some((arg) => /sushiai[^\n]*proxy/.test(arg)));
  assert.equal(notices.length, 0, "a queued task needs no notice");
});

test("a host whose sushiai has no orch capability says Update sushiai on <host>, and the panel shows the same", async () => {
  const proxy = fakeProxy({ capabilities: ["sessions", "attach"] });
  const { hosts } = rig(proxy);
  await hosts.setEnabled(true);
  await assert.rejects(hosts.call("task.list", {}, ENDPOINT), {
    message: "Update sushiai on Devbox",
    code: "ORCH_MISSING",
  });
  assert.deepEqual(
    proxy
      .requests()
      .filter((request) => request.method !== "orch.settings.get"),
    [],
  );
  const record = hosts.list().find((host) => host.id === ENDPOINT);
  assert.equal(record.state, "error");
  assert.equal(record.detail, "Update sushiai on Devbox");
});

test("a host without sushiai shows the manager's reason as the failure", async () => {
  const proxy = fakeProxy({ exit: 127, stderr: "sh: sushiai: No such file" });
  const { hosts } = rig(proxy);
  await hosts.setEnabled(true);
  await assert.rejects(hosts.call("task.list", {}, ENDPOINT), {
    message: /sushiai is not installed on this host/,
  });
  const record = hosts.list().find((host) => host.id === ENDPOINT);
  assert.equal(record.state, "error");
  assert.match(record.detail, /not installed/);
});

test("the preflight script and its parser read git, a C linker and each harness CLI", () => {
  assert.match(PREFLIGHT_SCRIPT, /claude auth status/);
  assert.match(PREFLIGHT_SCRIPT, /codex login status/);
  assert.deepEqual(
    parsePreflight("git=1\ncc=1\nclaude=1\nclaude_login=1\ncodex=1\n", 7),
    {
      git: true,
      cc: true,
      claude: { installed: true, loggedIn: true },
      codex: { installed: true, loggedIn: false },
      checkedAt: 7,
    },
  );
  assert.deepEqual(parsePreflight("", 1), {
    git: false,
    cc: false,
    claude: { installed: false, loggedIn: false },
    codex: { installed: false, loggedIn: false },
    checkedAt: 1,
  });
});
