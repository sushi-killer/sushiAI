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
  assert.equal(
    DIALOG_META["workspace-actions"].className,
    "workspace-actions-modal claude-controls-modal",
  );
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
    "a workspace removed by a Herdr snapshot closes its dialog",
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
