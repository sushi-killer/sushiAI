import { lazy } from "react";
import type { ModuleShell, ModuleUi } from "../extensions/modules.ts";
import {
  ORCHESTRATOR_EXTENSION_ID,
  useOrchestratorEnabled,
} from "./enabled.ts";
import { reviewAllTasks, useOrchestratorAttention } from "./moduleAttention.ts";
import { useOrchestratorShell } from "./moduleShell.ts";
import { useOrchestratorWorktreeClaims } from "./moduleWorktrees.ts";

const InboxTaskDetail = lazy(() =>
  import("./InboxTaskDetail.tsx").then(({ InboxTaskDetail: Component }) => ({
    default: Component,
  })),
);

/** The built-in orchestrator's module UI. Each hook gates itself on the
 * extension being enabled. */
export const orchestratorModule: ModuleUi = {
  extensionId: ORCHESTRATOR_EXTENSION_ID,
  useAttention: () => useOrchestratorAttention(useOrchestratorEnabled()),
  AttentionDetail: InboxTaskDetail,
  reviewAll: reviewAllTasks,
  useWorktreeClaims: () =>
    useOrchestratorWorktreeClaims(useOrchestratorEnabled()),
  useShell: (shell: ModuleShell) =>
    useOrchestratorShell(useOrchestratorEnabled(), shell),
};
