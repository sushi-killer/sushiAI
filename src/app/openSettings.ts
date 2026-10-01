/** Any dialog can ask the shell to open Settings on a tab (the Hosts page
 * wants Connections, the session picker wants Providers) without the shell
 * threading a callback through every dialog. `useSettingsTab` listens. */
export const OPEN_SETTINGS_EVENT = "sushiai:open-settings";

export function openSettings(tab: "connections" | "providers"): void {
  window.dispatchEvent(new CustomEvent(OPEN_SETTINGS_EVENT, { detail: tab }));
}

/** The same for a project's settings: the Run on menu links to its Hosts
 * page. The tab waits here until the dialog mounts and takes it. */
export const OPEN_PROJECT_SETTINGS_EVENT = "sushiai:open-project-settings";
export type ProjectSettingsTab =
  "General" | "Environment" | "MCP servers" | "Hosts";
type Pending = {
  tab: ProjectSettingsTab;
  cwd: string;
  connection: string;
  at: number;
};
let pending: Pending | null = null;
/** A request nobody picked up (the workspace was gone) must not turn up on
 * some later, unrelated dialog. */
const PENDING_MS = 2000;

/** The workspace is its folder and where that folder lives: the same path on
 * this Mac and on an SSH host are two workspaces. */
export function openProjectSettings(
  cwd: string,
  tab: ProjectSettingsTab,
  connection?: string,
): void {
  window.dispatchEvent(
    new CustomEvent(OPEN_PROJECT_SETTINGS_EVENT, {
      detail: { cwd, tab, connection },
    }),
  );
}

export function setPendingProjectTab(
  tab: ProjectSettingsTab,
  cwd: string,
  connection?: string,
): void {
  pending = { tab, cwd, connection: connection ?? "", at: Date.now() };
}

/** The tab asked for this workspace's dialog, once; otherwise General. */
export function takePendingTab(
  cwd: string,
  connection?: string,
  now = Date.now(),
): ProjectSettingsTab {
  const wanted = pending;
  pending = null;
  return wanted &&
    wanted.cwd === cwd &&
    wanted.connection === (connection ?? "") &&
    now - wanted.at < PENDING_MS
    ? wanted.tab
    : "General";
}
