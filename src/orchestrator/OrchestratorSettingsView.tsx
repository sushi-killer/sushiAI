import { Suspense, lazy } from "react";

// Large and rarely opened: kept out of the initial chunk, like the other
// settings screens.
const Settings = lazy(() =>
  import("./OrchestratorSettings").then(
    ({ OrchestratorSettings: Component }) => ({ default: Component }),
  ),
);

/** The Settings tab the orchestrator contributes. */
export function OrchestratorSettingsView() {
  return (
    <Suspense fallback={<div className="loading">Loading settings…</div>}>
      <Settings />
    </Suspense>
  );
}
