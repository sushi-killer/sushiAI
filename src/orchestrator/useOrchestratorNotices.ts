import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import type { PanelKind, Workspace } from "../types";
import { orchestratorTarget, toastReducer, type TaskTarget } from "./notices";
import { publishReveal } from "./reveal";

type Shell = {
  workspaces: Workspace[];
  showWorkspace(): void;
  switchWorkspace(id: string): void;
  setSelected(id: string): void;
  setZoomed(id: string | null): void;
  addPanel(
    kind: PanelKind,
    agent?: string,
    filesTarget?: undefined,
    modelProfile?: undefined,
    backend?: "herdr" | "local",
    targetWorkspaceId?: string,
  ): Promise<void>;
  createWorkspace(
    name: string,
    cwd: string,
    backend: string,
    starter: string,
  ): Promise<boolean>;
};

/** Toast stack plus the one open path both a toast's Open button and a
 * native notification click go through: reveal the task, then bring its
 * Orchestrator panel (adding the panel or the workspace when missing) to the
 * front. */
export function useOrchestratorNotices(shell: Shell) {
  const [toasts, dispatch] = useReducer(toastReducer, []);
  const [pending, setPending] = useState<TaskTarget | null>(null);
  const kicked = useRef("");

  useEffect(() => {
    const offs = [
      window.bridge?.onOrchestratorNotice((notice) =>
        dispatch({ type: "add", notice }),
      ),
      window.bridge?.onOrchestrator((event) => {
        if (event.event === "task")
          dispatch({
            type: "task",
            taskId: event.task.id,
            status: event.task.status,
          });
      }),
    ];
    return () => offs.forEach((off) => off?.());
  }, []);

  const { showWorkspace } = shell;
  const openTask = useCallback(
    (target: TaskTarget) => {
      // Leave any section page or Agent/Chat mode first; a no-op in Code mode.
      showWorkspace();
      publishReveal(target);
      kicked.current = "";
      setPending(target);
    },
    [showWorkspace],
  );

  useEffect(() => window.bridge?.onOrchestratorOpen(openTask), [openTask]);

  const { workspaces, switchWorkspace, setSelected, setZoomed } = shell;
  const { addPanel, createWorkspace } = shell;
  useEffect(() => {
    if (!pending) return;
    const target = orchestratorTarget(workspaces, pending.repo);
    if (target.kind === "panel") {
      switchWorkspace(target.workspaceId);
      setSelected(target.panelId);
      setZoomed(target.panelId);
      setPending(null);
      return;
    }
    const step =
      target.kind === "add-panel"
        ? `add:${target.workspaceId}`
        : `create:${target.cwd}`;
    if (kicked.current === step) return;
    kicked.current = step;
    if (target.kind === "add-panel")
      void addPanel(
        "orchestrator",
        "claude",
        undefined,
        undefined,
        "local",
        target.workspaceId,
      );
    else void createWorkspace(target.name, target.cwd, "local", "shell");
  }, [
    pending,
    workspaces,
    switchWorkspace,
    setSelected,
    setZoomed,
    addPanel,
    createWorkspace,
  ]);

  return { toasts, dispatch, openTask };
}
