import type { ComponentType } from "react";
import type { Workspace } from "../types.ts";
import { orchestratorModule } from "../orchestrator/module.ts";

/** The module UI API: renderer hooks for built-in extensions only. It is not
 * part of the manifest and has no API version. This file is a composition
 * root, like coreViews.ts: the only place core names a built-in module. */

export type AttentionKind = "answer" | "decide" | "review";

/** One Inbox row a module asks the owner to act on. */
export type AttentionItem = {
  key: string;
  kind: AttentionKind;
  title: string;
  project: string;
  host: string;
  at: number | null;
  search: string;
};

/** A module's statement that a worktree is its own. */
export type WorktreeClaim = {
  host: string;
  repo: string;
  path?: string;
  branch?: string;
  label: string;
  /** In use: removing the worktree is blocked. */
  active: boolean;
};

export type ModuleShell = {
  workspaces: Workspace[];
  openSurface(
    where:
      | { workspaceId: string }
      | { cwd: string; connection?: string; name: string },
    extensionId: string,
    surfaceId: string,
    args: Record<string, string>,
  ): void;
};

export type ModuleUi = {
  extensionId: string;
  /** [] when the module is off. */
  useAttention(): AttentionItem[];
  AttentionDetail: ComponentType<{ item: AttentionItem }>;
  reviewAll?: { label: string; run(keys: string[]): Promise<void> };
  useWorktreeClaims(): WorktreeClaim[];
  /** Called once from App.tsx. */
  useShell(shell: ModuleShell): void;
};

/** A constant list: callers loop over it calling hooks, and the order never
 * changes between renders, so the rules of hooks hold. */
export const moduleUis: readonly ModuleUi[] = Object.freeze([
  orchestratorModule,
]);
