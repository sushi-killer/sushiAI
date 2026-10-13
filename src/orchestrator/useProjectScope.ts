import { useEffect, useMemo, useState } from "react";
import { LOCAL } from "./client";
import { useOrchestratorHost } from "./hostContext";
import { projectSources, type ProjectSource } from "./projectScope";
import { useOrchestratorHosts } from "./useHosts";

/** The sources (host + folder) of the project that folder `cwd` belongs to,
 * the one being viewed first. Just the folder itself until the project
 * resolves. */
export function useProjectScope(cwd: string): {
  sources: ProjectSource[];
  /** The display name of a host id. */
  hostName(host: string): string;
} {
  const host = useOrchestratorHost();
  const { hosts } = useOrchestratorHosts();
  const [folders, setFolders] = useState<
    { endpoint: string; cwd: string }[] | undefined
  >();
  useEffect(() => {
    let cancelled = false;
    setFolders(undefined);
    window.bridge
      ?.projectsResolve({
        endpoint: host === LOCAL ? "local" : host,
        cwd,
      })
      .then((project) => {
        if (!cancelled) setFolders(project?.folders);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [cwd, host]);
  const hostIds = useMemo(() => hosts.map((h) => h.id), [hosts]);
  // A stable key so the sources only change when their content does.
  const sources = useMemo(
    () => projectSources({ folders }, { host, repo: cwd }, hostIds),
    [folders, host, cwd, hostIds],
  );
  const key = sources.map((s) => `${s.host}\n${s.repo}`).join("\n\n");
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const stable = useMemo(() => sources, [key]);
  const hostName = (id: string) => hosts.find((h) => h.id === id)?.name ?? id;
  return { sources: stable, hostName };
}
