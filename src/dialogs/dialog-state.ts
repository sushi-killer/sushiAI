import type { Panel, Workspace } from "../types";

export type Dialog =
  | { kind: "pane" }
  | { kind: "workspace" }
  | { kind: "settings" }
  | { kind: "notifications" }
  | { kind: "updates" }
  | { kind: "routine" }
  | { kind: "close-session"; workspaceId: string; panelId: string }
  // `cwd` stands in for an id the caller does not have (the Run on menu knows
  // only the folder): the dialog then opens for the workspace in that folder.
  | {
      kind: "workspace-actions";
      workspaceId: string;
      cwd?: string;
      connection?: string;
    };

export type DialogKind = Dialog["kind"];

export const DIALOG_META: Record<
  DialogKind,
  { label: string; className: string }
> = {
  pane: { label: "Add panel", className: "command-modal" },
  workspace: { label: "New project", className: "np-dialog" },
  settings: { label: "Settings", className: "" },
  notifications: { label: "Notifications", className: "" },
  updates: { label: "Software updates", className: "" },
  routine: { label: "New routine", className: "" },
  "close-session": { label: "Close session", className: "cs-dialog" },
  "workspace-actions": {
    label: "Workspace controls",
    className: "project-dialog",
  },
};

/** Dialogs remember their target by id and read the live object, so a rename or
 * a session update that arrives while the dialog is open is reflected instead of
 * showing a stale copy. `null` means the target is gone and the dialog should
 * close. */
export function resolveDialog(
  dialog: Dialog | null,
  workspaces: Workspace[],
): { workspace: Workspace; panel?: Panel } | null {
  if (!dialog || !("workspaceId" in dialog)) return null;
  const workspace = workspaces.find((item) =>
    dialog.workspaceId
      ? item.id === dialog.workspaceId
      : "cwd" in dialog &&
        item.cwd === dialog.cwd &&
        (item.connection ?? "") ===
          (("connection" in dialog && dialog.connection) || ""),
  );
  if (!workspace) return null;
  if (!("panelId" in dialog)) return { workspace };
  const panel = workspace.panels.find((item) => item.id === dialog.panelId);
  return panel ? { workspace, panel } : null;
}

export function dialogNeedsTarget(dialog: Dialog | null): boolean {
  return Boolean(dialog && "workspaceId" in dialog);
}
