const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const {
  remoteEnvBootstrap,
  remoteEnvPayload,
  remoteFileCommand,
  removeRemoteFiles,
  uploadRemoteFile,
} = require("../electron/ipc/terminals.cjs");
const { Projects } = require("../electron/projects.cjs");

async function runFakeHost(
  t,
  env,
  { token, command = "env", trusted = true } = {},
) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "fake-ssh-env-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const home = path.join(root, "home");
  await fs.mkdir(home);
  const bin = path.join(root, "bin");
  await fs.mkdir(bin);
  if (token)
    await fs.writeFile(path.join(bin, "claude"), "#!/bin/sh\ncat <&3\n", {
      mode: 0o755,
    });
  const config = path.join(root, "ssh.json");
  await fs.writeFile(
    config,
    JSON.stringify({
      home,
      bin: `${bin}:${process.env.PATH}`,
      log: path.join(root, "ssh.log"),
    }),
  );
  const safeStorage = {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => "keychain",
    encryptString: (value) => Buffer.from(value),
    decryptString: (value) => value.toString(),
  };
  const projects = new Projects({ userDataDir: root, safeStorage });
  const project = await projects.upsert({
    name: "Demo",
    env: Object.keys(env).map((name) => ({ name, secret: true })),
  });
  for (const [name, value] of Object.entries(env))
    await projects.setSecret(project.id, name, value);
  await projects.setHostTrust(project.id, "ssh:user@devbox", trusted);
  const deliveredEnv = await projects.environmentFor(
    project.id,
    "agent",
    "ssh:user@devbox",
  );
  const fakeSsh = path.join(__dirname, "fixtures/fake-ssh.cjs");
  const sshEnv = { ...process.env, FAKE_SSH_CONFIG: config };
  async function upload(payload) {
    const ssh = spawn(
      process.execPath,
      [fakeSsh, "-T", "user@devbox", remoteFileCommand()],
      {
        env: sshEnv,
        stdio: ["pipe", "pipe", "pipe"],
      },
    );
    let output = "";
    ssh.stdout.setEncoding("utf8").on("data", (chunk) => (output += chunk));
    ssh.stdin.end(payload);
    const status = await new Promise((resolve) => ssh.on("close", resolve));
    assert.equal(status, 0);
    return output;
  }
  const envPayload = remoteEnvPayload(deliveredEnv);
  const envPath = envPayload ? await upload(envPayload) : null;
  const tokenPath = token ? await upload(token) : null;
  const launch = remoteEnvBootstrap(home, command, envPath, tokenPath);
  const ssh = spawn(
    process.execPath,
    [fakeSsh, "-tt", "user@devbox", launch.shell],
    {
      env: sshEnv,
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  const output = [];
  const errors = [];
  ssh.stdout.on("data", (chunk) => output.push(chunk));
  ssh.stderr.on("data", (chunk) => errors.push(chunk));
  ssh.stdin.end();
  const status = await new Promise((resolve) => ssh.on("close", resolve));
  assert.equal(status, 0, Buffer.concat(errors).toString());
  assert.deepEqual(await fs.readdir(home), []);
  assert.doesNotMatch(
    await fs.readFile(path.join(root, "ssh.log"), "utf8"),
    /invented-secret|invented-subscription-token/,
  );
  return Buffer.concat(output).toString();
}

test("trusted remote env reaches the agent and the one-shot file is removed", async (t) => {
  const output = await runFakeHost(t, { PROJECT_TOKEN: "invented-secret" });
  assert.match(output, /PROJECT_TOKEN=invented-secret/);
});

test("untrusted remote env delivery contains no project values", async (t) => {
  const output = await runFakeHost(
    t,
    { PROJECT_TOKEN: "invented-secret" },
    { trusted: false },
  );
  assert.doesNotMatch(output, /invented-secret|PROJECT_TOKEN=/);
});

test("subscription token reaches the remote agent through its inherited fd", async (t) => {
  const output = await runFakeHost(
    t,
    {},
    {
      token: "invented-subscription-token",
      command: "claude",
    },
  );
  assert.equal(output.trim(), "invented-subscription-token");
});

test("a session that never started leaves no secret file on the host", async (t) => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "fake-ssh-clean-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const home = path.join(root, "home");
  await fs.mkdir(home);
  const config = path.join(root, "ssh.json");
  await fs.writeFile(
    config,
    JSON.stringify({
      home,
      bin: process.env.PATH,
      log: path.join(root, "log"),
    }),
  );
  process.env.FAKE_SSH_CONFIG = config;
  t.after(() => delete process.env.FAKE_SSH_CONFIG);
  const args = [
    path.join(__dirname, "fixtures/fake-ssh.cjs"),
    "-T",
    "user@devbox",
  ];
  const uploaded = await uploadRemoteFile(
    process.execPath,
    [...args, remoteFileCommand()],
    "invented-secret",
  );
  const other = await uploadRemoteFile(
    process.execPath,
    [...args, remoteFileCommand()],
    "invented-token",
  );
  assert.equal(await fs.readFile(uploaded, "utf8"), "invented-secret");
  await removeRemoteFiles(process.execPath, args, [uploaded, null, other]);
  await assert.rejects(fs.access(uploaded));
  await assert.rejects(fs.access(other));
  // Nothing to remove, or a path that is already gone, is not an error.
  await removeRemoteFiles(process.execPath, args, []);
  await removeRemoteFiles(process.execPath, args, [uploaded]);
});
