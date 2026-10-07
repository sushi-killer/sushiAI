import { useMemo } from "react";
import type { CoreViewProps } from "../extensions/coreViews.ts";
import { openSettings } from "../lib/openSettings.ts";
import { OrchestratorPanel } from "./OrchestratorPanel";
import { parseView } from "./paneArgs.ts";

/** The Orchestrator pane: maps the pane's args {view, host, repo} onto the
 * panel's own props and writes its changes back. */
export function OrchestrationView({
  args,
  cwd,
  connection,
  onArgs,
}: CoreViewProps) {
  const view = useMemo(() => parseView(args.view), [args.view]);
  return (
    <OrchestratorPanel
      cwd={cwd}
      endpoint={connection}
      view={view}
      host={args.host}
      repo={args.repo}
      onViewChange={(next) => onArgs({ view: next && JSON.stringify(next) })}
      onAddHost={() => openSettings("connections")}
      onHostChange={({ host, repo }) => onArgs({ host, repo })}
    />
  );
}
