const { test } = require("node:test");
const assert = require("node:assert/strict");

const library = import("../src/dialogs/dialog-state.ts");

const workspace = (id, panels = []) => ({
  id,
  name: id,
  cwd: "/tmp",
  panels,
  layout: null,
});
const panel = (id) => ({ id, kind: "terminal", title: id });

test("every dialog kind has a label and a class", async () => {
  const { DIALOG_META } = await library;
  for (const [kind, meta] of Object.entries(DIALOG_META)) {
    assert.ok(meta.label, `${kind} has an aria-label`);
    assert.equal(typeof meta.className, "string");
  }
  assert.equal(DIALOG_META.pane.label, "Add panel");
  assert.equal(DIALOG_META.pane.className, "command-modal");
  assert.equal(DIALOG_META["workspace-actions"].className, "project-dialog");
});

test("a targeted dialog reads the live workspace, not a snapshot", async () => {
  const { resolveDialog } = await library;
  const before = [workspace("w1", [panel("p1")])];
  const dialog = { kind: "workspace-actions", workspaceId: "w1" };
  assert.equal(resolveDialog(dialog, before).workspace.name, "w1");

  // Renaming used to need a second setState to refresh the dialog's copy.
  const after = [{ ...before[0], name: "renamed" }];
  assert.equal(resolveDialog(dialog, after).workspace.name, "renamed");
});

test("a dialog whose target disappeared resolves to null", async () => {
  const { resolveDialog } = await library;
  const list = [workspace("w1", [panel("p1")])];
  assert.equal(
    resolveDialog({ kind: "workspace-actions", workspaceId: "gone" }, list),
    null,
    "a workspace removed by a session update closes its dialog",
  );
  assert.equal(
    resolveDialog(
      { kind: "close-session", workspaceId: "w1", panelId: "gone" },
      list,
    ),
    null,
    "a panel that already closed elsewhere closes its dialog",
  );
  assert.deepEqual(
    resolveDialog(
      { kind: "close-session", workspaceId: "w1", panelId: "p1" },
      list,
    ),
    { workspace: list[0], panel: list[0].panels[0] },
  );
  assert.equal(resolveDialog({ kind: "pane" }, list), null);
  assert.equal(resolveDialog(null, list), null);
});

test("project settings can be opened by folder when the caller has no workspace id", async () => {
  const { resolveDialog } = await import("../src/dialogs/dialog-state.ts");
  const workspaces = [
    { id: "a", cwd: "/work/a", panels: [] },
    { id: "b", cwd: "/work/b", panels: [] },
  ];
  assert.equal(
    resolveDialog(
      { kind: "workspace-actions", workspaceId: "", cwd: "/work/b" },
      workspaces,
    )?.workspace.id,
    "b",
  );
  assert.equal(
    resolveDialog(
      { kind: "workspace-actions", workspaceId: "", cwd: "/work/none" },
      workspaces,
    ),
    null,
  );
  // An id still wins over a folder.
  assert.equal(
    resolveDialog(
      { kind: "workspace-actions", workspaceId: "a", cwd: "/work/b" },
      workspaces,
    )?.workspace.id,
    "a",
  );
});

test("a folder opens the workspace on the same host, not the same path elsewhere", async () => {
  const { resolveDialog } = await library;
  const here = { ...workspace("here"), cwd: "/work/app" };
  const there = {
    ...workspace("there"),
    cwd: "/work/app",
    connection: "ssh:lab",
  };
  const dialog = (connection) => ({
    kind: "workspace-actions",
    workspaceId: "",
    cwd: "/work/app",
    connection,
  });
  assert.equal(
    resolveDialog(dialog(undefined), [there, here])?.workspace.id,
    "here",
  );
  assert.equal(
    resolveDialog(dialog("ssh:lab"), [here, there])?.workspace.id,
    "there",
  );
  assert.equal(resolveDialog(dialog("ssh:other"), [here, there]), null);
});

test("a requested project tab is used once, by its own workspace, and expires", async () => {
  const { setPendingProjectTab, takePendingTab } =
    await import("../src/lib/openSettings.ts");
  setPendingProjectTab("Hosts", "/work/app", "ssh:lab");
  assert.equal(takePendingTab("/work/app", undefined), "General"); // same path, other host
  setPendingProjectTab("Hosts", "/work/app", "ssh:lab");
  assert.equal(takePendingTab("/work/other", "ssh:lab"), "General"); // other folder
  assert.equal(takePendingTab("/work/app", "ssh:lab"), "General"); // already consumed
  setPendingProjectTab("Hosts", "/work/app", "ssh:lab");
  assert.equal(takePendingTab("/work/app", "ssh:lab"), "Hosts");
  assert.equal(takePendingTab("/work/app", "ssh:lab"), "General");
  // A request nobody picked up does not turn up on a later dialog.
  setPendingProjectTab("Hosts", "/work/app");
  assert.equal(
    takePendingTab("/work/app", undefined, Date.now() + 60000),
    "General",
  );
});
