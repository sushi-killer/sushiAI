import type { ComponentType } from "react";
import type { Workspace } from "../types.ts";
import { orchestratorModule } from "../orchestrator/module.ts";

/** The module UI API: renderer hooks for built-in extensions only. It is not
 * part of the manifest and has no API version. This file is a composition
 * root, like coreViews.ts: the only place core names a built-in module. */

export type AttentionKind = "answer" | "decide" | "review";

/** One thing a module lets the owner do with an Inbox row. `key` binds a
 * keyboard shortcut for the selected row; `primary` draws it filled. */
export type AttentionAction = {
  id: string;
  label: string;
  key?: string;
  primary?: boolean;
  /** Drawn as picked (a choice that waits for a confirm). */
  selected?: boolean;
};

/** One Inbox row a module asks the owner to act on. */
export type AttentionItem = {
  key: string;
  kind: AttentionKind;
  title: string;
  project: string;
  host: string;
  at: number | null;
  search: string;
  /** The second line of the row, for example "failed · 0/0 attempts · $0.41". */
  meta?: string;
  /** Inline actions of the row, drawn as chips in this order. */
  actions?: AttentionAction[];
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
  /** Drawn at the title of the module's rows (provenance). */
  icon?: ComponentType<{ size?: number }>;
  /** [] when the module is off. */
  useAttention(): AttentionItem[];
  AttentionDetail: ComponentType<{ item: AttentionItem }>;
  /** Runs one of a row's actions. `text` carries what the owner typed. */
  act(key: string, actionId: string, text?: string): Promise<void>;
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
