const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { spawn } = require("node:child_process");
const { Connections, quote } = require("../electron/connections.cjs");
const { request } = require("../electron/herdr.cjs");
const {
  HERDR_CONTRACT,
  releaseArtifact,
} = require("../electron/herdr-contract.cjs");
const {
  installPinnedHerdr,
  installRemoteHerdr,
} = require("../electron/herdr-install.cjs");
const {
  assertHerdrCompatibility,
} = require("../electron/herdr-compatibility.cjs");

async function waitFor(check) {
  for (let i = 0; i < 200; i++) {
    if (await check()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("Isolated installed daemon did not become ready");
}

(async () => {
  const directory = await fs.mkdtemp(
    path.join(os.tmpdir(), "herdr-install-verify-"),
  );
  const socket = path.join(directory, "daemon.sock");
  const config = path.join(directory, "config.toml");
  await fs.writeFile(config, '[terminal]\nshell = "/bin/sh"\n');
  const connections = new Connections(path.join(directory, "app"));
  await connections.init();
  let daemon;
  try {
    const verifiedAsset = process.env.SUSHIAI_HERDR_VERIFIED_ASSET;
    const installOptions = verifiedAsset
      ? {
          fetchAsset: async () =>
            new Response(await fs.readFile(verifiedAsset)),
        }
      : {};
    const local = await installPinnedHerdr(
      path.join(directory, "managed"),
      installOptions,
    );
    assert.equal(local.installed, true);
    assert.equal(
      (await installPinnedHerdr(path.join(directory, "managed"))).installed,
      false,
    );
    const profile = await connections.save({
      host: `${os.userInfo().username}@${[127, 0, 0, 1].join(".")}`,
      port: Number(process.env.SUSHIAI_HERDR_BENCH_SSH_PORT),
      socket,
      name: "Isolated installation verification",
    });
    const originalArgs = connections.args.bind(connections);
    connections.args = (value) => [
      ...originalArgs(value),
      "-F",
      "/dev/null",
      "-i",
      process.env.SUSHIAI_HERDR_BENCH_SSH_KEY,
      "-o",
      "IdentitiesOnly=yes",
    ];
    const originalExec = connections.exec.bind(connections);
    connections.exec = (endpoint, command, options) =>
      originalExec(
        endpoint,
        `HOME=${quote(directory)} sh -c ${quote(command)}`,
        options,
      );
    const endpoint = `ssh:${profile.id}`;
    const remote = await installRemoteHerdr(endpoint, connections);
    assert.equal(remote.installed, true);
    assert.equal(
      (await installRemoteHerdr(endpoint, connections)).installed,
      false,
    );
    const expected = releaseArtifact().sha256;
    for (const binary of [local.binary, remote.binary])
      assert.equal(
        createHash("sha256")
          .update(await fs.readFile(binary))
          .digest("hex"),
        expected,
      );
    daemon = spawn(
      connections.ssh,
      [
        ...connections.args(profile),
        profile.host,
        `exec env HOME=${quote(directory)} XDG_CONFIG_HOME=${quote(path.join(directory, "config"))} XDG_STATE_HOME=${quote(path.join(directory, "state"))} HERDR_CONFIG_PATH=${quote(config)} HERDR_SOCKET_PATH=${quote(socket)} HERDR_CLIENT_SOCKET_PATH=${quote(path.join(directory, "client.sock"))} ${quote(remote.binary)} server`,
      ],
      { stdio: ["ignore", "ignore", "pipe"] },
    );
    let stderr = "";
    daemon.stderr.on("data", (chunk) => {
      stderr = (stderr + chunk).slice(-2000);
    });
    await waitFor(async () => {
      if (daemon.exitCode !== null) throw new Error(stderr);
      try {
        await request(socket, "ping", {}, 500);
        return true;
      } catch {
        return false;
      }
    });
    const status = await assertHerdrCompatibility({ endpoint, connections });
    assert.equal(status.cli.binary, remote.binary);
    console.log(
      JSON.stringify(
        {
          recordedAt: new Date().toISOString(),
          version: HERDR_CONTRACT.version,
          sourceCommit: HERDR_CONTRACT.sourceCommit,
          localAssetSource: verifiedAsset || releaseArtifact().url,
          local,
          remote,
          repeatUsesVerifiedInstallation: true,
          expectedSha256: expected,
          status,
        },
        null,
        2,
      ),
    );
  } finally {
    await connections.close();
    if (daemon && daemon.exitCode === null) {
      await request(socket, "server.stop", {}, 2000).catch(() => daemon.kill());
      await waitFor(() => daemon.exitCode !== null).catch(() => daemon.kill());
    }
    await fs.rm(directory, { recursive: true, force: true });
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
