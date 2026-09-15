import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import type { Workspace } from "../types";

/** Turns the many equivalent spellings of the same git remote
 * (`git@host:org/repo.git`, `ssh://git@host/org/repo`, `https://host/org/repo/`)
 * into one comparable key, so "the same project checked out twice" can be
 * detected regardless of which URL form either checkout happens to use. */
export function normalizeRemote(url: string): string {
  if (!url) return "";
  let value = url.trim();
  value = value.replace(/^git@([^:]+):/, "https://$1/");
  value = value.replace(/^ssh:\/\/(?:[^@/]+@)?/, "https://");
  value = value.replace(/^https?:\/\//, "");
  value = value.replace(/\.git$/i, "");
  value = value.replace(/\/+$/, "");
  return value.toLowerCase();
}

/** A workspace's git identity. `commonDir` is shared by every worktree of
 * one repository and by nothing else; `checkout` is the worktree root the
 * workspace's cwd sits in, and `subdir` the cwd relative to it. All fields
 * are "" outside a git repository. */
export type ProjectGit = {
  remote: string;
  commonDir: string;
  checkout: string;
  subdir: string;
  branch: string;
};
const NO_GIT: ProjectGit = {
  remote: "",
  commonDir: "",
  checkout: "",
  subdir: "",
  branch: "",
};

function inspectInto(
  workspace: Pick<Workspace, "id" | "cwd" | "connection">,
  setProjectGit: Dispatch<SetStateAction<Record<string, ProjectGit>>>,
) {
  if (!window.bridge) return;
  window.bridge
    .projectInspect(workspace.connection, {
      operation: "git_remote",
      root: workspace.cwd,
    })
    .then((result) => {
      const next: ProjectGit = {
        remote: normalizeRemote(result?.remote || ""),
        commonDir: result?.commonDir || "",
        checkout: result?.checkout || "",
        subdir: result?.subdir || "",
        branch: result?.branch || "",
      };
      setProjectGit((current) =>
        JSON.stringify(current[workspace.id]) === JSON.stringify(next)
          ? current
          : { ...current, [workspace.id]: next },
      );
    })
    // A failed refresh keeps what an earlier read already knew.
    .catch(() =>
      setProjectGit((current) =>
        current[workspace.id]
          ? current
          : { ...current, [workspace.id]: NO_GIT },
      ),
    );
}

/** Fetches each workspace's git identity once per id+cwd, so the sidebar can
 * tell when two workspaces are the same project checked out in two places.
 * The branch is the one field that changes under a running workspace, so the
 * active workspace - where a checkout actually gets switched - is re-read on
 * a slow timer instead of every workspace on every poll. */
export function useProjectGit(workspaces: Workspace[], activeId: string) {
  const [projectGit, setProjectGit] = useState<Record<string, ProjectGit>>({});
  const requested = useRef(new Set<string>());
  useEffect(() => {
    if (!window.bridge) return;
    for (const w of workspaces) {
      if (!w.cwd) continue;
      const key = `${w.id}:${w.cwd}`;
      if (requested.current.has(key)) continue;
      requested.current.add(key);
      inspectInto(w, setProjectGit);
    }
  }, [workspaces]);
  const active = workspaces.find((w) => w.id === activeId);
  const activeCwd = active?.cwd;
  const activeConnection = active?.connection;
  useEffect(() => {
    if (!activeCwd) return;
    const workspace = {
      id: activeId,
      cwd: activeCwd,
      connection: activeConnection,
    };
    const timer = setInterval(
      () => inspectInto(workspace, setProjectGit),
      15000,
    );
    return () => clearInterval(timer);
  }, [activeId, activeCwd, activeConnection]);
  return projectGit;
}
