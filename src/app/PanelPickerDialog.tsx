import { useEffect, useMemo, useState } from "react";
import {
  FolderOpen,
  Globe,
  Plus,
  Sparkles,
  TerminalSquare,
} from "lucide-react";
import { agentTitle } from "./agent-title.ts";
import { sessionHostOptions, type SessionHostContext } from "./sessionHosts.ts";
import { ExtensionPanelOptions } from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import type { ModelProfile, PanelKind, System, Workspace } from "../types";

/** Model profiles are this dialog's business only, so they load when it opens
 * and the picked profile resets with it. Same for the session-host pick
 * (D1-D3): the merge group is only ever the active workspace's, so it too is
 * safe to derive once, here, from `hostContext` (App.tsx's own state, handed
 * down as one prop). */
export function PanelPickerDialog({
  active,
  adding,
  system,
  addPanel,
  addExtensionPanel,
  extensionRegistry,
  connected,
  hostContext,
}: {
  active: Workspace;
  adding: boolean;
  system: System | null;
  addPanel(
    kind: PanelKind,
    agent?: string,
    filesTarget?: undefined,
    modelProfile?: ModelProfile,
    backend?: "herdr" | "local",
    targetWorkspaceId?: string,
  ): void;
  connected: boolean;
  addExtensionPanel(
    extensionId: string,
    contributionId: string,
    targetWorkspaceId?: string,
  ): void;
  extensionRegistry: ExtensionRegistry;
  hostContext: SessionHostContext;
}) {
  const [modelProfiles, setModelProfiles] = useState<ModelProfile[]>([]);
  // Only a Herdr-backed workspace has a choice to offer.
  const herdrWorkspace = Boolean(active.herdrId) && connected;
  const [backend, setBackend] = useState<"herdr" | "local">("herdr");
  const [selectedModelProfileId, setSelectedModelProfileId] = useState("");
  useEffect(() => {
    window.bridge?.modelProfilesList().then(setModelProfiles);
  }, []);
  // Empty outside a merge group (D3): the picker then targets `active` alone,
  // exactly as it always has.
  const hostOptions = useMemo(
    () => sessionHostOptions(active, hostContext),
    [active, hostContext],
  );
  const [hostId, setHostId] = useState(active.id);
  const targetWorkspaceId = hostOptions.length ? hostId : undefined;
  const launchLabel =
    hostOptions.find((option) => option.workspaceId === hostId)?.label ||
    active.name;
  return (
    <>
      <div className="dialog-eyebrow">MAKE IT YOUR SPACE</div>
      <h2>Add a panel</h2>
      <p>Everything you need, side by side.</p>
      {hostOptions.length > 0 && (
        <>
          <div className="dialog-eyebrow">LAUNCH ON</div>
          <div
            className="panel-backend"
            role="radiogroup"
            aria-label="Launch on"
          >
            {hostOptions.map((option) => (
              <button
                key={option.workspaceId}
                role="radio"
                aria-checked={hostId === option.workspaceId}
                className={hostId === option.workspaceId ? "selected" : ""}
                onClick={() => setHostId(option.workspaceId)}
              >
                {option.label}
              </button>
            ))}
          </div>
        </>
      )}
      {herdrWorkspace && (
        <div
          className="panel-backend"
          role="group"
          aria-label="Session backend"
        >
          {(
            [
              ["herdr", "Herdr", "Keeps running when the app closes"],
              ["local", "Local", "A plain shell in this window"],
            ] as const
          ).map(([value, label, detail]) => (
            <button
              key={value}
              className={backend === value ? "selected" : ""}
              aria-pressed={backend === value}
              title={detail}
              onClick={() => setBackend(value)}
            >
              {label}
            </button>
          ))}
        </div>
      )}
      <div className={`panel-options ${adding ? "is-busy" : ""}`}>
        {(
          [
            {
              kind: "terminal",
              title: "Terminal",
              detail: "A real shell in your project",
              icon: TerminalSquare,
            },
            {
              kind: "files",
              title: "Files & Git",
              detail: "Explore code, images and changes",
              icon: FolderOpen,
            },
            {
              kind: "browser",
              title: "Browser",
              detail: "Your local app or any website",
              icon: Globe,
            },
            {
              kind: "chat",
              title: "Thread",
              detail: "Talk to Claude Code or Codex",
              icon: Sparkles,
            },
          ] as const
        ).map((item) => (
          <button
            key={item.kind}
            onClick={() =>
              addPanel(
                item.kind,
                undefined,
                undefined,
                undefined,
                backend,
                targetWorkspaceId,
              )
            }
          >
            <item.icon size={19} />
            <div>
              <strong>{item.title}</strong>
              <small>{item.detail}</small>
            </div>
            <Plus size={15} />
          </button>
        ))}
        <ExtensionPanelOptions
          registry={extensionRegistry}
          onAdd={(extensionId, contributionId) =>
            addExtensionPanel(extensionId, contributionId, targetWorkspaceId)
          }
        />
      </div>
      <div className="dialog-eyebrow agent-options-label">CODING AGENTS</div>
      <div className="agent-options">
        {["claude", "codex", "gemini", "cursor-agent"].map((agent) => (
          <button
            key={agent}
            onClick={() =>
              addPanel(
                "agent",
                agent,
                undefined,
                agent === "claude"
                  ? modelProfiles.find(
                      (profile) => profile.id === selectedModelProfileId,
                    )
                  : undefined,
                backend,
                targetWorkspaceId,
              )
            }
          >
            <span className={agent === "claude" ? "agent-star" : "agent-logo"}>
              {agent === "claude"
                ? "✳"
                : agent === "codex"
                  ? "✺"
                  : agent === "gemini"
                    ? "✦"
                    : "⌘"}
            </span>
            <span>{agentTitle(agent)}</span>
            <small>
              {system?.agents.find((a) => a.name === agent)?.path
                ? "Installed"
                : "CLI required"}
            </small>
          </button>
        ))}
      </div>
      {modelProfiles.length > 0 && (
        <label className="agent-model-picker">
          Claude Code · Custom model
          <select
            value={selectedModelProfileId}
            onChange={(event) => setSelectedModelProfileId(event.target.value)}
          >
            <option value="">Automatic (Anthropic)</option>
            {modelProfiles.map((profile) => (
              <option key={profile.id} value={profile.id}>
                {profile.label}
              </option>
            ))}
          </select>
          <small>Pick a model, then click Claude Code above.</small>
        </label>
      )}
      <div className="dialog-footer">
        <span>
          Launches in <strong>{launchLabel}</strong>
        </span>
        <kbd>esc</kbd>
      </div>
    </>
  );
}
