const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const { Connections } = require("../electron/connections.cjs");
const { openHerdrStream } = require("../electron/terminal-stream.cjs");
const { request } = require("../electron/herdr.cjs");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(check, label) {
  for (let i = 0; i < 100; i++) {
    if (await check()) return;
    await sleep(100);
  }
  throw new Error("Timed out: " + label);
}
(async () => {
  const dir = await fs.mkdtemp("/tmp/sushiai-stream-test-");
  const connections = new Connections(dir);
  await connections.init();
  let workspace, stream, socket;
  try {
    const remote = process.env.SUSHIAI_SSH_HOST;
    const profile = remote
      ? await connections.save({
          host: remote,
          socket:
            process.env.SUSHIAI_SSH_SOCKET ||
            "~/.config/herdr/sessions/sushiai/herdr.sock",
        })
      : null;
    const endpoint = profile
      ? "ssh:" + profile.id
      : process.env.HERDR_SOCKET_PATH ||
        process.env.HOME + "/.config/herdr/herdr.sock";
    socket = await connections.socket(endpoint);
    const home = await connections.inspect(endpoint, { operation: "home" });
    const created = await request(socket, "workspace.create", {
      label: "sushiAI stream test",
      cwd: home.home,
      focus: false,
    });
    workspace = created.workspace.workspace_id;
    const pane = created.root_pane.pane_id;
    let output = "",
      frames = 0;
    stream = await openHerdrStream({
      endpoint,
      panelId: "test",
      target: pane,
      cols: 110,
      rows: 35,
      connections,
      binary: process.env.HOME + "/.local/bin/herdr",
      send: (_channel, event) => {
        output += event.data || "";
        frames++;
      },
    });
    await until(() => frames > 0, "first streamed frame");
    await sleep(400);
    let start = Date.now();
    stream.proc.write("printf 'STREAM_%s\\n' VERIFIED\r");
    await until(
      () =>
        output
          .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
          .includes("STREAM_VERIFIED"),
      "raw input / streamed output",
    );
    const latency = Date.now() - start;
    output = "";
    stream.proc.resize(123, 41);
    await sleep(300);
    stream.proc.write("stty size\r");
    await until(async () => {
      const r = await request(socket, "pane.read", {
        pane_id: pane,
        source: "recent",
        format: "text",
        strip_ansi: true,
      });
      return /41\s+123/.test(r.read.text);
    }, "PTY resize").catch((e) => {
      console.error(
        output.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").slice(-1000),
      );
      throw e;
    });
    output = "";
    stream.proc.write("printf 'EDIT_%s\\n' OKx\x7f\r");
    await until(
      () => output.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").includes("EDIT_OK"),
      "backspace input",
    );
    stream.proc.kill();
    await until(() => stream.exited, "release lease");
    output = "";
    frames = 0;
    stream = await openHerdrStream({
      endpoint,
      panelId: "test",
      target: pane,
      cols: 100,
      rows: 30,
      connections,
      binary: process.env.HOME + "/.local/bin/herdr",
      send: (_, event) => {
        output += event.data || "";
        frames++;
      },
    });
    await until(
      () =>
        output
          .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
          .includes("STREAM_VERIFIED"),
      "reattach retains terminal content",
    );
    await request(socket, "pane.close", { pane_id: pane });
    await until(() => stream.exited, "session close reaches client");
    console.log(
      JSON.stringify(
        {
          passed: true,
          remote: remote || false,
          latencyMs: latency,
          checks: [
            "raw streaming",
            "input/backspace",
            "real PTY resize",
            "release/reattach",
            "pane closure",
          ],
        },
        null,
        2,
      ),
    );
  } finally {
    stream?.proc.kill();
    if (workspace)
      await request(socket, "workspace.close", {
        workspace_id: workspace,
      }).catch(() => {});
    await connections.close();
    await fs.rm(dir, { recursive: true, force: true });
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
