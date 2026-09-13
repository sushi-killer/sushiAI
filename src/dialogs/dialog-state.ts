import type { Panel, Workspace } from "../types";

export type Dialog =
  | { kind: "pane" }
  | { kind: "workspace" }
  | { kind: "settings" }
  | { kind: "notifications" }
  | { kind: "updates" }
  | { kind: "routine" }
  | { kind: "sessions" }
  | { kind: "close-session"; workspaceId: string; panelId: string }
  | { kind: "workspace-actions"; workspaceId: string };

export type DialogKind = Dialog["kind"];

export const DIALOG_META: Record<
  DialogKind,
  { label: string; className: string }
> = {
  pane: { label: "Add panel", className: "command-modal" },
  workspace: { label: "New workspace", className: "" },
  settings: { label: "Settings", className: "" },
  notifications: { label: "Notifications", className: "" },
  updates: { label: "Software updates", className: "" },
  routine: { label: "New routine", className: "" },
  sessions: { label: "Session manager", className: "sessions-modal" },
  "close-session": { label: "Notifications", className: "" },
  "workspace-actions": {
    label: "Workspace controls",
    className: "workspace-actions-modal claude-controls-modal",
  },
};

/** Dialogs remember their target by id and read the live object, so a rename or
 * a Herdr snapshot that arrives while the dialog is open is reflected instead of
 * showing a stale copy. `null` means the target is gone and the dialog should
 * close. */
export function resolveDialog(
  dialog: Dialog | null,
  workspaces: Workspace[],
): { workspace: Workspace; panel?: Panel } | null {
  if (!dialog || !("workspaceId" in dialog)) return null;
  const workspace = workspaces.find((item) => item.id === dialog.workspaceId);
  if (!workspace) return null;
  if (!("panelId" in dialog)) return { workspace };
  const panel = workspace.panels.find((item) => item.id === dialog.panelId);
  return panel ? { workspace, panel } : null;
}

export function dialogNeedsTarget(dialog: Dialog | null): boolean {
  return Boolean(dialog && "workspaceId" in dialog);
}
