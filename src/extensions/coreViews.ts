import type { ComponentType } from "react";
import type { ExtensionPanel } from "../types.ts";
import { PreviewView } from "./preview/PreviewView.tsx";

export type LaunchAgentRequest = {
  agent: "claude" | "codex";
  branch: string;
  base: string;
  prompt: string;
};

export type CoreViewProps = {
  panel: ExtensionPanel;
  args: Record<string, string>;
  cwd: string;
  connection?: string;
  /** The Herdr pane id of the agent pane this view was opened beside. */
  besideHerdrPaneId?: string;
  /** The Herdr endpoint (local socket or "ssh:<id>") that pane lives on. */
  herdrEndpoint?: string;
  /** The title of that agent pane, for "Send to <agent>". */
  besideLabel?: string;
  /** Starts an agent session in a new worktree with a first prompt, through
   * the app's own session launch. Resolves false when the launch failed (the
   * app has already told the owner why). */
  launchAgent?(request: LaunchAgentRequest): Promise<boolean>;
  onArgs(next: Record<string, string>): void;
};

/** The React views builtin extensions draw, by the `viewId` of a surface whose
 * view is `{ kind: "core" }`. A manifest cannot ship one: the validator only
 * lets builtins name a core view. */
export const coreViews: Record<string, ComponentType<CoreViewProps>> = {
  "artifacts.preview": PreviewView,
};
