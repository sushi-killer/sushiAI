import { useCallback, useEffect, useRef, useState } from "react";
import type { ModuleShell } from "../extensions/modules.ts";
import type { Workspace } from "../types.ts";
import { ORCHESTRATOR_EXTENSION_ID } from "./enabled.ts";
import {
  ORCHESTRATION_SURFACE,
  orchestratorTarget,
  type TaskTarget,
} from "./notices.ts";
import { setTaskOpener } from "./moduleAttention.ts";
import { publishReveal } from "./reveal.ts";
import { setWorkspaceRepos } from "./workspaceRepos.ts";

/** What the open path asks the shell for: the surface in a workspace (revealed
 * when it is already there, added otherwise), or a workspace for a folder that
 * has none yet. */
export type OpenStep =
  | { kind: "surface"; workspaceId: string }
  | { kind: "workspace"; cwd: string; name: string };

export function openStep(
  workspaces: Workspace[],
  target: TaskTarget,
): OpenStep {
  const place = orchestratorTarget(workspaces, target.repo, target.host);
  return place.kind === "create-workspace"
    ? { kind: "workspace", cwd: place.cwd, name: place.name }
    : { kind: "surface", workspaceId: place.workspaceId };
}

/** The open path a desktop-mascot Open button, a native notification click and
 * the Inbox all go through: reveal the task, then bring its pane (adding the
 * pane or the workspace when missing) to the front. */
export function useOrchestratorShell(
  enabled: boolean,
  shell: ModuleShell,
): void {
  const [pending, setPending] = useState<TaskTarget | null>(null);
  const kicked = useRef("");
  const { workspaces, openSurface } = shell;
  useEffect(() => setWorkspaceRepos(workspaces), [workspaces]);

  const openTask = useCallback((target: TaskTarget) => {
    publishReveal(target);
    kicked.current = "";
    setPending(target);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    setTaskOpener(openTask);
    const off = window.bridge?.onOrchestratorOpen(openTask);
    return () => {
      setTaskOpener(null);
      off?.();
    };
  }, [enabled, openTask]);

  useEffect(() => {
    if (!pending) return;
    const step = openStep(workspaces, pending);
    if (step.kind === "surface") {
      openSurface(
        { workspaceId: step.workspaceId },
        ORCHESTRATOR_EXTENSION_ID,
        ORCHESTRATION_SURFACE,
        {},
      );
      setPending(null);
      return;
    }
    // Once the workspace exists the next pass finds it and adds the pane.
    if (kicked.current === step.cwd) return;
    kicked.current = step.cwd;
    openSurface(
      { cwd: step.cwd, name: step.name },
      ORCHESTRATOR_EXTENSION_ID,
      ORCHESTRATION_SURFACE,
      {},
    );
  }, [pending, workspaces, openSurface]);
}
