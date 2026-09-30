// The folders of every open workspace, per host, so the Orchestrator panel can
// suggest a repo on a remote host without the panel tree carrying the list.
import { useSyncExternalStore } from "react";
import type { Workspace } from "../types.ts";

export type WorkspaceRepo = Pick<Workspace, "cwd" | "connection">;

let repos: WorkspaceRepo[] = [];
let key = "";
const listeners = new Set<() => void>();

/** Replaces the list; unchanged content keeps the same array (no re-render). */
export function setWorkspaceRepos(workspaces: WorkspaceRepo[]): void {
  const next = workspaces.map(({ cwd, connection }) => ({ cwd, connection }));
  const nextKey = JSON.stringify(next);
  if (nextKey === key) return;
  key = nextKey;
  repos = next;
  for (const listener of [...listeners]) listener();
}

export function useWorkspaceRepos(): WorkspaceRepo[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => repos,
  );
}
