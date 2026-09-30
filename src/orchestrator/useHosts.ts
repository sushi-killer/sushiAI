import { useCallback, useEffect, useState } from "react";
import type { OrchestratorHost } from "./types";

const LOCAL_ONLY: OrchestratorHost[] = [
  { id: "local", name: "Local", state: "ready", enabled: true },
];

/** The hosts the Orchestrator can talk to (Local + each SSH profile), kept
 * current as the main process connects, installs or loses one. */
export function useOrchestratorHosts(): {
  hosts: OrchestratorHost[];
  refresh(): void;
} {
  const [hosts, setHosts] = useState<OrchestratorHost[]>(LOCAL_ONLY);
  const refresh = useCallback(() => {
    window.bridge
      ?.orchestratorHosts()
      .then(setHosts)
      .catch(() => {});
  }, []);
  useEffect(() => {
    refresh();
    return window.bridge?.onOrchestratorHosts(refresh);
  }, [refresh]);
  return { hosts, refresh };
}
