import type { ComponentType } from "react";
import { FileText, type LucideIcon } from "lucide-react";
import { PreviewView } from "./preview/PreviewView.tsx";
import { useChangedWhileHidden } from "./preview/useChangedWhileHidden.ts";

export type LaunchAgentRequest = {
  agent: "claude" | "codex";
  branch: string;
  base: string;
  prompt: string;
};

/** The worktrees the app launched sessions into that are still live: their
 * branches, and a way to go to one. */
export type WorktreeSessions = {
  branches: string[];
  open(branch: string): void;
};

export type CoreViewProps = {
  args: Record<string, string>;
  cwd: string;
  /** The agent pane's own live folder, which can differ from `cwd`. */
  paneCwd?: string;
  connection?: string;
  /** The Herdr pane id of the agent pane this view is the companion of. */
  agentHerdrPaneId?: string;
  /** The Herdr endpoint (local socket or "ssh:<id>") that pane lives on. */
  herdrEndpoint?: string;
  /** The title of that agent pane, for "Send to <agent>". */
  agentLabel?: string;
  /** The live sessions the app runs in worktrees. */
  worktrees?: WorktreeSessions;
  /** Starts an agent session in a new worktree with a first prompt, through
   * the app's own session launch. Resolves false when the launch failed (the
   * app has already told the owner why). */
  launchAgent?(request: LaunchAgentRequest): Promise<boolean>;
  /** The companion's header row, where the view draws its own controls. */
  headerSlot?: HTMLElement | null;
  onArgs(next: Record<string, string>): void;
};

/** One core view and what the companion's "show" button needs to draw it:
 * the icon, and an optional hook that is true when the view has something new
 * while it is hidden. */
export type CoreViewEntry = {
  View: ComponentType<CoreViewProps>;
  icon: LucideIcon;
  useChanged?(
    args: Record<string, string>,
    cwd: string,
    connection: string | undefined,
  ): boolean;
};

/** The React views builtin extensions draw, by the `viewId` of a surface whose
 * view is `{ kind: "core" }`. A manifest cannot ship one: the validator only
 * lets builtins name a core view. */
export const coreViews: Record<string, CoreViewEntry> = {
  "artifacts.preview": {
    View: PreviewView,
    icon: FileText,
    useChanged: useChangedWhileHidden,
  },
};
