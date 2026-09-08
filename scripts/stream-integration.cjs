const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const { Connections, quote } = require("../electron/connections.cjs");
const {
  openHerdrStream,
  claudeForeground,
} = require("../electron/terminal-stream.cjs");
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
    // A real mouse-aware TUI behind Herdr: verify coordinates and step counts.
    const fixture = [
      "import os,sys,tty,termios,select,time",
      "old=termios.tcgetattr(0)",
      "tty.setraw(0)",
      "os.write(1,b'\\x1b[?1000h\\x1b[?1006hMOUSE_READY')",
      "data=b''",
      "deadline=time.time()+8",
      "while time.time()<deadline:",
      " if select.select([0],[],[],0.1)[0]:",
      "  data+=os.read(0,4096)",
      "  if data.endswith(b'm'): break",
      "termios.tcsetattr(0,termios.TCSANOW,old)",
      "os.write(1,b'\\x1b[?1000l\\x1b[?1006l\\r\\nMOUSE_HEX:'+data.hex().encode()+b'\\r\\n')",
    ].join("\n");
    output = "";
    stream.proc.write(
      `python3 -c "import base64;exec(base64.b64decode('${Buffer.from(fixture).toString("base64")}'))"\r`,
    );
    await until(() => output.includes("MOUSE_READY"), "mouse fixture ready");
    await sleep(200);
    await stream.proc.scroll("up", 6, { column: 12, row: 5 });
    await stream.proc.scroll("up", 6, { column: 12, row: 5, fast: true });
    const click = "\x1b[<0;13;6M\x1b[<0;13;6m";
    stream.proc.write(click);
    const expectedMouse = Buffer.from(
      "\x1b[<64;13;6M".repeat(4) + click,
    ).toString("hex");
    await until(
      () =>
        output
          .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
          .replace(/\s/g, "")
          .includes("MOUSE_HEX:" + expectedMouse),
      "Herdr mouse coordinates, one normal wheel event plus three Alt steps and button click",
    );
    // Reproduce the dangerous fallback: a Claude-shaped foreground process
    // temporarily has mouse reporting disabled while it redraws. Host history
    // must not move; the application must still receive all four wheel reports.
    output = "";
    const redrawFixture = `
      process.title = 'claude';
      process.stdin.setRawMode(true);
      process.stdout.write('\\x1b[?1000l\\x1b[?1006lMOUSE_READY');
      let data = Buffer.alloc(0);
      const timer = setTimeout(() => process.exit(1), 8000);
      process.stdin.on('data', chunk => {
        data = Buffer.concat([data, chunk]);
        if (data.at(-1) !== 109) return;
        clearTimeout(timer);
        process.stdin.setRawMode(false);
        process.stdout.write('\\r\\nMOUSE_HEX:' + data.toString('hex') + '\\r\\n');
        process.exit(0);
      });
    `;
    stream.proc.write("node -e " + quote(redrawFixture) + "\r");
    await until(
      () => output.includes("MOUSE_READY"),
      "Claude redraw fixture ready",
    );
    await sleep(200);
    const redrawProcess = await request(socket, "pane.process_info", {
      pane_id: pane,
    });
    assert.equal(
      claudeForeground(redrawProcess),
      true,
      JSON.stringify(redrawProcess),
    );
    await stream.proc.scroll("up", 6, { column: 12, row: 5 });
    await stream.proc.scroll("up", 6, { column: 12, row: 5, fast: true });
    stream.proc.write(click);
    await until(
      () =>
        output
          .replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
          .replace(/\s/g, "")
          .includes("MOUSE_HEX:" + expectedMouse),
      "Claude receives accelerated scroll even while mouse reporting is disabled",
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
        output.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").includes("MOUSE_HEX:"),
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
            "mouse coordinates / native wheel / click",
            "Claude Alt scroll during disabled mouse reporting",
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
