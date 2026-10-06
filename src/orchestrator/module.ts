import type { ModuleShell, ModuleUi } from "../extensions/modules.ts";
import {
  ORCHESTRATOR_EXTENSION_ID,
  useOrchestratorEnabled,
} from "./enabled.ts";
import { InboxTaskDetail } from "./InboxTaskDetail.tsx";
import { reviewAllTasks, useOrchestratorAttention } from "./moduleAttention.ts";
import { useOrchestratorShell } from "./moduleShell.ts";
import { useOrchestratorWorktreeClaims } from "./moduleWorktrees.ts";

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
