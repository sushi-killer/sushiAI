const { request } = require("../electron/herdr.cjs");
const { openHerdrStream } = require("../electron/terminal-stream.cjs");
const assert = require("node:assert/strict");
const { herdrLaunchParams } = require("../electron/terminal-text.cjs");
const socket = process.env.HOME + "/.config/herdr/herdr.sock";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  let w, stream;
  try {
    const created = await request(
      socket,
      "workspace.create",
      herdrLaunchParams("workspace.create", {
        label: "Unicode diagnostic",
        cwd: "/tmp",
        focus: false,
      }),
    );
    w = created.workspace.workspace_id;
    let output = "";
    stream = await openHerdrStream({
      endpoint: socket,
      panelId: "unicode-test",
      target: created.root_pane.pane_id,
      cols: 100,
      rows: 30,
      connections: { socket: async () => socket },
      binary: process.env.HOME + "/.local/bin/herdr",
      send: (_, e) => (output += e.data || ""),
    });
    await sleep(700);
    stream.proc.write(
      'printf \'\\nLOCALE:%s|%s|%s|MB:%s\\n\' "$LANG" "$LC_ALL" "$LC_CTYPE" "$options[multibyte]"\r',
    );
    await sleep(700);
    output = "";
    for (const ch of "Привет") {
      stream.proc.write(ch);
      await sleep(60);
    }
    await sleep(500);
    assert.ok(output.length > 0);
    assert.equal(output.includes("�"), false);
    assert.equal(/<00[89]/.test(output), false);
    console.log(
      "Herdr Cyrillic typing passed with real user zsh configuration.",
    );
    stream.proc.write("\x03");
  } finally {
    stream?.proc.kill();
    if (w) await request(socket, "workspace.close", { workspace_id: w });
  }
})().catch((error) => { console.error(error.message); process.exitCode = 1; });
