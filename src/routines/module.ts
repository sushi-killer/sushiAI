import { lazy } from "react";
import { Workflow } from "lucide-react";
import type {
  AttentionItem,
  ModuleUi,
  WorktreeClaim,
} from "../extensions/modules.ts";
import { useRoutinesEnabled } from "./enabled.ts";
import { ROUTINES_EXTENSION_ID } from "./id.ts";
import { dismissAttention, useRoutinesStore } from "./store.ts";

const NONE: WorktreeClaim[] = [];
const OFF: AttentionItem[] = [];

const AttentionDetail = lazy(() =>
  import("./AttentionDetail.tsx").then(({ AttentionDetail: Component }) => ({
    default: Component,
  })),
);

/** The Routines module's rows in the Inbox: only what needs the owner. */
export const routinesModule: ModuleUi = {
  extensionId: ROUTINES_EXTENSION_ID,
  icon: Workflow,
  useAttention: () => {
    const enabled = useRoutinesEnabled();
    const store = useRoutinesStore(enabled);
    return enabled ? store.attention : OFF;
  },
  AttentionDetail,
  act: async (key, actionId) => {
    if (actionId === "dismiss") dismissAttention(key);
  },
  useWorktreeClaims: () => NONE,
  useShell: () => {},
};
