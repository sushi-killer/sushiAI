// The setup a host gets on connect: what it already has stays, and sushiai is
// installed from the host manifest.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const { createHash } = require("node:crypto");
const path = require("node:path");
const {
  setupHost,
  loadHostManifest,
  hostManifestFile,
  requireHostManifest,
  setupSummary,
  cleanEnvironment,
} = require("../electron/host-setup.cjs");
const { makeHost } = require("./helpers/fake-host.cjs");

const isolatePath = (script) =>
  script
    .replace(/^export PATH=.*\n/m, "")
    .replace('export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"; ', "");

async function makeSetupHost(t, options) {
  const host = await makeHost(t, options);
  const exec = host.connections.exec.bind(host.connections);
  host.connections.exec = (endpoint, command, options) =>
    exec(
      endpoint,
      isolatePath(command),
      typeof options?.input === "string"
        ? { ...options, input: isolatePath(options.input) }
        : options,
    );
  return host;
}

const tools = {
  curl: "#!/bin/sh\nexit 1\n",
  claude: "#!/bin/sh\n",
  codex: "#!/bin/sh\n",
};

test("a host with every tool keeps them and gets nothing installed", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const states = await setupHost(host.connections, host.endpoint);
  assert.equal(states.claude, "present");
  assert.equal(states.codex, "present");
  assert.equal(states.sushiai, undefined);
  assert.equal(setupSummary(states), "");
});

test("a script run here sees only the OS values of the app's own variables", () => {
  const seen = cleanEnvironment({
    HOME: "/home/user",
    PATH: "/usr/bin",
    ELECTRON_RUN_AS_NODE: "1",
    CLAUDECODE: "1",
    npm_config_cache: "/tmp/cache",
  });
  assert.deepEqual(seen, { HOME: "/home/user", PATH: "/usr/bin" });
});

// A built-in sushiai binary the fake host can run: it logs its arguments.
async function sushiaiDist(t) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "sushiai-dist-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const bytes = Buffer.from('#!/bin/sh\necho "$@" >> "$HOME/sushiai.log"\n');
  await fs.mkdir(path.join(dir, "linux-x64"));
  await fs.writeFile(path.join(dir, "linux-x64", "sushiai"), bytes);
  const manifest = {
    "Linux x86_64": {
      target: "linux-x64",
      path: "linux-x64/sushiai",
      version: "0.1.0",
      size: bytes.length,
      sha256: createHash("sha256").update(bytes).digest("hex"),
    },
  };
  await fs.writeFile(path.join(dir, "manifest.json"), JSON.stringify(manifest));
  return dir;
}

test("fake ssh: a host without sushiai gets it installed, linked and hooked, then reports unchanged", async (t) => {
  const host = await makeSetupHost(t, {
    bin: {
      ...tools,
      uname:
        '#!/bin/sh\n[ "$1" = "-sm" ] && echo "Linux x86_64" || echo Linux\n',
    },
  });
  const dir = await sushiaiDist(t);
  const sushiai = loadHostManifest(path.join(dir, "manifest.json"));
  assert.equal(sushiai.manifest["Linux x86_64"].version, "0.1.0");
  const options = { sushiai };
  const started = Date.now();
  const first = await setupHost(host.connections, host.endpoint, options);
  assert.ok(Date.now() - started < 60000);
  assert.equal(first.sushiai, "installed", first.sushiaiError);
  assert.equal(first.sushiaiVersion, "0.1.0");
  assert.match(setupSummary(first), /sushiai installed/);
  const link = path.join(host.home, ".sushiai/bin/sushiai");
  assert.match(
    await fs.readlink(link),
    /\.sushiai\/versions\/[0-9a-f]{64}\/sushiai$/,
  );
  assert.equal(
    await fs.readFile(path.join(host.home, "sushiai.log"), "utf8"),
    "hooks install\n",
  );
  const second = await setupHost(host.connections, host.endpoint, options);
  assert.equal(second.sushiai, "unchanged");
});

test("the tools and sushiai steps run; a command host is skipped with a note", async (t) => {
  const host = await makeSetupHost(t, {
    bin: {
      ...tools,
      uname:
        '#!/bin/sh\n[ "$1" = "-sm" ] && echo "Linux x86_64" || echo Linux\n',
    },
  });
  const dir = await sushiaiDist(t);
  const sushiai = loadHostManifest(path.join(dir, "manifest.json"));
  const states = await setupHost(host.connections, host.endpoint, { sushiai });
  assert.equal(states.sushiai, "installed", states.sushiaiError);
  assert.equal(states.sushiaiResult.status, "installed");
  assert.equal(states.sushiaiResult.version, "0.1.0");
  const command = await host.connections.save({
    name: "Tunnel",
    connector: { kind: "command", argv: ["my-tunnel"] },
  });
  const skipped = await setupHost(host.connections, `ssh:${command.id}`);
  assert.match(skipped.note, /no shell/);
});

test("the host manifest is read from the resources when packaged and target/host in a checkout", () => {
  assert.equal(
    hostManifestFile({ isPackaged: true, resourcesPath: "/app/Resources" }),
    "/app/Resources/host/manifest.json",
  );
  assert.equal(
    hostManifestFile({ isPackaged: false, repoRoot: "/repo" }),
    "/repo/target/host/manifest.json",
  );
  assert.throws(
    () => requireHostManifest({ repoRoot: "/nonexistent-repo" }),
    /Run npm run build:host first/,
  );
  assert.throws(
    () =>
      requireHostManifest({ isPackaged: true, resourcesPath: "/nonexistent" }),
    /no sushiai binaries/,
  );
});

test("a failed sushiai install is reported without stopping the rest of the setup", async (t) => {
  const host = await makeSetupHost(t, {
    bin: tools,
  });
  const states = await setupHost(host.connections, host.endpoint, {
    sushiai: { manifest: {}, binDir: os.tmpdir() },
    installSushiaiBinary: async () => {
      throw new Error("no sushiai binary for Linux x86_64");
    },
  });
  assert.equal(states.sushiai, "failed");
  assert.match(setupSummary(states), /sushiai failed · no sushiai binary/);
  assert.equal(states.claude, "present");
});

test("no sushiai option means no sushiai install", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const states = await setupHost(host.connections, host.endpoint, {
    installSushiaiBinary: async () => assert.fail("must not run"),
  });
  assert.equal(states.sushiai, undefined);
});
