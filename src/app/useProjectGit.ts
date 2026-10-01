import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import type { Workspace } from "../types";
import type { SessionHostContext } from "./sessionHosts.ts";
import {
  projectGitRequestKey,
  readyProjectGitHostKeys,
  readyProjectGitWorkspaceIds,
} from "./projectGitReadiness.ts";

/** Turns the many equivalent spellings of the same git remote
 * (`git@host:org/repo.git`, `ssh://git@host/org/repo`, `https://host/org/repo/`)
 * into one comparable key, so "the same project checked out twice" can be
 * detected regardless of which URL form either checkout happens to use. */
export function normalizeRemote(url: string): string {
  if (!url) return "";
  let value = url.trim();
  value = value.replace(/^git@([^:]+):/, "https://$1/");
  value = value.replace(/^ssh:\/\/(?:[^@/]+@)?/i, "https://");
  value = value.replace(/^https?:\/\//i, "");
  // A clone over SSH names the server's SSH port and a login; the same
  // repository over HTTPS names neither, and neither identifies the repository.
  value = value.replace(/^[^@/]+@/, "");
  value = value.replace(/^([^/:]+):\d+\//, "$1/");
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
  linkedWorktree: boolean;
  subdir: string;
  branch: string;
};
const NO_GIT: ProjectGit = {
  remote: "",
  commonDir: "",
  checkout: "",
  linkedWorktree: false,
  subdir: "",
  branch: "",
};

function inspectInto(
  workspace: Pick<Workspace, "id" | "cwd" | "connection">,
  setProjectGit: Dispatch<SetStateAction<Record<string, ProjectGit>>>,
  settle: (
    workspace: Pick<Workspace, "id" | "cwd" | "connection">,
    key: string,
  ) => boolean | null,
  finished: () => void = () => {},
) {
  const key = projectGitRequestKey(workspace);
  if (!window.bridge) {
    const wasSettled = settle(workspace, key);
    if (wasSettled !== null)
      setProjectGit((current) =>
        wasSettled && current[workspace.id]
          ? current
          : { ...current, [workspace.id]: NO_GIT },
      );
    finished();
    return;
  }
  window.bridge
    .projectInspect(workspace.connection, {
      operation: "git_remote",
      root: workspace.cwd,
    })
    .then((result) => {
      if (settle(workspace, key) === null) return;
      const next: ProjectGit = {
        remote: normalizeRemote(result?.remote || ""),
        commonDir: result?.commonDir || "",
        checkout: result?.checkout || "",
        linkedWorktree: result?.linkedWorktree || false,
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
    .catch(() => {
      const wasSettled = settle(workspace, key);
      if (wasSettled === null) return;
      setProjectGit((current) =>
        wasSettled && current[workspace.id]
          ? current
          : { ...current, [workspace.id]: NO_GIT },
      );
    })
    .finally(finished);
}

/** Fetches each workspace's git identity once per id+cwd, so the sidebar can
 * tell when two workspaces are the same project checked out in two places.
 * The branch is the one field that changes under a running workspace, so the
 * active workspace - where a checkout actually gets switched - is re-read on
 * a slow timer instead of every workspace on every poll. */
export function useProjectGit(
  workspaces: Workspace[],
  activeId: string,
  onProjectGitChange?: (projectGit: Record<string, ProjectGit>) => void,
) {
  const [projectGit, setProjectGit] = useState<Record<string, ProjectGit>>({});
  const [settledKeys, setSettledKeys] = useState<Record<string, string>>({});
  const settledKeysRef = useRef(settledKeys);
  const currentKeys = new Map(
    workspaces.map((workspace) => [
      workspace.id,
      projectGitRequestKey(workspace),
    ]),
  );
  const currentKeysRef = useRef(currentKeys);
  currentKeysRef.current = currentKeys;
  settledKeysRef.current = settledKeys;
  const settle = useCallback(
    (workspace: Pick<Workspace, "id" | "cwd" | "connection">, key: string) => {
      if (currentKeysRef.current.get(workspace.id) !== key) return null;
      const wasSettled = settledKeysRef.current[workspace.id] === key;
      if (!wasSettled) {
        const next = { ...settledKeysRef.current, [workspace.id]: key };
        settledKeysRef.current = next;
        setSettledKeys(next);
      }
      return wasSettled;
    },
    [],
  );
  const requested = useRef(new Map<string, string>());
  useEffect(() => {
    for (const w of workspaces) {
      if (!w.cwd) continue;
      const key = projectGitRequestKey(w);
      if (
        settledKeysRef.current[w.id] === key ||
        requested.current.get(w.id) === key
      )
        continue;
      requested.current.set(w.id, key);
      inspectInto(w, setProjectGit, settle, () => {
        if (requested.current.get(w.id) === key) requested.current.delete(w.id);
      });
    }
  }, [workspaces, settledKeys, settle]);
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
      () => inspectInto(workspace, setProjectGit, settle),
      15000,
    );
    return () => clearInterval(timer);
  }, [activeId, activeCwd, activeConnection, settle]);
  const readyWorkspaceIds = useMemo(
    () => readyProjectGitWorkspaceIds(workspaces, settledKeys),
    [workspaces, settledKeys],
  );
  const readyHostKeys = useMemo(
    () => readyProjectGitHostKeys(workspaces, settledKeys),
    [workspaces, settledKeys],
  );
  const currentProjectGit = useMemo(
    () =>
      Object.fromEntries(
        workspaces
          .filter(
            (workspace) => readyWorkspaceIds.has(workspace.id) && workspace.cwd,
          )
          .flatMap((workspace) =>
            projectGit[workspace.id]
              ? [[workspace.id, projectGit[workspace.id]]]
              : [],
          ),
      ),
    [workspaces, readyWorkspaceIds, projectGit],
  );
  useEffect(
    () => onProjectGitChange?.(currentProjectGit),
    [currentProjectGit, onProjectGitChange],
  );
  const [hydratedHostKeys, setHydratedHostKeys] = useState<Set<string>>(
    () => new Set(),
  );
  useEffect(() => {
    setHydratedHostKeys((current) => {
      const next = new Set(current);
      for (const host of readyHostKeys) next.add(host);
      return next.size === current.size ? current : next;
    });
  }, [readyHostKeys]);
  return { projectGit: currentProjectGit, readyWorkspaceIds, hydratedHostKeys };
}

/** What the "+" picker is handed about hosts: the workspaces and their git
 * state, plus the one thing it may do on its own, open a workspace on a host
 * and close itself (a session started on a host that had no workspace). The
 * callbacks live in refs: the object changes only when the data it carries
 * does, so a re-render of the app never makes the picker look again. */
export function useHostContext(
  base: Omit<SessionHostContext, "startOnHost">,
  createWorkspace: (
    name: string,
    cwd: string,
    backend: string,
    starter: string,
    endpoint?: string,
  ) => Promise<boolean>,
  closeDialog: () => void,
): SessionHostContext {
  const { workspaces, projectGit, connectionProfiles, workspaceGrouping } =
    base;
  const latest = useRef({ createWorkspace, closeDialog });
  latest.current = { createWorkspace, closeDialog };
  const startOnHost = useCallback(
    async (name: string, cwd: string, endpoint: string, starter: string) => {
      const started = await latest.current.createWorkspace(
        name,
        cwd,
        "herdr",
        starter,
        endpoint,
      );
      if (started) latest.current.closeDialog();
      return started;
    },
    [],
  );
  return useMemo(
    () => ({
      workspaces,
      projectGit,
      connectionProfiles,
      workspaceGrouping,
      startOnHost,
    }),
    [
      workspaces,
      projectGit,
      connectionProfiles,
      workspaceGrouping,
      startOnHost,
    ],
  );
}
