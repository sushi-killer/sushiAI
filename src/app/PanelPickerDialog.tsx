import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  FolderOpen,
  GitBranch,
  Globe,
  Lock,
  Sparkles,
  TerminalSquare,
} from "lucide-react";
import { agentTitle } from "./agent-title.ts";
import { Icon } from "../PanelIcon.tsx";
import { openSettings } from "../lib/openSettings.ts";
import { checkoutPath, tildePath as tilde } from "../projectPrepare.ts";
import {
  launchTarget,
  projectChoices,
  sessionHostOptions,
  type SessionHostContext,
} from "./sessionHosts.ts";
import { PrepareAndStart } from "./PrepareAndStart.tsx";
import {
  ExtensionIcon,
  extensionPanelOptions,
} from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import {
  launchesInWorktree,
  suggestWorktreeBranch,
  worktreeBranchError,
  worktreeBaseRef,
} from "../workspace/worktree.ts";
import type {
  ClaudeAccount,
  CodexAccount,
  ModelProfile,
  PanelKind,
  Project,
  ProjectHostReadiness,
  System,
  Workspace,
} from "../types";

const AGENTS = ["claude", "codex", "gemini", "cursor-agent"];
const GLYPHS: Record<string, string> = {
  claude: "✳",
  codex: "✺",
  gemini: "✦",
  "cursor-agent": "⌘",
};
const TOOLS = [
  {
    key: "t",
    kind: "terminal",
    title: "Terminal",
    detail: "A shell in the project",
    icon: TerminalSquare,
  },
  {
    key: "f",
    kind: "files",
    title: "Files & Git",
    detail: "Code, images, changes",
    icon: FolderOpen,
  },
  {
    key: "b",
    kind: "browser",
    title: "Browser",
    detail: "Local app or any site",
    icon: Globe,
  },
  {
    key: "h",
    kind: "chat",
    title: "Thread",
    detail: "Talk to an agent",
    icon: Sparkles,
  },
] as const;

type Tone = "ok" | "warning" | "danger";

type BaseBranch = {
  name: string;
  ref: string;
  local: boolean;
  remoteOnly?: boolean;
};

/** Model profiles are this dialog's business only, so they load when it opens
 * and the picked profile resets with it. Same for the session-host pick
 * (D1-D3): the merge group is only ever the active workspace's, so it too is
 * safe to derive once, here, from `hostContext` (App.tsx's own state, handed
 * down as one prop). */
export function PanelPickerDialog({
  active: opened,
  adding,
  system,
  addPanel,
  addExtensionPanel,
  extensionRegistry,
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
    targetWorkspaceId?: string,
    worktree?: { branch: string; base: string },
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
  // The project the picker targets: the one it opened for, or another from
  // the title's list (its own open workspace).
  const [baseId, setBaseId] = useState(opened.id);
  const active =
    hostContext.workspaces.find((item) => item.id === baseId) ?? opened;
  const [allProjects, setAllProjects] = useState<Project[]>([]);
  const [projectMenu, setProjectMenu] = useState(false);
  const [setupPick, setSetupPick] = useState("");
  const [starting, setStarting] = useState<string | null>(null);
  const [modelProfiles, setModelProfiles] = useState<ModelProfile[]>([]);
  const [claudeAccounts, setClaudeAccounts] = useState<ClaudeAccount[]>([]);
  const [codexAccounts, setCodexAccounts] = useState<CodexAccount[]>([]);
  const [selectedCodexAccountId, setSelectedCodexAccountId] = useState("");
  const codexTouched = useRef(false);
  const [codexMenu, setCodexMenu] = useState(false);
  const [selectedModelProfileId, setSelectedModelProfileId] = useState("");
  const [selectedClaudeAccountId, setSelectedClaudeAccountId] = useState("");
  const accountTouched = useRef(false);
  const [accountMenu, setAccountMenu] = useState(false);
  const [project, setProject] = useState<Project | null>(null);
  const [hostChecks, setHostChecks] = useState<
    Record<string, ProjectHostReadiness | "checking" | "unreachable">
  >({});
  const agentButtons = useRef<(HTMLButtonElement | null)[]>([]);
  const [focusedAgent, setFocusedAgent] = useState(0);
  useEffect(() => {
    window.bridge
      ?.projectsList()
      .then(setAllProjects)
      .catch(() => {});
    window.bridge?.modelProfilesList().then(setModelProfiles);
    window.bridge?.claudeAccountsList().then(setClaudeAccounts);
    window.bridge
      ?.codexAccountsList()
      .then(setCodexAccounts)
      .catch(() => {});
  }, []);
  useEffect(() => {
    let live = true;
    if (!window.bridge) {
      setProject(null);
      return;
    }
    // By the folder's remote, or by the folder itself when it has none.
    window.bridge
      .projectsResolve({
        endpoint: active.connection || "local",
        cwd: active.cwd,
      })
      .then((found) => {
        if (!live) return;
        setProject(found);
        // The project's own account is the default for a new session.
        if (!accountTouched.current)
          setSelectedClaudeAccountId(found?.sessions.claudeAccount || "");
        if (!codexTouched.current)
          setSelectedCodexAccountId(found?.sessions.codexAccount || "");
      })
      .catch(() => live && setProject(null));
    return () => {
      live = false;
    };
  }, [active.connection, active.cwd, active.id]);
  const projectId = project?.id || "";
  // Empty outside a merge group (D3): the picker then targets `active` alone,
  // exactly as it always has.
  const hostOptions = useMemo(
    () => sessionHostOptions(active, hostContext),
    [active, hostContext],
  );
  const [hostId, setHostId] = useState(active.id);
  const targetWorkspaceId = launchTarget(
    hostOptions.length,
    hostId,
    baseId,
    opened.id,
  );
  // The launch host, resolved the same way addPanel resolves it - the picker
  // shows worktree choices for whichever workspace a session would actually
  // start in, not always the active one.
  const targetWorkspace =
    (targetWorkspaceId &&
      hostContext.workspaces.find((w) => w.id === targetWorkspaceId)) ||
    active;
  const hostName = (workspace: Workspace) =>
    workspace.connection?.startsWith("ssh:")
      ? hostContext.connectionProfiles.find(
          (profile) => `ssh:${profile.id}` === workspace.connection,
        )?.name || "SSH host"
      : "This Mac";
  const hosts = (
    hostOptions.length
      ? hostOptions
      : [{ workspaceId: active.id, label: hostName(active) }]
  ).map((option) => ({
    ...option,
    label: option.label.replace(/^Local\b/, "This Mac"),
    workspace:
      hostContext.workspaces.find((w) => w.id === option.workspaceId) || active,
  }));
  // The member workspaces by what a probe depends on, so a re-render with the
  // same hosts never probes them again.
  const hostsKey = hosts
    .map(
      (item) =>
        `${item.workspace.id}|${item.workspace.connection}|${item.workspace.cwd}`,
    )
    .join(",");
  const choices = projectChoices(
    allProjects,
    hostContext.workspaces,
    hostContext.projectGit,
  );
  const launchLabel = hostName(targetWorkspace);
  useEffect(() => {
    let live = true;
    const remote = hosts
      .map((host) => host.workspace)
      .filter((workspace) => workspace.connection?.startsWith("ssh:"));
    for (const workspace of remote) {
      setHostChecks((current) => ({ ...current, [workspace.id]: "checking" }));
      if (!window.bridge || !projectId) {
        setHostChecks((current) => ({
          ...current,
          [workspace.id]: "unreachable",
        }));
        continue;
      }
      window.bridge
        .projectHostCheck(projectId, workspace.connection!, workspace.cwd)
        .then((matrix) => {
          if (live)
            setHostChecks((current) => ({
              ...current,
              [workspace.id]: matrix,
            }));
        })
        .catch(() => {
          if (live)
            setHostChecks((current) => ({
              ...current,
              [workspace.id]: "unreachable",
            }));
        });
    }
    return () => {
      live = false;
    };
    // `hosts` is rebuilt every render; its identity is the member workspaces.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active.id, hostsKey, projectId]);
  const targetIsSsh = Boolean(targetWorkspace.connection?.startsWith("ssh:"));
  // Slice 1 launches on this Mac only, so a worktree needs a local checkout.
  const canWorktree = !setupPick && !targetIsSsh;
  const [checkout, setCheckout] = useState<"current" | "worktree">("current");
  const [branch, setBranch] = useState(() => suggestWorktreeBranch(new Date()));
  const [base, setBase] = useState("main");
  const [baseBranches, setBaseBranches] = useState<{
    connection: string | undefined;
    cwd: string;
    branches: BaseBranch[];
    error: string;
  } | null>(null);
  // A host switch can take the worktree option away while it is
  // selected; the radiogroup unmounts, so the choice has to lapse with it or a
  // remote path reaches the local git.
  const wantsWorktree = canWorktree && checkout === "worktree";
  const baseConnection = targetWorkspace.connection;
  const baseCwd = targetWorkspace.cwd;
  useEffect(() => {
    if (!wantsWorktree) return;
    let cancelled = false;
    setBaseBranches(null);
    setBase("main");
    async function loadBranches() {
      try {
        if (!window.bridge) throw new Error("Open the desktop app first.");
        const { branches }: { branches: BaseBranch[] } =
          await window.bridge.projectInspect(baseConnection, {
            operation: "branches",
            root: baseCwd,
            includeRemote: true,
          });
        if (cancelled) return;
        setBaseBranches({
          connection: baseConnection,
          cwd: baseCwd,
          branches,
          error: "",
        });
        setBase(
          branches.find((branch) => branch.ref === "main")?.ref ||
            branches.find((branch) => branch.ref === "origin/main")?.ref ||
            "",
        );
      } catch (error) {
        if (cancelled) return;
        setBaseBranches({
          connection: baseConnection,
          cwd: baseCwd,
          branches: [],
          error: error instanceof Error ? error.message : String(error),
        });
        setBase("");
      }
    }
    void loadBranches();
    return () => {
      cancelled = true;
    };
  }, [wantsWorktree, baseConnection, baseCwd]);
  const baseReady =
    baseBranches?.connection === baseConnection &&
    baseBranches?.cwd === baseCwd;
  const branches = baseReady ? baseBranches.branches : [];
  const baseError = baseReady ? baseBranches.error : "";
  const branchError = wantsWorktree ? worktreeBranchError(branch) : "";
  const selectedBase = branches.find((branch) => branch.ref === base);
  const worktreeArg = useMemo(
    () =>
      wantsWorktree && selectedBase
        ? { branch, base: worktreeBaseRef(selectedBase) }
        : undefined,
    [branch, selectedBase, wantsWorktree],
  );
  const worktreeInvalid =
    wantsWorktree &&
    (Boolean(branchError || baseError) || !baseReady || !selectedBase);
  const baseMessage =
    baseError ||
    (baseReady && !branches.length
      ? "No base branches are available."
      : baseReady && !base
        ? "main is unavailable. Choose a base branch."
        : "");
  useEffect(() => {
    if (!canWorktree && checkout !== "current") setCheckout("current");
  }, [canWorktree, checkout]);
  const launchAgent = useCallback(
    (agent: string) => {
      if (adding || worktreeInvalid) return;
      // A host that is not set up yet is prepared first, then started on.
      if (setupPick) return setStarting(agent);
      addPanel(
        "agent",
        agent,
        undefined,
        agent === "claude"
          ? modelProfiles.find(
              (profile) => profile.id === selectedModelProfileId,
            )
          : undefined,
        // "" is a deliberate "none": the host's own login, not the
        // project's account.
        agent === "claude"
          ? selectedClaudeAccountId
          : agent === "codex" && codexTouched.current
            ? selectedCodexAccountId
            : // Untouched: the main process applies the project's account,
              // and the host's login when that one cannot run.
              undefined,
        targetWorkspaceId,
        worktreeArg,
      );
    },
    [
      addPanel,
      setupPick,
      modelProfiles,
      selectedClaudeAccountId,
      selectedCodexAccountId,
      selectedModelProfileId,
      targetWorkspaceId,
      worktreeArg,
      worktreeInvalid,
      adding,
    ],
  );
  const extensionTools = extensionPanelOptions(extensionRegistry);
  const tools = TOOLS;
  // An extension's tool takes the first letter of its label that no other
  // tool already owns.
  const taken = new Set<string>(tools.map((item) => item.key));
  const extensionKeys = extensionTools.map((item) => {
    const key = [...item.label.toLowerCase()].find(
      (letter) => /[a-z0-9]/.test(letter) && !taken.has(letter),
    );
    if (key) taken.add(key);
    return key;
  });
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        event.shiftKey ||
        event.repeat ||
        adding ||
        starting
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
        if (target !== document.body && !target?.closest(".pk-agents")) return;
        event.preventDefault();
        launchAgent(AGENTS[focusedAgent]);
      } else {
        const letter = event.key.toLowerCase();
        const tool = tools.find((item) => item.key === letter);
        const at = extensionKeys.indexOf(letter);
        if (tool) {
          if (launchesInWorktree(tool.kind) && worktreeInvalid) return;
          event.preventDefault();
          addPanel(
            tool.kind,
            undefined,
            undefined,
            undefined,
            undefined,
            targetWorkspaceId,
            launchesInWorktree(tool.kind) ? worktreeArg : undefined,
          );
        } else if (at >= 0) {
          event.preventDefault();
          addExtensionPanel(
            extensionTools[at].extensionId,
            extensionTools[at].surfaceId,
            targetWorkspaceId,
          );
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // `extensionKeys` is rebuilt from the registry, which is listed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    focusedAgent,
    extensionRegistry,
    targetWorkspaceId,
    worktreeArg,
    worktreeInvalid,
    addPanel,
    addExtensionPanel,
    launchAgent,
    adding,
    starting,
  ]);

  const targetCheck = hostChecks[targetWorkspace.id];
  const matrix =
    targetCheck && targetCheck !== "checking" && targetCheck !== "unreachable"
      ? targetCheck
      : undefined;
  const hostTone = (workspace: Workspace): Tone => {
    if (!workspace.connection?.startsWith("ssh:")) return "ok";
    const check = hostChecks[workspace.id];
    if (!check || check === "checking") return "warning";
    if (check === "unreachable" || !check.checkout.ok) return "danger";
    return check.setup.configured && !check.setup.stale ? "ok" : "warning";
  };
  // SSH hosts the picker knows but that have no workspace for this project:
  // shown, not launchable, so a host that is not set up is visible too.
  const shownConnections = new Set(
    hosts.map((host) => host.workspace.connection),
  );
  const setupHosts = hostContext.connectionProfiles.filter(
    (profile) => !shownConnections.has(`ssh:${profile.id}`),
  );
  /** What a coding agent is on the launch host. */
  const agentDetail = (agent: string): { text: string; tone?: Tone } => {
    if (targetIsSsh) {
      if (targetCheck === "unreachable")
        return { text: `${launchLabel} is unreachable`, tone: "warning" };
      if (!matrix) return { text: "Checking…" };
      if (agent === "claude" || agent === "codex")
        return !matrix.clis[agent].installed
          ? { text: `Not installed on ${launchLabel}`, tone: "warning" }
          : matrix.clis[agent].loggedIn
            ? { text: `Ready on ${launchLabel}` }
            : {
                text: `Not signed in on ${launchLabel} · sign in there once`,
                tone: "warning",
              };
      return { text: "Ready" };
    }
    return system?.agents.find((a) => a.name === agent)?.path
      ? { text: "Ready" }
      : { text: "CLI required", tone: "warning" };
  };
  const secrets = project?.env.filter((entry) => entry.secret).length ?? 0;
  const servers = Object.keys(
    (project?.mcp as { mcpServers?: object } | undefined)?.mcpServers ?? {},
  ).length;
  const account = claudeAccounts.find(
    (item) => item.id === selectedClaudeAccountId,
  );
  const profile = modelProfiles.find(
    (item) => item.id === selectedModelProfileId,
  );
  const accountLabel = profile?.label || account?.label || "Signed-in account";
  // Where the checkout is and what the next run does first. A remote path is
  // shown from the host's own home, never the local one.
  const info = !targetIsSsh
    ? tilde(targetWorkspace.cwd, system?.home)
    : !targetCheck || targetCheck === "checking"
      ? `${tilde(targetWorkspace.cwd, system?.home)} · checking the host…`
      : targetCheck === "unreachable"
        ? `${tilde(targetWorkspace.cwd, system?.home)} · host unreachable`
        : !targetCheck.checkout.ok
          ? `${tilde(targetWorkspace.cwd, system?.home)} · not cloned yet`
          : `${checkoutPath(targetCheck)} · ${
              targetCheck.setup.stale
                ? `${targetCheck.setup.lockFile || "the lock file"} changed since the last install · Prepare it in Project settings → Hosts`
                : "ready"
            }`;
  const setupProfile = setupHosts.find((item) => item.id === setupPick);
  const pick = (next: { account?: string; profile?: string }) => {
    accountTouched.current = true;
    setSelectedClaudeAccountId(next.account || "");
    setSelectedModelProfileId(next.profile || "");
    setAccountMenu(false);
  };
  const subscriptions = claudeAccounts.filter(
    (item) => item.kind === "subscription",
  );
  const keys = claudeAccounts.filter((item) => item.kind === "apiKey");
  const remoteNote = targetIsSsh ? launchLabel : "";
  const codexAccount = codexAccounts.find(
    (item) => item.id === selectedCodexAccountId,
  );
  const codexLabel = codexAccount?.label || "Signed-in account";
  const pickCodex = (id: string) => {
    codexTouched.current = true;
    setSelectedCodexAccountId(id);
    setCodexMenu(false);
  };

  if (starting && setupProfile && project)
    return (
      <div className="pk">
        <PrepareAndStart
          project={project}
          hostName={setupProfile.name}
          endpoint={`ssh:${setupProfile.id}`}
          onStart={(path) =>
            hostContext.startOnHost(
              project.name,
              path,
              `ssh:${setupProfile.id}`,
              starting,
            )
          }
          onCancel={() => setStarting(null)}
        />
      </div>
    );

  return (
    <div className="pk">
      <header className="pk-head">
        <div className="pk-head-text">
          <span className="pk-eyebrow">New session</span>
          <h2>
            {choices.length > 1 ? (
              <button
                type="button"
                className="pk-project"
                aria-haspopup="menu"
                aria-expanded={projectMenu}
                onClick={() => setProjectMenu((open) => !open)}
              >
                {project?.name || active.name}
                <ChevronDown size={14} aria-hidden />
              </button>
            ) : (
              project?.name || active.name
            )}
          </h2>
          {projectMenu && (
            <div className="pk-project-menu" role="menu" aria-label="Projects">
              {choices.map((choice) => (
                <button
                  key={choice.project.id}
                  type="button"
                  role="menuitemradio"
                  aria-checked={choice.workspace.id === active.id}
                  onClick={() => {
                    setBaseId(choice.workspace.id);
                    setHostId(choice.workspace.id);
                    setSetupPick("");
                    setProjectMenu(false);
                  }}
                >
                  <span className="pk-project-text">
                    <strong>{choice.project.name}</strong>
                    <small>{tilde(choice.workspace.cwd, system?.home)}</small>
                  </span>
                  {choice.workspace.id === active.id && (
                    <Check size={13} aria-hidden />
                  )}
                </button>
              ))}
            </div>
          )}
        </div>
        {project && (
          <span className="pk-env">
            <Lock size={12} aria-hidden />
            {`${project.env.length} variables · ${secrets} secrets · ${servers} MCP`}
          </span>
        )}
      </header>
      <section className="pk-where">
        <div className="pk-hosts" role="radiogroup" aria-label="Launch on">
          {hosts.map((host) => (
            <button
              key={host.workspaceId}
              role="radio"
              aria-checked={!setupPick && hostId === host.workspaceId}
              className={`pk-host${!setupPick && hostId === host.workspaceId ? " selected" : ""}`}
              onClick={() => {
                setSetupPick("");
                setHostId(host.workspaceId);
              }}
            >
              <span className={`ui-dot ui-tone-${hostTone(host.workspace)}`} />
              {host.label}
            </button>
          ))}
          {setupHosts.map((profile) => (
            <button
              key={profile.id}
              role="radio"
              aria-checked={setupPick === profile.id}
              className={`pk-host${setupPick === profile.id ? " selected" : ""}`}
              title={`${project?.name || "This project"} is not set up on ${profile.name}. Starting a session there prepares it first.`}
              onClick={() => project && setSetupPick(profile.id)}
            >
              <span className="ui-dot ui-tone-danger" />
              {profile.name}
            </button>
          ))}
        </div>
        {canWorktree && (
          <label className={`pk-worktree${wantsWorktree ? " on" : ""}`}>
            <input
              type="checkbox"
              checked={wantsWorktree}
              onChange={(event) =>
                setCheckout(event.target.checked ? "worktree" : "current")
              }
            />
            <span className="pk-box" aria-hidden>
              {wantsWorktree && <Check size={11} strokeWidth={3} />}
            </span>
            New worktree
          </label>
        )}
      </section>
      {wantsWorktree ? (
        <>
          <div className="pk-worktree-line">
            <label className="pk-base">
              <span>Base</span>
              <select
                aria-label="Base branch"
                value={baseReady ? base : ""}
                disabled={!baseReady || !branches.length}
                onChange={(event) => setBase(event.target.value)}
              >
                {!baseReady ? (
                  <option value="">Loading branches…</option>
                ) : (
                  <>
                    {!base && <option value="">Choose a base branch</option>}
                    {branches.map((branch) => (
                      <option key={branch.ref} value={branch.ref}>
                        {branch.remoteOnly ? branch.ref : branch.name}
                      </option>
                    ))}
                  </>
                )}
              </select>
            </label>
            <span className="pk-branch">
              <GitBranch size={12} aria-hidden />
              <input
                aria-label="Branch"
                value={branch}
                onChange={(event) => setBranch(event.target.value)}
                placeholder="feature/my-change"
              />
            </span>
          </div>
          <p className="pk-info">
            {branchError || baseMessage ? (
              <span className="pk-error">{branchError || baseMessage}</span>
            ) : !baseReady ? (
              "Loading base branches…"
            ) : (
              `New worktree from ${selectedBase?.name || base} on ${launchLabel}`
            )}
          </p>
        </>
      ) : (
        <p className="pk-info">{info}</p>
      )}
      <div className={`pk-lists${adding ? " is-busy" : ""}`}>
        <section className="pk-agents" aria-label="Coding agents">
          <div className="pk-label">Coding agents</div>
          {AGENTS.map((agent, index) => {
            const detail = agentDetail(agent);
            return (
              <div
                key={agent}
                className={`pk-row${focusedAgent === index ? " focused" : ""}`}
              >
                <button
                  ref={(element) => {
                    agentButtons.current[index] = element;
                  }}
                  className="pk-row-main"
                  disabled={adding || worktreeInvalid}
                  onFocus={() => setFocusedAgent(index)}
                  onClick={() => launchAgent(agent)}
                >
                  <span
                    className={`pk-glyph${agent === "claude" ? " claude" : agent === "gemini" ? " gemini" : ""}`}
                    aria-hidden
                  >
                    {agent === "claude" ? (
                      <Icon kind="agent" agent="claude" size={17} />
                    ) : (
                      GLYPHS[agent]
                    )}
                  </span>
                  <span className="pk-text">
                    <strong>{agentTitle(agent)}</strong>
                    <small className={detail.tone ? `is-${detail.tone}` : ""}>
                      {detail.text}
                    </small>
                  </span>
                </button>
                {agent === "claude" && (
                  <span className="pk-account">
                    <button
                      type="button"
                      className={`picker-account-chip${accountMenu ? " open" : ""}`}
                      aria-haspopup="menu"
                      aria-expanded={accountMenu}
                      aria-label={`Claude Code account: ${accountLabel}`}
                      onClick={() => setAccountMenu((open) => !open)}
                    >
                      {accountLabel}
                      <ChevronDown size={11} aria-hidden />
                    </button>
                    {accountMenu && (
                      <div
                        className="picker-account-menu"
                        role="menu"
                        aria-label="Claude Code runs as"
                        onKeyDown={(event) => {
                          if (event.key === "Escape") {
                            event.stopPropagation();
                            setAccountMenu(false);
                          }
                        }}
                      >
                        <div className="pk-menu-group">Claude Code runs as</div>
                        {subscriptions.map((item) => (
                          <button
                            key={item.id}
                            role="menuitemradio"
                            aria-checked={
                              selectedClaudeAccountId === item.id &&
                              !selectedModelProfileId
                            }
                            className="pk-menu-item"
                            onClick={() =>
                              pick(
                                selectedClaudeAccountId === item.id
                                  ? {}
                                  : { account: item.id },
                              )
                            }
                          >
                            <span className="pk-text">
                              <strong>{item.label}</strong>
                              <small
                                className={item.hasValue ? "" : "is-warning"}
                              >
                                {item.hasValue
                                  ? remoteNote
                                    ? `Subscription · token sent to ${remoteNote}`
                                    : "Subscription · logged in"
                                  : remoteNote
                                    ? "Subscription · paste its token in Settings first"
                                    : "Subscription · not logged in · logs in on first run"}
                              </small>
                            </span>
                            {selectedClaudeAccountId === item.id &&
                              !selectedModelProfileId && (
                                <Check size={12} aria-hidden />
                              )}
                          </button>
                        ))}
                        {(keys.length > 0 || modelProfiles.length > 0) && (
                          <>
                            <div className="pk-menu-rule" />
                            <div className="pk-menu-group">
                              API keys from Providers
                            </div>
                          </>
                        )}
                        {keys.map((item) => (
                          <button
                            key={item.id}
                            role="menuitemradio"
                            aria-checked={
                              selectedClaudeAccountId === item.id &&
                              !selectedModelProfileId
                            }
                            className="pk-menu-item"
                            onClick={() =>
                              pick(
                                selectedClaudeAccountId === item.id
                                  ? {}
                                  : { account: item.id },
                              )
                            }
                          >
                            <span className="pk-text">
                              <strong>{item.label}</strong>
                              <small>
                                {remoteNote
                                  ? `Key sent to ${remoteNote} like a project secret`
                                  : `API key${item.hint ? ` · ${item.hint}` : ""}`}
                              </small>
                            </span>
                            {selectedClaudeAccountId === item.id &&
                              !selectedModelProfileId && (
                                <Check size={12} aria-hidden />
                              )}
                          </button>
                        ))}
                        {modelProfiles.map((item) => (
                          <button
                            key={item.id}
                            role="menuitemradio"
                            aria-checked={selectedModelProfileId === item.id}
                            className="pk-menu-item"
                            onClick={() =>
                              pick(
                                selectedModelProfileId === item.id
                                  ? {}
                                  : { profile: item.id },
                              )
                            }
                          >
                            <span className="pk-text">
                              <strong>{item.label}</strong>
                              <small>
                                {remoteNote
                                  ? `Custom model · key sent to ${remoteNote}`
                                  : "Custom model"}
                              </small>
                            </span>
                            {selectedModelProfileId === item.id && (
                              <Check size={12} aria-hidden />
                            )}
                          </button>
                        ))}
                        <div className="pk-menu-rule" />
                        <button
                          role="menuitem"
                          className="pk-menu-foot"
                          onClick={() => openSettings("providers")}
                        >
                          Add a subscription or a key in Settings → Providers
                        </button>
                      </div>
                    )}
                  </span>
                )}
                {agent === "codex" && (
                  <span className="pk-account">
                    <button
                      type="button"
                      className={`picker-account-chip${codexMenu ? " open" : ""}`}
                      aria-haspopup="menu"
                      aria-expanded={codexMenu}
                      aria-label={`Codex account: ${codexLabel}`}
                      onClick={() => setCodexMenu((open) => !open)}
                    >
                      {codexLabel}
                      <ChevronDown size={11} aria-hidden />
                    </button>
                    {codexMenu && (
                      <div
                        className="picker-account-menu"
                        role="menu"
                        aria-label="Codex runs as"
                        onKeyDown={(event) => {
                          if (event.key === "Escape") {
                            event.stopPropagation();
                            setCodexMenu(false);
                          }
                        }}
                      >
                        <div className="pk-menu-group">Codex runs as</div>
                        {codexAccounts.map((item) => (
                          <button
                            key={item.id}
                            role="menuitemradio"
                            aria-checked={selectedCodexAccountId === item.id}
                            className="pk-menu-item"
                            onClick={() =>
                              pickCodex(
                                selectedCodexAccountId === item.id
                                  ? ""
                                  : item.id,
                              )
                            }
                          >
                            <span className="pk-text">
                              <strong>{item.label}</strong>
                              <small
                                className={item.signedIn ? "" : "is-warning"}
                              >
                                {!item.signedIn
                                  ? "Not signed in · sign in from Settings first"
                                  : remoteNote
                                    ? item.mode === "chatgpt"
                                      ? `Login copied to ${remoteNote} for this session · a refresh comes back`
                                      : `Key sent to ${remoteNote} for this session`
                                    : `${item.mode === "apiKey" ? "API key" : "ChatGPT"}${item.detail ? ` · ${item.detail}` : ""}`}
                              </small>
                            </span>
                            {selectedCodexAccountId === item.id && (
                              <Check size={12} aria-hidden />
                            )}
                          </button>
                        ))}
                        {codexAccounts.length > 0 && (
                          <div className="pk-menu-rule" />
                        )}
                        <button
                          role="menuitem"
                          className="pk-menu-foot"
                          onClick={() => openSettings("providers")}
                        >
                          Add a Codex account in Settings → Providers
                        </button>
                      </div>
                    )}
                  </span>
                )}
                <kbd className="pk-kbd">{index + 1}</kbd>
              </div>
            );
          })}
        </section>
        <section className="pk-tools" aria-label="Tools">
          <div className="pk-label">Tools</div>
          {tools.map((item) => (
            <div className="pk-row" key={item.kind}>
              <button
                className="pk-row-main"
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
                    targetWorkspaceId,
                    launchesInWorktree(item.kind) ? worktreeArg : undefined,
                  )
                }
              >
                <item.icon size={17} aria-hidden className="pk-icon" />
                <span className="pk-text">
                  <strong>{item.title}</strong>
                  <small>{item.detail}</small>
                </span>
              </button>
              <kbd className="pk-kbd">{item.key.toUpperCase()}</kbd>
            </div>
          ))}
          {extensionTools.map((item, index) => (
            <div className="pk-row" key={item.key}>
              <button
                className="pk-row-main"
                disabled={adding}
                onClick={() =>
                  addExtensionPanel(
                    item.extensionId,
                    item.surfaceId,
                    targetWorkspaceId,
                  )
                }
              >
                <span className="pk-icon">
                  <ExtensionIcon icon={item.icon} size={17} />
                </span>
                <span className="pk-text">
                  <strong>{item.label}</strong>
                  <small>{item.description}</small>
                </span>
              </button>
              {extensionKeys[index] && (
                <kbd className="pk-kbd">
                  {extensionKeys[index]!.toUpperCase()}
                </kbd>
              )}
            </div>
          ))}
        </section>
      </div>
      <footer className="pk-foot">
        <span>
          {setupPick
            ? `${agentTitle(AGENTS[focusedAgent])} on ${setupProfile?.name}: it is prepared first, then the session starts`
            : wantsWorktree && !worktreeInvalid
              ? `${agentTitle(AGENTS[focusedAgent])} on ${launchLabel} in a new worktree on ${branch}`
              : `${agentTitle(AGENTS[focusedAgent])} on ${launchLabel}${project ? ", with this project’s variables, secrets and MCP servers" : ""}`}
        </span>
        <kbd className="pk-kbd">⏎</kbd>
        <span className="pk-foot-text">
          {setupProfile ? `Prepare ${setupProfile.name} and start` : "start"}
        </span>
        <kbd className="pk-kbd">esc</kbd>
      </footer>
    </div>
  );
}
