const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { createHash } = require("node:crypto");
const { performance } = require("node:perf_hooks");
const { request } = require("../electron/herdr.cjs");
const { Connections, quote, run } = require("../electron/connections.cjs");
const {
  assertHerdrCompatibility,
} = require("../electron/herdr-compatibility.cjs");
const { HerdrEvents } = require("../electron/herdr-events.cjs");
const { openHerdrStream } = require("../electron/terminal-stream.cjs");

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const samples = Number(process.env.SUSHIAI_HERDR_BENCH_SAMPLES || 100);
const idleMs = Number(process.env.SUSHIAI_HERDR_BENCH_IDLE_MS || 10000);

async function waitFor(check, label, timeout = 10000) {
  const deadline = performance.now() + timeout;
  while (performance.now() < deadline) {
    if (await check()) return;
    await sleep(10);
  }
  throw new Error(`Timed out: ${label}`);
}

function percentiles(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {
    p50: sorted[Math.floor(sorted.length * 0.5)],
    p95: sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))],
    min: sorted[0],
    max: sorted.at(-1),
    samples: sorted.length,
    rawMs: values,
  };
}

class BenchConnections extends Connections {
  args(profile) {
    return [
      ...super.args(profile),
      "-F",
      "/dev/null",
      "-i",
      process.env.SUSHIAI_HERDR_BENCH_SSH_KEY,
      "-o",
      "IdentitiesOnly=yes",
    ];
  }
}

async function cpuTicks(pid) {
  const stat = await fs.readFile(`/proc/${pid}/stat`, "utf8");
  const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
  return Number(fields[11]) + Number(fields[12]);
}

async function measure(binary, transport) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-bench-"));
  const socket = path.join(directory, "api.sock");
  const connections =
    transport === "ssh"
      ? new BenchConnections(path.join(directory, "app"))
      : new Connections(path.join(directory, "app"));
  await connections.init();
  const config = path.join(directory, "config.toml");
  await fs.writeFile(config, '[terminal]\nshell = "/bin/sh"\n');
  const env = {
    ...process.env,
    HOME: directory,
    XDG_CONFIG_HOME: path.join(directory, "config"),
    XDG_STATE_HOME: path.join(directory, "state"),
    HERDR_CONFIG_PATH: config,
    HERDR_SOCKET_PATH: socket,
    HERDR_CLIENT_SOCKET_PATH: path.join(directory, "client.sock"),
    HERDR_DISABLE_SOUND: "1",
  };
  let endpoint = socket,
    daemon,
    daemonPid,
    events,
    stream;
  let diagnostic = "";
  try {
    if (transport === "ssh") {
      const profile = await connections.save({
        host: `${os.userInfo().username}@${[127, 0, 0, 1].join(".")}`,
        port: Number(process.env.SUSHIAI_HERDR_BENCH_SSH_PORT),
        socket,
        name: "Isolated SSH benchmark",
      });
      endpoint = `ssh:${profile.id}`;
      const assignments = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "HERDR_CONFIG_PATH",
        "HERDR_SOCKET_PATH",
        "HERDR_CLIENT_SOCKET_PATH",
        "HERDR_DISABLE_SOUND",
      ]
        .map((key) => `${key}=${quote(env[key])}`)
        .join(" ");
      daemon = spawn(
        connections.ssh,
        [
          ...connections.args(profile),
          profile.host,
          `echo $$ > ${quote(path.join(directory, "daemon.pid"))}; exec env ${assignments} ${quote(binary)} server`,
        ],
        { stdio: ["ignore", "ignore", "pipe"] },
      );
    } else {
      daemon = spawn(binary, ["server"], {
        env,
        stdio: ["ignore", "ignore", "pipe"],
      });
      daemonPid = daemon.pid;
    }
    daemon.stderr.on(
      "data",
      (chunk) => (diagnostic = (diagnostic + chunk).slice(-4000)),
    );
    await waitFor(async () => {
      if (daemon.exitCode !== null) throw new Error(diagnostic);
      try {
        await request(socket, "ping", {}, 500);
        return true;
      } catch {
        return false;
      }
    }, "isolated daemon");
    if (transport === "ssh")
      daemonPid = Number(
        await fs.readFile(path.join(directory, "daemon.pid"), "utf8"),
      );
    let forwarded = await connections.socket(endpoint);
    const compatibility = await assertHerdrCompatibility({
      endpoint,
      connections,
      binary,
    });
    const notifications = [];
    events = new HerdrEvents({
      getConnections: () => connections,
      send: (_, event) =>
        notifications.push({ ...event, received: performance.now() }),
    });
    events.subscribe(endpoint, "bench");
    await waitFor(
      () => notifications.some((event) => event.type === "connected"),
      "subscription",
    );
    const createdAt = performance.now();
    const created = await request(forwarded, "workspace.create", {
      label: "Benchmark",
      cwd: directory,
      focus: false,
      env: { SUSHIAI_BENCH_ENV: "value 🍣 日本語" },
    });
    const workspaceId = created.workspace.workspace_id;
    const paneId = created.root_pane.pane_id;
    await waitFor(
      () => notifications.some((event) => event.event === "workspace_created"),
      "native event",
    );
    const eventMs =
      notifications.find((event) => event.event === "workspace_created")
        .received - createdAt;
    const split = await request(forwarded, "pane.split", {
      target_pane_id: paneId,
      direction: "right",
      focus: false,
      env: { SUSHIAI_BENCH_ENV: "value 🍣 日本語" },
    });
    const splitId = split.pane.pane_id;
    await sleep(200);
    const paneProbe = async (id) => {
      await request(forwarded, "pane.send_input", {
        pane_id: id,
        text: 'printf \'ENV:%s PID:%s\\n\' "$SUSHIAI_BENCH_ENV" "$$"',
      });
      await request(forwarded, "pane.send_input", {
        pane_id: id,
        keys: ["Enter"],
      });
      let text = "";
      await waitFor(async () => {
        text = (
          await request(forwarded, "pane.read", {
            pane_id: id,
            source: "recent",
            format: "text",
            strip_ansi: true,
          })
        ).read.text;
        return text.includes("ENV:value 🍣 日本語 PID:");
      }, "env and Unicode");
      return /ENV:value 🍣 日本語 PID:(\d+)/.exec(text)[1];
    };
    const pid = await paneProbe(paneId);
    await paneProbe(splitId);
    let terminal = "";
    stream = await openHerdrStream({
      endpoint,
      panelId: "bench",
      target: paneId,
      cols: 80,
      rows: 24,
      connections,
      binary,
      send: (_, packet) => (terminal += packet.data),
    });
    await waitFor(
      () => terminal.includes("ENV:value"),
      "terminal stream image",
    );
    stream.proc.write("printf 'STREAM:%s\\n' '🍣 日本語'\r");
    await waitFor(async () => {
      const text = (
        await request(forwarded, "pane.read", {
          pane_id: paneId,
          source: "recent",
          format: "text",
          strip_ansi: true,
        })
      ).read.text;
      return text.includes("STREAM:🍣 日本語");
    }, "terminal stream Unicode input");
    await waitFor(() => terminal.includes("🍣"), "Unicode stream output");
    assert.equal(terminal.includes("\ufffd"), false);
    stream.proc.kill();
    await waitFor(() => stream.exited, "stream release");
    stream = null;
    events.close();
    events = null;
    if (transport === "ssh") await connections.disconnect(endpoint);
    await sleep(100);
    forwarded = await connections.socket(endpoint);
    const reattached = await request(forwarded, "session.snapshot");
    assert.ok(
      reattached.snapshot.panes.some((pane) => pane.pane_id === paneId),
    );
    assert.equal(
      await paneProbe(paneId),
      pid,
      "release/reconnect must preserve shell PID",
    );
    const missing = {};
    for (const [method, params] of [
      ["workspace.close", { workspace_id: "w999999" }],
      ["pane.split", { target_pane_id: "w999999:p999999", direction: "right" }],
    ]) {
      try {
        await request(forwarded, method, params);
        assert.fail("missing ID accepted");
      } catch (error) {
        missing[method] = { code: error.code, message: error.message };
      }
    }
    assert.equal(missing["workspace.close"].code, "workspace_not_found");
    assert.equal(missing["pane.split"].code, "pane_not_found");
    let lifecycle;
    if (process.env.SUSHIAI_HERDR_LAUNCH_SOURCE) {
      await request(forwarded, "workspace.close", {
        workspace_id: workspaceId,
      });
      const { SessionLauncher } = require(
        path.join(
          process.env.SUSHIAI_HERDR_LAUNCH_SOURCE,
          "electron/session-launch.cjs",
        ),
      );
      const { Connections: LaunchConnections } = require(
        path.join(
          process.env.SUSHIAI_HERDR_LAUNCH_SOURCE,
          "electron/connections.cjs",
        ),
      );
      const launchConnections = new LaunchConnections(
        path.join(directory, "launch-app"),
      );
      launchConnections.args = connections.args.bind(connections);
      await launchConnections.init();
      launchConnections.profiles = connections.profiles;
      const inspectHost = launchConnections.inspect.bind(launchConnections);
      launchConnections.inspect = async (host, options) => {
        try {
          return await inspectHost(host, options);
        } catch (error) {
          error.message = `${options.operation} at ${options.root || "host"}: ${error.message}`;
          throw error;
        }
      };
      const checkout = path.join(directory, "checkout");
      await fs.mkdir(checkout);
      await run("git", ["-C", checkout, "init", "-b", "main"]);
      await run("git", [
        "-C",
        checkout,
        "-c",
        "user.name=Benchmark",
        "-c",
        "user.email=benchmark@example.invalid",
        "commit",
        "--allow-empty",
        "-m",
        "Initial benchmark checkout",
      ]);
      const alias = path.join(directory, "checkout-alias");
      await fs.symlink(checkout, alias);
      const stubBin = path.join(directory, "stub-bin");
      await fs.mkdir(stubBin);
      await fs.writeFile(
        path.join(stubBin, "claude"),
        '#!/bin/sh\nprintf "AGENT:%s\\n" "$SUSHIAI_BENCH_ENV"\n',
        { mode: 0o700 },
      );
      const launchOptions = {
        getConnections: () => launchConnections,
        journalPath: path.join(directory, "session-journal.json"),
        modelProviders: {
          resolveEnv: async () => ({
            key: "synthetic-benchmark-key",
            settings: {
              SUSHIAI_BENCH_ENV: "service 🍣 日本語",
              PATH: `${stubBin}:/usr/bin:/bin`,
            },
          }),
        },
        checkCompatibility: (value) =>
          assertHerdrCompatibility({
            endpoint: value,
            connections: launchConnections,
            binary,
          }),
      };
      const launcher = new SessionLauncher(launchOptions);
      try {
        const startedAt = performance.now();
        const intentions = Array.from({ length: 20 }, (_, i) => ({
          operationId: `bench-op-${i}`,
          endpoint,
          kind: "terminal",
          cwd: i % 2 ? alias : checkout,
          label: "Concurrent benchmark",
          modelProfileId: "synthetic-probe",
        }));
        const launched = await Promise.all(
          intentions.map((input) => launcher.launch(input)),
        );
        assert.ok(
          launched.every((result) => result.ok),
          JSON.stringify(launched),
        );
        const launchMs = performance.now() - startedAt;
        const workspaceIds = new Set(
          launched.map((result) => result.value.workspaceId),
        );
        const paneIds = new Set(launched.map((result) => result.value.paneId));
        assert.equal(workspaceIds.size, 1);
        assert.equal(paneIds.size, 20);
        const retried = await Promise.all(
          Array.from({ length: 20 }, () => launcher.launch(intentions[0])),
        );
        assert.ok(
          retried.every(
            (result) =>
              result.ok && result.value.paneId === launched[0].value.paneId,
          ),
        );
        const snap = await request(forwarded, "session.snapshot");
        assert.equal(
          snap.snapshot.panes.filter(
            (pane) => pane.workspace_id === launched[0].value.workspaceId,
          ).length,
          20,
        );
        const restartedLauncher = new SessionLauncher(launchOptions);
        const restartRepeat = await restartedLauncher.launch(intentions[0]);
        assert.ok(restartRepeat.ok, JSON.stringify(restartRepeat));
        assert.equal(restartRepeat.value.paneId, launched[0].value.paneId);
        const worktree = await launcher.launch({
          operationId: "bench-worktree",
          endpoint,
          kind: "terminal",
          cwd: checkout,
          label: "Worktree benchmark",
          modelProfileId: "synthetic-probe",
          worktree: { branch: "bench-worktree" },
        });
        assert.ok(worktree.ok, JSON.stringify(worktree));
        assert.notEqual(
          worktree.value.workspaceId,
          launched[0].value.workspaceId,
        );
        for (const target of [
          launched[0].value.paneId,
          worktree.value.paneId,
        ]) {
          await request(forwarded, "pane.send_input", {
            pane_id: target,
            text: "printf 'SERVICE:%s\\n' \"$SUSHIAI_BENCH_ENV\"",
          });
          await request(forwarded, "pane.send_input", {
            pane_id: target,
            keys: ["Enter"],
          });
          await waitFor(
            async () =>
              (
                await request(forwarded, "pane.read", {
                  pane_id: target,
                  source: "recent",
                  format: "text",
                  strip_ansi: true,
                })
              ).read.text.includes("SERVICE:service 🍣 日本語"),
            "service worktree env",
          );
        }
        let failAgentPreparation = true;
        const faultInput = {
          operationId: "bench-agent-retry",
          endpoint,
          kind: "agent",
          agent: "claude",
          cwd: checkout,
          label: "Agent retry benchmark",
          modelProfileId: "synthetic-probe",
        };
        const faultLauncher = new SessionLauncher({
          ...launchOptions,
          rpc: async (socket, method, params) => {
            if (method === "pane.send_input" && failAgentPreparation) {
              failAgentPreparation = false;
              throw Object.assign(
                new Error("Synthetic agent preparation failure"),
                { code: "SYNTHETIC_PREPARATION_FAILURE" },
              );
            }
            return request(socket, method, params);
          },
        });
        const failed = await faultLauncher.launch(faultInput);
        assert.equal(failed.ok, false);
        assert.ok(failed.error.created, JSON.stringify(failed));
        const failedPaneId = failed.error.created.paneId;
        const countBeforeRetry = (await request(forwarded, "session.snapshot"))
          .snapshot.panes.length;
        const recovered = await new SessionLauncher(launchOptions).launch(
          faultInput,
        );
        assert.ok(recovered.ok, JSON.stringify(recovered));
        assert.equal(recovered.value.paneId, failedPaneId);
        assert.equal(
          (await request(forwarded, "session.snapshot")).snapshot.panes.length,
          countBeforeRetry,
        );
        await waitFor(
          async () =>
            (
              await request(forwarded, "pane.read", {
                pane_id: failedPaneId,
                source: "recent",
                format: "text",
                strip_ansi: true,
              })
            ).read.text.includes("AGENT:service 🍣 日本語"),
          "agent env after preparation retry",
        );
        const agentWorktree = await launcher.launch({
          operationId: "bench-agent-worktree",
          endpoint,
          kind: "agent",
          agent: "claude",
          cwd: checkout,
          label: "Agent worktree benchmark",
          modelProfileId: "synthetic-probe",
          worktree: { branch: "bench-agent-worktree" },
        });
        assert.ok(agentWorktree.ok, JSON.stringify(agentWorktree));
        await waitFor(
          async () =>
            (
              await request(forwarded, "pane.read", {
                pane_id: agentWorktree.value.paneId,
                source: "recent",
                format: "text",
                strip_ansi: true,
              })
            ).read.text.includes("AGENT:service 🍣 日本語"),
          "agent worktree env",
        );
        const journal = JSON.parse(
          await fs.readFile(launchOptions.journalPath, "utf8"),
        );
        assert.equal(
          JSON.stringify(journal).includes("synthetic-benchmark-key"),
          false,
        );
        for (const record of journal.operations) {
          if (record.settingsPath) {
            await fs.unlink(record.settingsPath).catch(() => {});
            await fs
              .unlink(record.settingsPath.replace(/\.json$/, ".key"))
              .catch(() => {});
          }
        }
        lifecycle = {
          distinctOperations: 20,
          repeatedOperationCalls: 20,
          workspaceCount: workspaceIds.size,
          paneCount: paneIds.size,
          launchMs,
          aliasMatchesCanonicalCheckout: true,
          worktreeRemainsSeparate: true,
          envReachesCheckoutAndWorktree: true,
          completedOperationSurvivesServiceRestart: true,
          preparationRetryAfterServiceRestartUsesCreatedPane: true,
          envReachesAgentAndAgentWorktree: true,
          noSyntheticSecretInJournal: true,
        };
        for (const id of [
          launched[0].value.workspaceId,
          worktree.value.workspaceId,
          agentWorktree.value.workspaceId,
        ])
          await request(forwarded, "workspace.close", { workspace_id: id });
      } finally {
        await launchConnections.close();
      }
    }
    const sequential = [];
    for (let i = 0; i < samples; i++) {
      const start = performance.now();
      await request(forwarded, "session.snapshot");
      sequential.push(performance.now() - start);
      await sleep(2 + (i % 7));
    }
    const concurrent = [];
    await Promise.all(
      Array.from({ length: 8 }, async () => {
        for (let i = 0; i < Math.ceil(samples / 8); i++) {
          const start = performance.now();
          await request(forwarded, "session.snapshot");
          concurrent.push(performance.now() - start);
          await sleep(2 + (i % 7));
        }
      }),
    );
    const before = await cpuTicks(daemonPid),
      idleStart = performance.now();
    await sleep(idleMs);
    const idleElapsedMs = performance.now() - idleStart,
      after = await cpuTicks(daemonPid);
    const ticksPerSecond = Number((await run("getconf", ["CLK_TCK"])).trim());
    if (!lifecycle)
      await request(forwarded, "workspace.close", {
        workspace_id: workspaceId,
      });
    return {
      binary,
      sha256: createHash("sha256")
        .update(await fs.readFile(binary))
        .digest("hex"),
      transport,
      transportScope:
        transport === "ssh"
          ? "real encrypted SSH over loopback; no WAN latency"
          : "Unix domain socket",
      compatibility,
      eventMs,
      missing,
      lifecycle,
      preservedShellPid: pid,
      sequentialMs: percentiles(sequential),
      eightClientsMs: percentiles(concurrent),
      daemonIdle: {
        elapsedMs: idleElapsedMs,
        cpuMs: ((after - before) / ticksPerSecond) * 1000,
        percentOneCore:
          ((((after - before) / ticksPerSecond) * 1000) / idleElapsedMs) * 100,
      },
      checks: [
        "daemon and CLI separately compatible",
        "create and split env",
        "Unicode stream input and output",
        "stream release and SSH disconnect preserve shell PID",
        "full snapshot after SSH reconnect",
        "structured vanished codes",
      ],
    };
  } finally {
    stream?.proc.kill();
    events?.close();
    await connections.close();
    if (daemon && daemon.exitCode === null) {
      await request(socket, "server.stop", {}, 2000).catch(() => daemon.kill());
      await waitFor(() => daemon.exitCode !== null, "daemon stop", 5000).catch(
        () => daemon.kill(),
      );
    }
    await fs.rm(directory, { recursive: true, force: true });
  }
}

(async () => {
  const binaries = process.argv.slice(2);
  assert.ok(binaries.length, "Pass one or more absolute Herdr binary paths");
  const result = {
    recordedAt: new Date().toISOString(),
    platform: process.platform,
    architecture: process.arch,
    cpu: os.cpus()[0]?.model,
    samples,
    idleMs,
    measurements: [],
  };
  for (const binary of binaries) {
    result.measurements.push(await measure(binary, "local"));
    if (
      process.env.SUSHIAI_HERDR_BENCH_SSH_KEY &&
      process.env.SUSHIAI_HERDR_BENCH_SSH_PORT
    )
      result.measurements.push(await measure(binary, "ssh"));
  }
  console.log(JSON.stringify(result, null, 2));
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
