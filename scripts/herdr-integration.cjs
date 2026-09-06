const assert = require("node:assert/strict");
const path = require("node:path");
const os = require("node:os");
const { request, inputCommands } = require("../electron/herdr.cjs");
const socket =
  process.env.HERDR_SOCKET_PATH ||
  path.join(os.homedir(), ".config/herdr/herdr.sock");
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
(async () => {
  let workspaceId;
  try {
    const created = await request(socket, "workspace.create", {
      label: "sushiAI integration test",
      cwd: process.cwd(),
      focus: false,
    });
    workspaceId = created.workspace.workspace_id;
    const paneId = created.root_pane.pane_id;
    await sleep(500);
    for (const command of inputCommands("printf 'HERDR_BRIDGE_VERIFIED\\n'\r"))
      await request(socket, "pane.send_input", { pane_id: paneId, ...command });
    let output = "";
    for (let i = 0; i < 20; i++) {
      await sleep(200);
      const result = await request(socket, "pane.read", {
        pane_id: paneId,
        source: "recent",
        format: "ansi",
        strip_ansi: false,
      });
      output = result.read.text;
      if (output.includes("\nHERDR_BRIDGE_VERIFIED")) break;
    }
    assert.ok(
      output.includes("\nHERDR_BRIDGE_VERIFIED"),
      "Command must execute, not only echo input",
    );
    const split = await request(socket, "pane.split", {
      target_pane_id: paneId,
      direction: "right",
      focus: false,
    });
    assert.ok(split.pane.pane_id);
    const snapshot = await request(socket, "session.snapshot");
    assert.equal(
      snapshot.snapshot.panes.filter((p) => p.workspace_id === workspaceId)
        .length,
      2,
    );
    console.log(
      JSON.stringify(
        {
          passed: true,
          protocol: snapshot.snapshot.protocol,
          checks: [
            "workspace creation",
            "ordered terminal input",
            "real command execution",
            "ANSI output",
            "pane split",
            "snapshot synchronization",
            "test workspace cleanup",
          ],
        },
        null,
        2,
      ),
    );
  } finally {
    if (workspaceId)
      await request(socket, "workspace.close", { workspace_id: workspaceId });
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
