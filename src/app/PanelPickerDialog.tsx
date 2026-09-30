import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FolderOpen,
  Globe,
  ListChecks,
  Plus,
  Sparkles,
  TerminalSquare,
} from "lucide-react";
import { agentTitle } from "./agent-title.ts";
import { useOrchestratorEnabled } from "../orchestrator/enabled.ts";
import { sessionHostOptions, type SessionHostContext } from "./sessionHosts.ts";
import { ExtensionPanelOptions } from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import {
  launchesInWorktree,
  suggestWorktreeBranch,
  worktreeBranchError,
} from "../workspace/worktree.ts";
import type {
  ClaudeAccount,
  ModelProfile,
  PanelKind,
  System,
  Workspace,
} from "../types";

const AGENTS = ["claude", "codex", "gemini", "cursor-agent"];
const TOOLS = [
  {
    key: "a",
    kind: "terminal",
    title: "Terminal",
    detail: "A real shell in your project",
    icon: TerminalSquare,
  },
  {
    key: "s",
    kind: "files",
    title: "Files & Git",
    detail: "Explore code, images and changes",
    icon: FolderOpen,
  },
  {
    key: "d",
    kind: "browser",
    title: "Browser",
    detail: "Your local app or any website",
    icon: Globe,
  },
  {
    key: "f",
    kind: "chat",
    title: "Thread",
    detail: "Talk to Claude Code or Codex",
    icon: Sparkles,
  },
  {
    key: "g",
    kind: "orchestrator",
    title: "Orchestrator",
    detail: "Tasks carried to done",
    icon: ListChecks,
  },
] as const;

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
    claudeAccountId?: string,
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
  const orchestrator = useOrchestratorEnabled();
  const [modelProfiles, setModelProfiles] = useState<ModelProfile[]>([]);
  const [claudeAccounts, setClaudeAccounts] = useState<ClaudeAccount[]>([]);
  // Only a Herdr-backed workspace has a choice to offer.
  const herdrWorkspace = Boolean(active.herdrId) && connected;
  const [backend, setBackend] = useState<"herdr" | "local">("herdr");
  const [selectedModelProfileId, setSelectedModelProfileId] = useState("");
  const [selectedClaudeAccountId, setSelectedClaudeAccountId] = useState("");
  const [projectName, setProjectName] = useState(active.name);
  const [environmentCount, setEnvironmentCount] = useState(0);
  const [projectId, setProjectId] = useState("");
  const [hostReadiness, setHostReadiness] = useState<Record<string, string>>(
    {},
  );
  const agentButtons = useRef<(HTMLButtonElement | null)[]>([]);
  const [focusedAgent, setFocusedAgent] = useState(0);
  useEffect(() => {
    window.bridge?.modelProfilesList().then(setModelProfiles);
    window.bridge?.claudeAccountsList().then(setClaudeAccounts);
  }, []);
  useEffect(() => {
    let live = true;
    const remote = hostContext.projectGit[active.id]?.remote;
    if (!remote || !window.bridge) {
      setProjectId("");
      setProjectName(active.name);
      setEnvironmentCount(0);
      return;
    }
    window.bridge
      .projectsResolve({
        remote,
        endpoint: active.connection || "local",
      })
      .then((project) => {
        if (!live) return;
        setProjectId(project?.id || "");
        setProjectName(project?.name || active.name);
        setEnvironmentCount(project?.env.length || 0);
      })
      .catch(() => {
        if (!live) return;
        setProjectId("");
        setProjectName(active.name);
        setEnvironmentCount(0);
      });
    return () => {
      live = false;
    };
  }, [active.connection, active.id, active.name, hostContext.projectGit]);
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
  useEffect(() => {
    let live = true;
    const sshHosts = (
      hostOptions.length
        ? hostOptions
        : [{ workspaceId: active.id, label: active.name }]
    )
      .map((option) =>
        hostContext.workspaces.find((w) => w.id === option.workspaceId),
      )
      .filter((workspace): workspace is Workspace =>
        Boolean(workspace?.connection?.startsWith("ssh:")),
      );
    for (const workspace of sshHosts) {
      setHostReadiness((current) => ({
        ...current,
        [workspace.id]: "Checking…",
      }));
      if (!window.bridge || !projectId) {
        setHostReadiness((current) => ({
          ...current,
          [workspace.id]: "Unreachable",
        }));
        continue;
      }
      window.bridge
        .projectHostCheck(projectId, workspace.connection!, workspace.cwd)
        .then(() => {
          if (live)
            setHostReadiness((current) => ({
              ...current,
              [workspace.id]: "Ready",
            }));
        })
        .catch(() => {
          if (live)
            setHostReadiness((current) => ({
              ...current,
              [workspace.id]: "Unreachable",
            }));
        });
    }
    return () => {
      live = false;
    };
  }, [active.id, active.name, hostOptions, hostContext.workspaces, projectId]);
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
  const worktreeArg = useMemo(
    () => (wantsWorktree ? { branch } : undefined),
    [branch, wantsWorktree],
  );
  const worktreeInvalid = wantsWorktree && Boolean(branchError);
  useEffect(() => {
    if (!canWorktree && checkout !== "current") setCheckout("current");
  }, [canWorktree, checkout]);
  const launchAgent = useCallback(
    (agent: string) => {
      if (adding || worktreeInvalid) return;
      addPanel(
        "agent",
        agent,
        undefined,
        agent === "claude"
          ? modelProfiles.find(
              (profile) => profile.id === selectedModelProfileId,
            )
          : undefined,
        agent === "claude" ? selectedClaudeAccountId || undefined : undefined,
        backend,
        targetWorkspaceId,
        worktreeArg,
      );
    },
    [
      addPanel,
      backend,
      modelProfiles,
      selectedClaudeAccountId,
      selectedModelProfileId,
      targetWorkspaceId,
      worktreeArg,
      worktreeInvalid,
      adding,
    ],
  );
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        event.shiftKey ||
        event.repeat ||
        adding
      )
        return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, select, [contenteditable='true']"))
        return;
      if (/^[1-4]$/.test(event.key)) {
        event.preventDefault();
        const index = Number(event.key) - 1;
        setFocusedAgent(index);
        agentButtons.current[index]?.focus();
      } else if (event.key === "Enter") {
        if (target !== document.body && !target?.closest(".picker-agent-list"))
          return;
        event.preventDefault();
        launchAgent(AGENTS[focusedAgent]);
      } else {
        const tool = TOOLS.find((item) => item.key === event.key.toLowerCase());
        if (tool && (orchestrator || tool.kind !== "orchestrator")) {
          if (launchesInWorktree(tool.kind) && worktreeInvalid) return;
          event.preventDefault();
          addPanel(
            tool.kind,
            undefined,
            undefined,
            undefined,
            undefined,
            backend,
            targetWorkspaceId,
            launchesInWorktree(tool.kind) ? worktreeArg : undefined,
          );
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    focusedAgent,
    orchestrator,
    backend,
    targetWorkspaceId,
    worktreeArg,
    worktreeInvalid,
    addPanel,
    launchAgent,
    adding,
  ]);
  return (
    <div className="panel-picker-v4">
      <div className="picker-project-head">
        <div>
          <span className="dialog-eyebrow">NEW SESSION</span>
          <h2>{projectName}</h2>
        </div>
        <span className="picker-environment-pill">
          <span /> Environment · {environmentCount}
        </span>
      </div>
      <p>Choose where to start, then pick an agent or tool.</p>
      {(hostOptions.length > 0 || herdrWorkspace) && (
        <section className="picker-host-row">
          <div className="picker-section-label">HOST</div>
          <div
            className="panel-backend"
            role="radiogroup"
            aria-label="Launch on"
          >
            {(hostOptions.length
              ? hostOptions
              : [{ workspaceId: active.id, label: active.name }]
            ).map((option) => (
              <button
                key={option.workspaceId}
                role="radio"
                aria-checked={hostId === option.workspaceId}
                className={hostId === option.workspaceId ? "selected" : ""}
                onClick={() => setHostId(option.workspaceId)}
              >
                {option.label}
                <small className="host-readiness">
                  <span
                    className={
                      hostContext.workspaces
                        .find((w) => w.id === option.workspaceId)
                        ?.connection?.startsWith("ssh:") &&
                      hostReadiness[option.workspaceId] !== "Ready"
                        ? "is-pending"
                        : ""
                    }
                  />
                  {hostContext.workspaces
                    .find((w) => w.id === option.workspaceId)
                    ?.connection?.startsWith("ssh:")
                    ? hostReadiness[option.workspaceId] || "Checking…"
                    : "Ready"}
                </small>
              </button>
            ))}
          </div>
          {herdrWorkspace && (
            <div
              className="panel-backend picker-backend"
              role="group"
              aria-label="Session backend"
            >
              {(
                [
                  ["herdr", "Herdr"],
                  ["local", "Local"],
                ] as const
              ).map(([value, label]) => (
                <button
                  key={value}
                  className={backend === value ? "selected" : ""}
                  aria-pressed={backend === value}
                  onClick={() => setBackend(value)}
                >
                  {label}
                </button>
              ))}
            </div>
          )}
        </section>
      )}
      {canWorktree && (
        <div className="picker-checkout-line">
          <label className="picker-worktree-check">
            <input
              type="checkbox"
              checked={wantsWorktree}
              onChange={(event) =>
                setCheckout(event.target.checked ? "worktree" : "current")
              }
            />{" "}
            New worktree
          </label>
          {wantsWorktree && (
            <div className="picker-branch-field">
              <span>{targetWorkspace.cwd}</span>
              <input
                className="worktree-branch"
                aria-label="Branch"
                value={branch}
                onChange={(event) => setBranch(event.target.value)}
                placeholder="feature/my-change"
              />
            </div>
          )}
          {branchError && <small className="inline-error">{branchError}</small>}
        </div>
      )}
      <div className={`picker-lists ${adding ? "is-busy" : ""}`}>
        <section className="picker-agent-list">
          <div className="picker-list-heading">
            <span>AGENTS</span>
            <small>Choose one to start</small>
          </div>
          <div className="agent-options">
            {AGENTS.map((agent, index) => (
              <button
                key={agent}
                ref={(element) => {
                  agentButtons.current[index] = element;
                }}
                className={focusedAgent === index ? "is-focused" : ""}
                disabled={adding || worktreeInvalid}
                onFocus={() => setFocusedAgent(index)}
                onClick={() => launchAgent(agent)}
              >
                <kbd>{index + 1}</kbd>
                <span
                  className={agent === "claude" ? "agent-star" : "agent-logo"}
                >
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
        </section>
        <section className="picker-tool-list">
          <div className="picker-list-heading">
            <span>TOOLS</span>
            <small>Open alongside your work</small>
          </div>
          <div className="panel-options">
            {TOOLS.filter(
              (item) => orchestrator || item.kind !== "orchestrator",
            ).map((item) => (
              <button
                key={item.kind}
                disabled={
                  adding || (launchesInWorktree(item.kind) && worktreeInvalid)
                }
                onClick={() =>
                  addPanel(
                    item.kind,
                    undefined,
                    undefined,
                    undefined,
                    undefined,
                    backend,
                    targetWorkspaceId,
                    launchesInWorktree(item.kind) ? worktreeArg : undefined,
                  )
                }
              >
                <kbd>{item.key}</kbd>
                <item.icon size={18} />
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
                !adding &&
                addExtensionPanel(
                  extensionId,
                  contributionId,
                  targetWorkspaceId,
                )
              }
            />
          </div>
        </section>
      </div>
      {modelProfiles.length > 0 && (
        <label className="agent-model-picker">
          Claude Code · Custom model
          <select
            value={selectedModelProfileId}
            onChange={(event) => {
              setSelectedModelProfileId(event.target.value);
              if (event.target.value) setSelectedClaudeAccountId("");
            }}
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
      {claudeAccounts.length > 0 && (
        <label className="agent-model-picker">
          Claude account
          <select
            value={selectedClaudeAccountId}
            onChange={(event) => {
              setSelectedClaudeAccountId(event.target.value);
              if (event.target.value) setSelectedModelProfileId("");
            }}
          >
            <option value="">Use signed-in account</option>
            {claudeAccounts
              .filter((account) => account.hasValue)
              .map((account) => (
                <option key={account.id} value={account.id}>
                  {account.label}
                  {account.hint ? ` · ${account.hint}` : ""}
                </option>
              ))}
          </select>
          <small>Applies to the next Claude Code session.</small>
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
        <span className="picker-shortcuts">
          <kbd>1–4</kbd> agents <kbd>a/s/d/f/g</kbd> tools <kbd>↵</kbd> start{" "}
          <kbd>esc</kbd> close
        </span>
      </div>
    </div>
  );
}
