import {
  Blocks,
  FolderOpen,
  Globe,
  Sparkles,
  TerminalSquare,
} from "lucide-react";
import type { PanelKind } from "./types";

export const Icon = ({
  kind,
  agent,
  size = 13,
}: {
  kind: PanelKind | "extension";
  agent?: string;
  size?: number;
}) =>
  agent && ["claude", "codex", "gemini", "cursor-agent"].includes(agent) ? (
    <img
      className="harness-icon"
      src={`./agents/${agent}.svg`}
      width={size}
      height={size}
      alt={agent}
    />
  ) : kind === "agent" ? (
    <span className="agent-star">✳</span>
  ) : kind === "terminal" ? (
    <TerminalSquare size={size} />
  ) : kind === "browser" ? (
    <Globe size={size} />
  ) : kind === "files" ? (
    <FolderOpen size={size} />
  ) : kind === "extension" ? (
    <Blocks size={size} />
  ) : (
    <Sparkles size={size} />
  );
