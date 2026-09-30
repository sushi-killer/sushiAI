const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readiness, hostProbeScript } = require("../electron/project-hosts.cjs");
const { Projects } = require("../electron/projects.cjs");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { Connections } = require("../electron/connections.cjs");

test("fake ssh probe reports checkout, setup, CLI, MCP, and secret readiness", () => {
  const script = hostProbeScript("/home/dev/sushiai/demo");
  assert.match(script, /mkdir -p/);
  assert.match(script, /git remote get-url origin/);
  assert.match(script, /timeout 10/);
  assert.match(script, /echo "cc=1"/);
  assert.match(
    hostProbeScript(undefined, "Demo Project"),
    /sushiai\/demo-project/,
  );
  const matrix = readiness({
    now: 10,
    cwd: "/home/dev/sushiai/demo",
    project: {
      host: "ssh:user@devbox",
      git: { url: "https://example.invalid/team/demo.git" },
      setup: { install: "npm ci", check: "npm test" },
      mcp: { local: {} },
      env: [{ name: "TOKEN", secret: true, hasValue: true }],
      hosts: { "ssh:user@devbox": { trusted: true } },
    },
    output:
      "home=/home/dev\nroot=/home/dev/sushiai/demo\nstandard=1\nremote=https://example.invalid/team/demo.git\ngit=1\nclaude=1\nclaude_login=1\ncodex=1\n",
  });
  assert.equal(matrix.checkout.ok, true);
  assert.equal(matrix.checkout.nonStandard, false);
  assert.equal(matrix.setup.ok, true);
  assert.equal(matrix.clis.claude.loggedIn, true);
  assert.equal(matrix.clis.codex.loggedIn, false);
  assert.equal(matrix.clis.git, true);
  assert.equal(matrix.clis.cc, false);
  assert.deepEqual(matrix.mcp, { ok: true, count: 1 });
  assert.deepEqual(matrix.secrets, { ok: true, count: 1 });
  assert.equal(matrix.trusted, true);
});

test("the hosts readiness IPC probes through a fake ssh executable", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "fake-ssh-hosts-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const ssh = path.join(dir, "ssh");
  await fs.writeFile(
    ssh,
    '#!/bin/sh\nprintf "home=/home/user\\nroot=/home/user/sushiai/demo\\nstandard=1\\nremote=https://example.invalid/team/demo\\ngit=1\\nclaude=1\\nclaude_login=1\\n"\n',
    { mode: 0o755 },
  );
  const connections = new Connections(dir, { ssh });
  const profile = await connections.save({
    id: "00000000-0000-4000-8000-000000000001",
    name: "Devbox",
    host: "user@devbox",
    socket: "~/.sushiai/orchestrator/orchd.sock",
  });
  const output = await connections.exec(`ssh:${profile.id}`, hostProbeScript());
  const matrix = readiness({
    output,
    project: {
      host: `ssh:${profile.id}`,
      git: { url: "git@example.invalid:team/demo.git" },
      setup: {},
      mcp: {},
      env: [],
    },
  });
  assert.equal(matrix.checkout.ok, true);
  assert.equal(matrix.checkout.nonStandard, false);
  assert.equal(matrix.clis.claude.loggedIn, true);
  await connections.disconnect(`ssh:${profile.id}`);
});

test("checkout mismatch is flagged and a non-standard location is detected", () => {
  const matrix = readiness({
    cwd: "/srv/checkout/demo",
    project: {
      host: "ssh:user@devbox",
      git: { url: "https://example.invalid/team/demo" },
      setup: {},
      mcp: {},
      env: [],
    },
    output:
      "home=/home/dev\nroot=/srv/checkout/demo\nremote=https://example.invalid/other/repo\n",
  });
  assert.equal(matrix.checkout.ok, false);
  assert.equal(matrix.checkout.nonStandard, true);
});

test("missing checkout, setup, CLIs, MCP, secrets, and trust report missing states", () => {
  const matrix = readiness({
    output: "home=/home/dev\nroot=/home/dev/sushiai/demo\nstandard=1\n",
    project: {
      host: "ssh:user@devbox",
      git: { url: "https://example.invalid/team/demo" },
      setup: {},
      mcp: {},
      env: [],
      hosts: {},
    },
  });
  assert.equal(matrix.checkout.ok, false);
  assert.equal(matrix.setup.ok, false);
  assert.equal(matrix.clis.claude.installed, false);
  assert.equal(matrix.clis.codex.installed, false);
  assert.equal(matrix.mcp.ok, false);
  assert.equal(matrix.secrets.ok, false);
  assert.equal(matrix.trusted, false);
});

test("revoking project host trust blocks stored secrets on the next read", async (t) => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "project-hosts-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const safeStorage = {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "keychain",
    encryptString: (value) => Buffer.from(value),
    decryptString: (value) => value.toString(),
  };
  const projects = new Projects({ userDataDir: dir, safeStorage });
  const project = await projects.upsert({
    name: "Demo",
    env: [{ name: "TOKEN", secret: true }],
  });
  await projects.setSecret(project.id, "TOKEN", "invented-secret-value");
  await projects.upsert({
    ...project,
    hosts: { "ssh:user@devbox": { trusted: true } },
  });
  assert.equal(
    (await projects.get(project.id)).hosts?.["ssh:user@devbox"]?.trusted,
    undefined,
  );
  await projects.setHostTrust(project.id, "ssh:user@devbox", true);
  await projects.setHostOverrides(project.id, "ssh:user@devbox", {
    backend: "local",
  });
  assert.equal(
    await projects.secretForHost(project.id, "TOKEN", "ssh:user@devbox"),
    "invented-secret-value",
  );
  assert.deepEqual(
    (await projects.get(project.id)).hosts["ssh:user@devbox"].overrides,
    { backend: "local" },
  );
  await projects.setHostTrust(project.id, "ssh:user@devbox", false);
  assert.equal(
    await projects.secretForHost(project.id, "TOKEN", "ssh:user@devbox"),
    null,
  );
});
