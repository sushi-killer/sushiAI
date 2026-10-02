// The environment setup a host gets on connect: what it already has stays,
// and a Herdr server that is not running is started.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { execFile } = require("node:child_process");
const net = require("node:net");
const {
  setupHost,
  setupSummary,
  cleanEnvironment,
  SETUP_SCRIPT,
} = require("../electron/host-setup.cjs");
const {
  HERDR_CONTRACT,
  releaseArtifact,
} = require("../electron/herdr-contract.cjs");
const {
  checkHerdrCompatibility,
} = require("../electron/herdr-compatibility.cjs");
const { installPinnedHerdr } = require("../electron/herdr-install.cjs");
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
      options?.input
        ? { ...options, input: isolatePath(options.input) }
        : options,
    );
  return host;
}

function herdrCli(
  version = HERDR_CONTRACT.version,
  protocol = HERDR_CONTRACT.protocol,
) {
  const schema = {
    protocol,
    schema_version: HERDR_CONTRACT.schemaVersion,
    schemas: {
      request: {
        oneOf: HERDR_CONTRACT.requiredMethods.map((method) => ({
          properties: { method: { const: method } },
        })),
        $defs: {
          ...Object.fromEntries(
            HERDR_CONTRACT.launchEnvMethods.map((name) => [
              name,
              { properties: { env: {} } },
            ]),
          ),
          Subscription: {
            oneOf: HERDR_CONTRACT.eventTypes.map((type) => ({
              properties: { type: { const: type } },
            })),
          },
        },
      },
    },
  };
  return `#!/bin/sh
case "$1" in
  --version) printf '%s\\n' 'herdr ${version}' ;;
  api) printf '%s\\n' '${JSON.stringify(schema)}' ;;
  terminal) printf 'Usage: herdr terminal session control [OPTIONS] <TARGET>\\n--cols <N>\\n--rows <N>\\n' ;;
  server) printf '%s' "\${HERDR_SOCKET_PATH:-default}" > "$HOME/socket"; touch "$HOME/.herdr-up" ;;
  status) [ -e "$HOME/.herdr-up" ] && echo 'status: running' || echo 'status: not running' ;;
esac
`;
}

const tools = {
  curl: "#!/bin/sh\nexit 1\n",
  claude: "#!/bin/sh\n",
  codex: "#!/bin/sh\n",
};

function checks(
  host,
  {
    daemonVersion = HERDR_CONTRACT.version,
    daemonProtocol = HERDR_CONTRACT.protocol,
  } = {},
) {
  return async (options) =>
    checkHerdrCompatibility({
      ...options,
      connections: {
        socket: async () => options.endpoint,
        exec: (...args) => host.connections.exec(...args),
      },
      rpc: async () => {
        await fs.access(path.join(host.home, ".herdr-up"));
        return { version: daemonVersion, protocol: daemonProtocol };
      },
    });
}

test("a host with every tool keeps them, and its stopped Herdr server is started", async (t) => {
  const host = await makeSetupHost(t, {
    bin: {
      ...tools,
      herdr: herdrCli(),
    },
  });
  const options = { checkCompatibility: checks(host) };
  const states = await setupHost(host.connections, host.endpoint, "", options);
  assert.equal(states.herdr, "present");
  assert.equal(states.claude, "present");
  assert.equal(states.codex, "present");
  assert.equal(states.server, "started");
  assert.equal(states.herdrInstallation, "user");
  assert.equal(states.compatibility.cli.compatible, true);
  assert.equal(states.compatibility.daemon.compatible, true);
  assert.equal(states.compatibility.compatible, true);
  assert.equal(
    await fs.readFile(path.join(host.root, "bin/herdr"), "utf8"),
    herdrCli(),
  );
  assert.doesNotMatch(SETUP_SCRIPT, /herdr|herdr\.dev\/install/);
  assert.match(SETUP_SCRIPT, /claude\.ai\/install\.sh/);
  assert.match(SETUP_SCRIPT, /openai\/codex\/releases\/latest/);
  assert.equal(setupSummary(states), "server started");
  assert.equal(
    (await setupHost(host.connections, host.endpoint, "", options)).server,
    "running",
  );
});

test("a connection's own nondefault socket is the one checked and started", async (t) => {
  const host = await makeSetupHost(t, { bin: { ...tools, herdr: herdrCli() } });
  const socket = "~/run/my herdr.sock";
  await host.connections.save({
    ...host.connections.get(host.endpoint),
    socket,
  });
  const options = { checkCompatibility: checks(host) };
  await setupHost(host.connections, host.endpoint, socket, options);
  assert.equal(
    await fs.readFile(path.join(host.home, "socket"), "utf8"),
    path.join(host.home, "run/my herdr.sock"),
  );
  await assert.rejects(
    setupHost(host.connections, host.endpoint, "~/another.sock", options),
    /must match/,
  );
});

test("an incompatible user CLI stays intact and cannot bootstrap a stopped daemon", async (t) => {
  const host = await makeSetupHost(t, {
    bin: { ...tools, herdr: herdrCli("0.0.1", 19) },
  });
  let installs = 0;
  const states = await setupHost(host.connections, host.endpoint, "", {
    checkCompatibility: checks(host),
    installRemote: async () => {
      installs++;
      throw new Error("must not overwrite user CLI");
    },
  });
  assert.equal(states.herdr, "incompatible");
  assert.equal(states.server, "failed");
  assert.equal(states.compatibility.cli.compatible, false);
  assert.equal(states.compatibility.daemon.available, false);
  assert.equal(installs, 0);
  await assert.rejects(fs.access(path.join(host.home, ".herdr-up")), {
    code: "ENOENT",
  });
  assert.equal(
    await fs.readFile(path.join(host.root, "bin/herdr"), "utf8"),
    herdrCli("0.0.1", 19),
  );
  assert.match(
    setupSummary(states),
    /herdr incompatible.*older than supported/,
  );
});

test("a compatible CLI reports an incompatible running daemon without replacing it", async (t) => {
  const host = await makeSetupHost(t, { bin: { ...tools, herdr: herdrCli() } });
  await fs.writeFile(path.join(host.home, ".herdr-up"), "user daemon");
  const states = await setupHost(host.connections, host.endpoint, "", {
    checkCompatibility: checks(host, {
      daemonVersion: "0.0.1",
      daemonProtocol: 19,
    }),
  });
  assert.equal(states.herdr, "present");
  assert.equal(states.server, "incompatible");
  assert.equal(states.compatibility.cli.compatible, true);
  assert.equal(states.compatibility.daemon.compatible, false);
  assert.equal(
    await fs.readFile(path.join(host.home, ".herdr-up"), "utf8"),
    "user daemon",
  );
  await assert.rejects(fs.access(path.join(host.home, "socket")), {
    code: "ENOENT",
  });
});

test("a missing remote CLI uses the shared managed installer and starts its exact returned binary", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const binary = path.join(
    host.home,
    ".local/share/sushiai/herdr",
    HERDR_CONTRACT.version,
    "herdr",
  );
  let installs = 0;
  const options = {
    checkCompatibility: checks(host),
    installRemote: async (endpoint, connections) => {
      assert.equal(endpoint, host.endpoint);
      assert.equal(connections, host.connections);
      installs++;
      await fs.mkdir(path.dirname(binary), { recursive: true });
      await fs.writeFile(binary, herdrCli(), { mode: 0o700 });
      return { binary, installed: true };
    },
  };
  const first = setupHost(host.connections, host.endpoint, "", options);
  const duplicate = setupHost(host.connections, host.endpoint, "", options);
  assert.equal(first, duplicate);
  const states = await first;
  assert.equal(installs, 1);
  assert.equal(states.herdr, "installed");
  assert.equal(states.herdrInstallation, "managed");
  assert.equal(states.server, "started");
  assert.equal(states.compatibility.cli.binary, binary);
  assert.equal(states.compatibility.compatible, true);
  await assert.rejects(fs.access(path.join(host.home, ".local/bin/herdr")), {
    code: "ENOENT",
  });
  assert.match(
    await fs.readFile(host.log, "utf8"),
    new RegExp(binary.replaceAll(".", "\\.")),
  );
});

test("a managed checksum failure is reported and no daemon is launched", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const states = await setupHost(host.connections, host.endpoint, "", {
    checkCompatibility: checks(host),
    installRemote: async () => {
      throw Object.assign(new Error("Herdr checksum mismatch"), {
        code: "HERDR_CHECKSUM_MISMATCH",
      });
    },
  });
  assert.equal(states.herdr, "failed");
  assert.equal(states.server, "failed");
  assert.equal(states.herdrError.code, "HERDR_CHECKSUM_MISMATCH");
  assert.match(setupSummary(states), /Herdr checksum mismatch/);
  await assert.rejects(fs.access(path.join(host.home, ".herdr-up")), {
    code: "ENOENT",
  });
});

test("local managed installation uses connections' directory and the shared checksum verifier", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const socket = path.join(host.root, "isolated-local.sock");
  const localScript = (script, timeout) =>
    new Promise((resolve, reject) => {
      const child = execFile(
        "/bin/sh",
        ["-s"],
        {
          timeout,
          env: cleanEnvironment({
            HOME: host.home,
            PATH: path.join(host.root, "bin"),
          }),
        },
        (error, stdout) => (error ? reject(error) : resolve(stdout)),
      );
      child.stdin.end(isolatePath(script));
    });
  let fetched;
  const states = await setupHost(host.connections, "local", socket, {
    runLocalScript: localScript,
    checkCompatibility: checks(host),
    installLocal: async (directory) => {
      assert.equal(directory, host.connections.herdrInstallDirectory);
      return installPinnedHerdr(directory, {
        fetchAsset: async (url) => {
          fetched = url;
          return { ok: true, body: [Buffer.from("corrupt release")] };
        },
      });
    },
  });
  assert.equal(fetched, releaseArtifact().url);
  assert.equal(states.herdrError.code, "HERDR_CHECKSUM_MISMATCH");
  assert.equal(states.compatibility.endpoint, socket);
  assert.equal(states.server, "failed");
  await assert.rejects(
    fs.access(path.join(host.root, "herdr", HERDR_CONTRACT.version, "herdr")),
    { code: "ENOENT" },
  );
});

test("local managed startup uses its exact binary, preserves an explicit socket and excludes app environment", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const binary = path.join(
    host.connections.herdrInstallDirectory,
    HERDR_CONTRACT.version,
    "herdr",
  );
  const socket = path.join(host.root, "isolated local.sock");
  const states = await setupHost(host.connections, "local", socket, {
    installLocal: async (directory) => {
      assert.equal(directory, host.connections.herdrInstallDirectory);
      await fs.mkdir(path.dirname(binary), { recursive: true });
      await fs.writeFile(binary, herdrCli(), { mode: 0o700 });
      return { binary, installed: true };
    },
    runLocalScript: (script, timeout) =>
      new Promise((resolve, reject) => {
        const env = cleanEnvironment({
          HOME: host.home,
          PATH: path.join(host.root, "bin"),
          CLAUDE_CODE_TEST: "app-only",
        });
        assert.equal(env.CLAUDE_CODE_TEST, undefined);
        const child = execFile(
          "/bin/sh",
          ["-s"],
          { timeout, env },
          (error, stdout) => (error ? reject(error) : resolve(stdout)),
        );
        child.stdin.end(isolatePath(script));
      }),
    checkCompatibility: async (options) =>
      checkHerdrCompatibility({
        ...options,
        rpc: async () => {
          await fs.access(path.join(host.home, ".herdr-up"));
          return {
            version: HERDR_CONTRACT.version,
            protocol: HERDR_CONTRACT.protocol,
          };
        },
      }),
  });
  assert.equal(states.herdr, "installed");
  assert.equal(states.herdrInstallation, "managed");
  assert.equal(states.compatibility.cli.binary, binary);
  assert.equal(states.server, "started");
  assert.equal(
    await fs.readFile(path.join(host.home, "socket"), "utf8"),
    socket,
  );
});

test("an occupied unresponsive socket is left intact instead of bootstrapping another daemon", async (t) => {
  const host = await makeSetupHost(t, { bin: { ...tools, herdr: herdrCli() } });
  const socket = path.join(host.home, "occupied.sock");
  await host.connections.save({
    ...host.connections.get(host.endpoint),
    socket,
  });
  const server = net.createServer((client) => client.destroy());
  t.after(() => new Promise((resolve) => server.close(resolve)));
  await new Promise((resolve) => server.listen(socket, resolve));
  const states = await setupHost(host.connections, host.endpoint, socket, {
    checkCompatibility: checks(host),
  });
  assert.equal(states.herdr, "present");
  assert.equal(states.server, "failed");
  assert.equal(server.listening, true);
  assert.equal((await fs.stat(socket)).isSocket(), true);
  await assert.rejects(fs.access(path.join(host.home, "socket")), {
    code: "ENOENT",
  });
});

test("a release download failure is visible and never launches a daemon", async (t) => {
  const host = await makeSetupHost(t, { bin: tools });
  const states = await setupHost(host.connections, host.endpoint, "", {
    checkCompatibility: checks(host),
    installRemote: async () => {
      throw new Error("Herdr download failed (503).");
    },
  });
  assert.equal(states.herdr, "failed");
  assert.equal(states.server, "failed");
  assert.match(states.herdrError.message, /download failed \(503\)/);
  assert.match(setupSummary(states), /download failed \(503\)/);
  await assert.rejects(fs.access(path.join(host.home, ".herdr-up")), {
    code: "ENOENT",
  });
});
