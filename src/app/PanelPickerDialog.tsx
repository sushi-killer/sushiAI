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
import {
  launchesInWorktree,
  suggestWorktreeBranch,
  worktreeBranchError,
} from "../workspace/worktree.ts";
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
    worktree?: { branch: string },
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
  // The launch host, resolved the same way addPanel resolves it - the picker
  // shows worktree choices for whichever workspace a session would actually
  // start in, not always the active one.
  const targetWorkspace =
    (targetWorkspaceId &&
      hostContext.workspaces.find((w) => w.id === targetWorkspaceId)) ||
    active;
  const targetIsSsh = Boolean(targetWorkspace.connection?.startsWith("ssh:"));
  const canHerdrWorktree =
    Boolean(targetWorkspace.herdrId) && connected && backend === "herdr";
  // A worktree launched without Herdr becomes a plain local process on this
  // Mac, so it needs the target workspace's own checkout to be local too.
  const canLocalWorktree =
    !targetIsSsh && (!targetWorkspace.herdrId || backend === "local");
  const canWorktree = canHerdrWorktree || canLocalWorktree;
  const [checkout, setCheckout] = useState<"current" | "worktree">("current");
  const [branch, setBranch] = useState(() => suggestWorktreeBranch(new Date()));
  // A backend or host switch can take the worktree option away while it is
  // selected; the radiogroup unmounts, so the choice has to lapse with it or a
  // remote path reaches the local git.
  const wantsWorktree = canWorktree && checkout === "worktree";
  const branchError = wantsWorktree ? worktreeBranchError(branch) : "";
  const worktreeArg = wantsWorktree ? { branch } : undefined;
  const worktreeInvalid = wantsWorktree && Boolean(branchError);
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
      {canWorktree && (
        <>
          <div className="dialog-eyebrow">CHECKOUT</div>
          <div
            className="panel-backend"
            role="radiogroup"
            aria-label="Checkout"
          >
            {(
              [
                ["current", "This checkout"],
                ["worktree", "New worktree"],
              ] as const
            ).map(([value, label]) => (
              <button
                key={value}
                role="radio"
                aria-checked={checkout === value}
                className={checkout === value ? "selected" : ""}
                onClick={() => setCheckout(value)}
              >
                {label}
              </button>
            ))}
          </div>
          {wantsWorktree && (
            <>
              <div className="dialog-eyebrow">BRANCH</div>
              <input
                className="worktree-branch"
                aria-label="Branch"
                value={branch}
                onChange={(event) => setBranch(event.target.value)}
                placeholder="feature/my-change"
              />
              {branchError && (
                <small className="inline-error">{branchError}</small>
              )}
            </>
          )}
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
            disabled={launchesInWorktree(item.kind) && worktreeInvalid}
            onClick={() =>
              addPanel(
                item.kind,
                undefined,
                undefined,
                undefined,
                backend,
                targetWorkspaceId,
                launchesInWorktree(item.kind) ? worktreeArg : undefined,
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
            disabled={worktreeInvalid}
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
                worktreeArg,
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
          {!wantsWorktree ? (
            <>
              Launches in <strong>{launchLabel}</strong>
            </>
          ) : worktreeInvalid ? null : (
            // Only a terminal or an agent gets the worktree; the other panels
            // are views of the project and open where they always did.
            <>
              Terminal and agents launch in a new worktree on{" "}
              <strong>{branch}</strong>
            </>
          )}
        </span>
        <kbd>esc</kbd>
      </div>
    </>
  );
}
