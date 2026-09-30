import { useEffect, useState } from "react";
import { useOrchestratorClient } from "./hostContext";
import { RECENT_DAYS, recurringTotal } from "./improvementsModel";

/** How many failures repeat in this repo's recent live tasks; the rail's
 * Improvements badge adds it to the open proposals. `refresh` changes
 * whenever the task list does. */
export function useRecurringFailures(cwd: string, refresh: string) {
  const client = useOrchestratorClient();
  const [total, setTotal] = useState(0);
  useEffect(() => {
    let cancelled = false;
    client
      .failuresCatalogue(cwd, RECENT_DAYS)
      .then((rows) => !cancelled && setTotal(recurringTotal(rows)))
      .catch(() => !cancelled && setTotal(0));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh, client]);
  return total;
}
