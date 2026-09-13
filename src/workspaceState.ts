import type { Panel, Workspace } from "./types";
import { contains, leaf, split, uid } from "./layout.ts";
import { validRoute, type RouteRef } from "./extensions/routes.ts";

export const STORAGE = "sushiai.v1";

export type Routine = { id: string; name: string; command: string };

export type Saved = {
  workspaces: Workspace[];
  activeId: string;
  socket: string;
  routines: Routine[];
  fontScale: number;
  mode?: "Agent" | "Code" | "Chat";
  tabMode?: boolean;
  section?: string;
  selected?: string;
  zoomed?: string | null;
  sidebar?: boolean;
  route?: RouteRef;
};

export type WorkspaceStorage = {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
};

export function restore(
  storage?: Pick<WorkspaceStorage, "getItem">,
): Saved | null {
  try {
    const value = JSON.parse(
      (storage || localStorage).getItem(STORAGE) || "null",
    );
    if (!Array.isArray(value?.workspaces) || !value.workspaces.length)
      return null;
    const mode =
      value.mode === "Agent" || value.mode === "Chat" ? value.mode : "Code";
    return {
      ...value,
      mode,
      tabMode: value.tabMode === true,
      section: typeof value.section === "string" ? value.section : "",
      selected: typeof value.selected === "string" ? value.selected : "",
      zoomed: typeof value.zoomed === "string" ? value.zoomed : null,
      sidebar: typeof value.sidebar === "boolean" ? value.sidebar : undefined,
      route: validRoute(value.route) ? value.route : undefined,
      workspaces: value.workspaces.map((w: Workspace) => ({
        ...w,
        connection: w.herdrId ? w.connection || value.socket : undefined,
        panels: w.panels.map((p) => ({
          ...p,
          busy: false,
          started: false,
          // A reply that never arrived leaves an empty bubble; drop it.
          messages: p.messages?.filter((m) => m.role === "user" || m.text),
        })),
      })),
    };
  } catch {
    return null;
  }
}

export function saveWorkspaceState(
  value: Saved,
  storage?: Pick<WorkspaceStorage, "setItem">,
): void {
  (storage || localStorage).setItem(STORAGE, JSON.stringify(value));
}

export function initialWorkspace(cwd = ""): Workspace {
  const panels: Panel[] = [
    { id: uid(), kind: "agent", title: "Claude Code", agent: "claude" },
    { id: uid(), kind: "terminal", title: "zsh" },
    { id: uid(), kind: "browser", title: "Browser" },
    { id: uid(), kind: "chat", title: "Thread", agent: "claude", messages: [] },
  ];
  return {
    id: uid(),
    name: "sushiai",
    cwd,
    panels,
    layout: split(
      leaf(panels[0].id),
      split(
        leaf(panels[1].id),
        split(leaf(panels[2].id), leaf(panels[3].id), "column", 0.49),
        "column",
        0.28,
      ),
      "row",
      0.53,
    ),
  };
}

export const codePanels = (w: Workspace) =>
  w.panels.filter((p) => p.kind !== "chat" || contains(w.layout, p.id));
