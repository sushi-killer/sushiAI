import { useEffect, useRef, useState } from "react";
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

/** Fetches each workspace's git remote (once per id+cwd, never on every
 * poll) so the sidebar can tell when two workspaces are the same project
 * checked out in two places. Unknown/non-git workspaces map to "". */
export function useProjectRemotes(workspaces: Workspace[]) {
  const [remotes, setRemotes] = useState<Record<string, string>>({});
  const requested = useRef(new Set<string>());
  useEffect(() => {
    if (!window.bridge) return;
    for (const w of workspaces) {
      if (!w.cwd) continue;
      const key = `${w.id}:${w.cwd}`;
      if (requested.current.has(key)) continue;
      requested.current.add(key);
      window.bridge
        .projectInspect(w.connection, { operation: "git_remote", root: w.cwd })
        .then((result) =>
          setRemotes((current) => ({
            ...current,
            [w.id]: normalizeRemote(result?.remote || ""),
          })),
        )
        .catch(() => setRemotes((current) => ({ ...current, [w.id]: "" })));
    }
  }, [workspaces]);
  return remotes;
}
