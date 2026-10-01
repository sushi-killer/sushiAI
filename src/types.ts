export type PanelKind =
  "agent" | "terminal" | "browser" | "chat" | "files" | "orchestrator";
export type ConnectionProfile = {
  id: string;
  name: string;
  host: string;
  port?: number;
  socket: string;
  connected?: boolean;
  /** Hidden from the workspace sidebar - the tunnel itself is unaffected. */
  hidden?: boolean;
  /** Reconnect automatically on app launch - set on explicit Connect, cleared
   * on explicit Disconnect. */
  autoConnect?: boolean;
};
export type Message = {
  id: string;
  role: "user" | "assistant";
  text: string;
  attachments?: string[];
  /** The model that actually answered, as the CLI resolved it. */
  model?: string;
};
/** What the Orchestrator panel shows: home (the default), the plan, its
 * chat, one task, improvements, analytics, the archive or the
 * agent messages. Kept on the panel so a restart reopens the same view. */
export type OrchestratorView =
  | { kind: "home" }
  | { kind: "plan" }
  | { kind: "chat" }
  | { kind: "task"; id: string }
  | { kind: "improvements" }
  | { kind: "analytics" }
  | { kind: "archive" }
  | { kind: "messages" };
/** Where a Files pane was browsing: its root folder, the folder listed and the
 * open file ("" for none). Not part of the panel's remount key. */
export type FilesView = { root: string; directory: string; file: string };
type PanelState = {
  id: string;
  title: string;
  agent?: string;
  started?: boolean;
  herdrId?: string;
  status?: string;
  url?: string;
  messages?: Message[];
  busy?: boolean;
  error?: string;
  previewFile?: { root: string; path: string; endpoint?: string };
  filesTarget?: {
    root: string;
    path: string;
    endpoint?: string;
    edit?: boolean;
    openToken?: number;
  };
  orchestratorView?: OrchestratorView;
  /** The host ("local" or "ssh:<id>") and the repo path on it this Orchestrator
   * pane was pointed at; unset follows the workspace. */
  orchestratorHost?: string;
  orchestratorRepo?: string;
  filesView?: FilesView;
  /** A Herdr pane that is gone from its host: the slot stays in the layout
   * with a Reopen button until the user reopens or closes it. */
  ended?: boolean;
  pinned?: boolean;
  updatedAt?: number;
  note?: string;
  usage?: ChatUsage;
  /** What the CLI reported using for the last turn, which may differ from `model`. */
  resolvedModel?: string;
  model?: string;
  effort?: string;
  permission?: string;
  /** Set when this agent panel was launched against a custom model provider. */
  modelProfileId?: string;
  claudeAccountId?: string;
};
export type CorePanel = PanelState & { kind: PanelKind };
export type ExtensionPanel = PanelState & {
  kind: "extension";
  extension: {
    extensionId: string;
    contributionId: string;
    instanceId: string;
    stateVersion: number;
  };
};
export type Panel = CorePanel | ExtensionPanel;
export type ModelProviderKind = "openrouter" | "opencode-go" | "custom";
export type ClaudeAccount = {
  id: string;
  label: string;
  kind: "subscription" | "apiKey";
  hint: string;
  hasValue: boolean;
};
export type ModelProvider = {
  id: string;
  kind: ModelProviderKind;
  label: string;
  baseUrl: string;
  hasKey: boolean;
  /** True when the key could not be Keychain-encrypted and is stored plainly. */
  keyPlaintext: boolean;
};
export type ModelProfile = {
  id: string;
  providerId: string;
  modelId: string;
  label: string;
  effort?: string;
  contextWindow?: number;
};
export type ProviderModel = { id: string; label: string; context?: number };
export type ProviderTestResult = {
  ok: boolean;
  code: "ok" | "no_key" | "unauthorized" | "network" | "unknown";
  message: string;
};
export type Layout =
  | { type: "leaf"; id: string }
  | {
      type: "split";
      id: string;
      axis: "row" | "column";
      ratio: number;
      a: Layout;
      b: Layout;
    };
export type Workspace = {
  id: string;
  name: string;
  cwd: string;
  herdrId?: string;
  connection?: string;
  panels: Panel[];
  layout: Layout | null;
};
export type Project = {
  /** The folder name under ~/sushiai on a host; fixed when the project is made. */
  slug?: string;
  /** Folders attached to the project (by host and path). */
  folders?: { endpoint: string; cwd: string }[];
  id: string;
  name: string;
  git: { url: string; defaultBranch: string };
  env: {
    name: string;
    secret: boolean;
    hasValue?: boolean;
    hint?: string;
    availableTo?: string[];
    hosts?: string[];
  }[];
  mcp: Record<string, unknown>;
  setup: { install: string; check: string };
  network: { allowedDomains: string[] };
  sessions: { claudeAccount?: string; backend?: "herdr" | "local" };
  targets: string[];
  hosts?: Record<
    string,
    { withheld?: boolean; overrides?: Record<string, unknown> }
  >;
};
/** One step of preparing a host: what it is, how it went, and how long. */
export type ProjectPrepareStep = {
  id: "clone" | "install" | "check";
  state: "done" | "failed" | "pending";
  seconds?: number;
};
/** What an import added: names and flags, never values. */
export type ProjectImportResult = {
  project: Project;
  addedVariables: { name: string; secret: boolean }[];
  /** Variables that existed without a value and got the folder's. */
  filledVariables?: string[];
  skippedVariables?: string[];
  addedServers: string[];
  skippedServers?: string[];
  /** Removed by the owner earlier, so not brought back. */
  removedVariables?: string[];
  removedServers?: string[];
};
/** What pulling a folder into its project would change. */
export type ProjectImportPreview = {
  newVariables: string[];
  fillVariables: string[];
  /** Removed by the owner earlier; a forced pull brings them back. */
  removedVariables: string[];
  newServers: string[];
  removedServers: string[];
};
export type ProjectHostReadiness = {
  /** `uname -sm` of the host, e.g. "Linux x86_64". */
  platform?: string;
  checkout: { ok: boolean; path: string; nonStandard: boolean };
  setup: {
    ok: boolean;
    configured: boolean;
    /** The next run reinstalls: the lock file changed since the last install. */
    stale?: boolean;
    lockFile?: string;
  };
  clis: import("./orchestrator/types.ts").Preflight;
  mcp: {
    ok: boolean;
    count: number;
    /** Enabled stdio servers whose command the host does not have. */
    missing?: { name: string; command: string }[];
  };
  secrets: { ok: boolean; count: number };
  /** The owner switched sending this project's values to the host off. */
  withheld: boolean;
};
export type Snapshot = {
  version: string;
  workspaces: {
    workspace_id: string;
    label: string;
    worktree?: { checkout_path: string };
  }[];
  panes: {
    pane_id: string;
    workspace_id: string;
    cwd?: string;
    label?: string;
    agent?: string;
    agent_status: string;
    terminal_title_stripped?: string;
  }[];
};
export type System = {
  home: string;
  cwd: string;
  socketPath: string;
  platform: string;
  agents: { name: string; path: string | null }[];
};
export type ChatUsage = {
  input: number;
  output: number;
  cached: number;
  /** The window the CLI reported for this turn, when it reports one. */
  context?: number;
};
export type ChatEvent = {
  panelId: string;
  text?: string;
  /** One line of live progress from the agent, replaced as the turn advances. */
  note?: string;
  model?: string;
  usage?: ChatUsage;
  done?: boolean;
  error?: string;
};
export type ChatModel = {
  id: string;
  label: string;
  description: string;
  efforts: string[];
  defaultEffort: string;
  context: number;
};
export type SkillProvider = "Codex" | "Claude" | "Agent" | "Other";
export type SkillUsage = {
  at: number;
  project?: string;
  session?: string;
};
export type SkillCatalogItem = {
  name: string;
  description: string;
  path?: string;
  provider?: SkillProvider;
  harness?: string;
  source?: string;
  availability?: "active" | "disabled" | "external";
  disabledBy?: "provider" | "skill-config" | "skill-override";
  plugin?: string;
  pluginScope?: "user" | "project" | "local";
  updatedAt?: number;
  lastUsedAt?: number;
  lastUsedSource?: "skill event" | "usage cache" | "filesystem access";
  usageCount?: number;
  usageSource?: "skill events" | "usage cache" | "filesystem access";
  recentUses?: SkillUsage[];
  size?: number;
  changed?: boolean;
  isRecent?: boolean;
  isUnused?: boolean;
  isStale?: boolean;
  isDuplicate?: boolean;
  duplicateKind?: "exact" | "name";
  duplicateCount?: number;
  duplicateWith?: string[];
  needsReview?: boolean;
};
export type SkillManagementAction = "enable" | "disable" | "delete";
export type ClaudeMcpServer = {
  name: string;
  source: "local" | "project" | "user" | "saved";
  sourceLabel: string;
  disabled: boolean;
};
export type ClaudePlugin = {
  name: string;
  source: "local" | "project" | "user" | "installed";
  sourceLabel: string;
  disabled: boolean;
};
export type ChatModels = Record<
  string,
  {
    models: ChatModel[];
    efforts: string[];
    defaultModel: string;
    defaultModelId?: string;
    defaultContext?: number;
    defaultEffort?: string;
  }
>;
export type UpdateSettings = {
  autoCheck: boolean;
  autoDownload: boolean;
  includePrereleases: boolean;
};
export type UpdateState = {
  canInstall: boolean;
  currentVersion: string;
  repository: string;
  settings: UpdateSettings;
  phase:
    | "idle"
    | "checking"
    | "available"
    | "downloading"
    | "installing"
    | "ready"
    | "current"
    | "error";
  release: {
    version: string;
    name: string;
    size: number;
    notes: string;
  } | null;
  progress: number;
  checkedAt: string | null;
  error: string | null;
};
/** A panel an attention event points at: a notification click opens it. */
export type AttentionTarget = { workspaceId: string; panelId: string };
export type AttentionNotice = AttentionTarget & { title: string; body: string };
/** Kept by the main process, which needs both before any window exists. */
export type AppPreferences = {
  /** Closing the window hides it; sushiAI stays in the menu bar. */
  runInMenuBar: boolean;
  /** macOS notifications for agents that need input or finished. */
  notifications: boolean;
  /** Shows orchd task notices in a desktop mascot instead of a native
   * notification (needs notifications on). */
  desktopMascot: boolean;
  /** Opt-in global shortcut that toggles the mascot. */
  mascotShortcut: boolean;
};
export type MascotShortcutStatus = {
  accelerator: string;
  registered: boolean;
  /** The OS refused the accelerator, usually because another app holds it. */
  failed: boolean;
};
export interface Bridge {
  /** The workspace snapshot text from <userData>/workspace-state.json, or
   * null when none is stored. Synchronous: restore() runs before first paint. */
  workspaceStateRead(): string | null;
  /** Debounced snapshot write; the main process writes it atomically. */
  workspaceStateWrite(text: string): Promise<void>;
  /** Synchronous write for when the window is going away. */
  workspaceStateFlush(text: string): void;
  /** Shows a macOS notification unless notifications are off or the window
   * is focused. Clicking it shows the window and fires `onAttentionOpen`. */
  attentionNotify(notice: AttentionNotice): Promise<void>;
  /** Waiting count for the Dock badge and the menu bar title; 0 clears both. */
  attentionBadge(count: number, working: number): Promise<void>;
  onAttentionOpen(callback: (target: AttentionTarget) => void): () => void;
  /** Adds a git worktree on a new branch next to the repository holding
   * `cwd`, on this Mac. Resolves to the new checkout's path. */
  worktreeCreate(cwd: string, branch: string): Promise<{ path: string }>;
  appPreferences(): Promise<AppPreferences>;
  appPreferencesSet(patch: Partial<AppPreferences>): Promise<AppPreferences>;
  mascotShortcutStatus(): Promise<MascotShortcutStatus>;
  agentProviders(): Promise<import("./agents/types").AgentProvider[]>;
  agentCall: import("./agents/types").AgentCall;
  agentOpenExternal(url: string): Promise<void>;
  onAgents(
    callback: (event: import("./agents/types").AgentEvent) => void,
  ): () => void;
  /** Holds or releases the main process's power-save blocker. Resolves to
   * whether the blocker is active afterwards. */
  keepAwake(on: boolean): Promise<boolean>;
  updatesState(): Promise<UpdateState>;
  updatesCheck(): Promise<UpdateState>;
  updatesDownload(): Promise<UpdateState>;
  updatesConfigure(settings: Partial<UpdateSettings>): Promise<UpdateState>;
  updatesOpen(): Promise<void>;
  updatesInstall(): Promise<UpdateState>;
  updatesReleasePage(): Promise<void>;
  onUpdates(callback: (state: UpdateState) => void): () => void;
  system(): Promise<System>;
  chooseDirectory(): Promise<string | null>;
  chooseAttachments(): Promise<string[]>;
  pathForFile(file: File): string;
  /** What to type into a Herdr pane so the project's values reach its shell. */
  projectSessionEnv(options: {
    endpoint: string;
    cwd: string;
  }): Promise<{ prefix: string }>;
  terminalOpen(options: {
    panelId: string;
    cwd: string;
    command?: string;
    cols?: number;
    rows?: number;
    endpoint?: string;
    herdrId?: string;
    modelProfileId?: string;
    claudeAccountId?: string;
  }): Promise<{ history: string; exited?: boolean }>;
  terminalWrite(panelId: string, data: string): Promise<void>;
  terminalAttach(input: {
    panelId: string;
    name: string;
    data: string;
  }): Promise<string>;
  terminalResize(panelId: string, cols: number, rows: number): Promise<void>;
  terminalClose(panelId: string): Promise<void>;
  terminalScroll(
    panelId: string,
    direction: string,
    lines: number,
    position?: { column: number; row: number; fast?: boolean },
  ): Promise<void>;
  herdr(
    socket: string,
    method: string,
    params?: Record<string, unknown>,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- generic RPC passthrough, callers narrow the result themselves
  ): Promise<any>;
  /** Raw NDJSON-RPC passthrough to the orchestrator daemon; the renderer's
   * typed wrapper is `src/orchestrator/client.ts`. */
  orchestrator(
    method: string,
    params?: Record<string, unknown>,
    /** The host whose daemon answers: "local" (the default) or "ssh:<id>". */
    host?: string,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- generic RPC passthrough, callers narrow the result themselves
  ): Promise<any>;
  orchestratorHosts(): Promise<
    import("./orchestrator/types.ts").OrchestratorHost[]
  >;
  orchestratorPreflight(
    host: string,
  ): Promise<import("./orchestrator/types.ts").Preflight | null>;
  /** The owner's one button: provisions the host, installing Rust there first
   * when orchd has to be built and cargo is missing. */
  orchestratorHostSetup(
    host: string,
  ): Promise<import("./orchestrator/types.ts").Preflight | null>;
  /** A ping of the host's current connection that never starts or
   * provisions its daemon. */
  orchestratorProbe(host: string): Promise<{ pid: number }>;
  onOrchestratorHosts(callback: () => void): () => void;
  onOrchestrator(
    callback: (
      event: import("./orchestrator/types.ts").OrchestratorEvent,
    ) => void,
  ): () => void;
  /** The owner clicked a native orchd notification or the desktop mascot's
   * Open button. */
  onOrchestratorOpen(
    callback: (target: import("./orchestrator/notices.ts").TaskTarget) => void,
  ): () => void;
  /** The desktop mascot's Answer all in Inbox. */
  onOpenInbox(callback: () => void): () => void;
  chat(options: {
    panelId: string;
    cwd: string;
    agent: string;
    messages: Message[];
    endpoint?: string;
    model?: string;
    effort?: string;
    permission?: string;
  }): Promise<unknown>;
  cancelChat(panelId: string): Promise<void>;
  chatModels(): Promise<ChatModels>;
  onTerminal(
    callback: (event: {
      panelId: string;
      data: string;
      exitCode?: number;
      agent?: string | null;
    }) => void,
  ): () => void;
  onChat(callback: (event: ChatEvent) => void): () => void;
  catalog(
    kind: string,
    options?: { force?: boolean },
  ): Promise<SkillCatalogItem[]>;
  skillsManage(
    action: SkillManagementAction,
    item: SkillCatalogItem,
  ): Promise<{
    action: SkillManagementAction;
    changed: boolean;
    message: string;
  }>;
  claudeMcpList(
    cwd: string,
    endpoint?: string,
  ): Promise<{
    cwd: string;
    servers: ClaudeMcpServer[];
  }>;
  /** Calls per MCP server (and per plugin, with the servers seen) from this
   * project in the last 30 days. */
  claudeMcpUsage(cwd: string): Promise<{
    servers: Record<string, number>;
    plugins: Record<string, { uses: number; servers: string[] }>;
  }>;
  claudeMcpToggle(input: {
    cwd: string;
    endpoint?: string;
    name: string;
    source?: ClaudeMcpServer["source"];
    disabled: boolean;
  }): Promise<{
    cwd: string;
    servers: ClaudeMcpServer[];
  }>;
  claudePluginsList(
    cwd: string,
    endpoint?: string,
  ): Promise<{
    cwd: string;
    plugins: ClaudePlugin[];
  }>;
  claudePluginsToggle(input: {
    cwd: string;
    endpoint?: string;
    name: string;
    disabled: boolean;
  }): Promise<{
    cwd: string;
    plugins: ClaudePlugin[];
  }>;
  providersList(): Promise<ModelProvider[]>;
  providersUpsert(input: {
    id?: string;
    kind: ModelProviderKind;
    label?: string;
    baseUrl?: string;
  }): Promise<ModelProvider>;
  providersDelete(id: string): Promise<void>;
  providersSetKey(
    id: string,
    key: string,
  ): Promise<{ hasKey: boolean; keyPlaintext: boolean; keyHint: string }>;
  providersClearKey(id: string): Promise<void>;
  providersTest(id: string): Promise<ProviderTestResult>;
  providersModels(id: string): Promise<ProviderModel[]>;
  modelProfilesList(): Promise<ModelProfile[]>;
  modelProfilesUpsert(input: {
    id?: string;
    providerId: string;
    modelId: string;
    label?: string;
    effort?: string;
    contextWindow?: number;
  }): Promise<ModelProfile>;
  modelProfilesDelete(id: string): Promise<void>;
  modelSettingsStage(modelProfileId: string): Promise<string>;
  claudeAccountsList(): Promise<ClaudeAccount[]>;
  claudeAccountsUpsert(input: {
    id?: string;
    label?: string;
    kind: ClaudeAccount["kind"];
  }): Promise<ClaudeAccount>;
  claudeAccountValueSet(
    id: string,
    value: string,
  ): Promise<{ hasValue: boolean; hint: string }>;
  claudeAccountDelete(id: string): Promise<void>;
  extensionsList(): Promise<import("./extensions/types.ts").ExtensionSnapshot>;
  extensionsRefresh(): Promise<
    import("./extensions/types.ts").ExtensionSnapshot
  >;
  extensionsStateRead(
    extensionId: string,
    surfaceId: string,
    version: number,
    scope: string,
  ): Promise<unknown>;
  extensionsStateAggregate(
    extensionId: string,
    surfaceId: string,
    version: number,
  ): Promise<{ scope: string; records: unknown }[]>;
  extensionsStateWrite(
    extensionId: string,
    surfaceId: string,
    version: number,
    scope: string,
    value: unknown,
  ): Promise<void>;
  onExtensionState(
    callback: (change: {
      extensionId: string;
      surfaceId: string;
      version: number;
      scope: string;
    }) => void,
  ): () => void;
  extensionsSetEnabled(
    extensionId: string,
    enabled: boolean,
  ): Promise<import("./extensions/types.ts").ExtensionSnapshot>;
  window(action: string): Promise<void>;
  connectionsList(): Promise<ConnectionProfile[]>;
  connectionsSave(
    profile: Partial<ConnectionProfile>,
  ): Promise<ConnectionProfile>;
  connectionsSetHidden(
    endpoint: string,
    hidden: boolean,
  ): Promise<ConnectionProfile>;
  connectionsDelete(endpoint: string): Promise<void>;
  connectionsConnect(endpoint: string): Promise<void>;
  connectionsDisconnect(endpoint: string): Promise<void>;
  connectionsForward(endpoint: string, url: string): Promise<string>;
  projectsList(): Promise<Project[]>;
  projectsGet(id: string): Promise<Project | null>;
  projectsUpsert(
    project: Partial<Project> & { importToken?: string },
  ): Promise<Project>;
  projectsDelete(id: string): Promise<void>;
  /** Variables change through these, on the stored state, never by saving a
   * whole project: a stale copy cannot overwrite what an import added. */
  projectEnvUpdate(
    id: string,
    change: { set?: Project["env"]; remove?: string[] },
  ): Promise<Project>;
  /** Stores the clone token, adding a GIT_TOKEN variable when none exists. */
  projectGitTokenSet(id: string, value: string): Promise<Project>;
  projectMcpUpdate(
    id: string,
    change: {
      set?: Record<string, Record<string, unknown>>;
      remove?: string[];
      disabled?: string[];
    },
  ): Promise<Project>;
  projectSecretSet(
    id: string,
    name: string,
    value: string,
  ): Promise<{ hasValue: boolean; hint: string }>;
  projectHostSecretSet(
    id: string,
    name: string,
    host: string,
    value: string,
  ): Promise<{ hasValue: boolean; hint: string }>;
  projectSecretClear(id: string, name: string): Promise<void>;
  /** "Don't send secrets to this host": it gets no value of the project and
   * its daemon drops what it holds. */
  projectHostWithhold(
    id: string,
    host: string,
    withheld: boolean,
  ): Promise<{ withheld: boolean }>;
  projectHostOverrides(
    id: string,
    host: string,
    overrides: Record<string, unknown>,
  ): Promise<Record<string, unknown>>;
  projectHostCheck(
    id: string,
    host: string,
    cwd?: string,
  ): Promise<ProjectHostReadiness>;
  /** Cheap check before a start: the project is on the host and installed. */
  projectHostReady(
    id: string,
    host: string,
  ): Promise<{ ready: boolean; path?: string; reason?: string }>;
  projectHostPrepare(
    id: string,
    host: string,
    useHostLogin?: boolean,
    /** `pull: false` leaves an existing checkout's history alone. */
    options?: { pull?: boolean },
  ): Promise<
    | {
        ok: true;
        path: string;
        output: string;
        pull: string;
        message: string;
        steps: ProjectPrepareStep[];
      }
    | {
        ok: false;
        stage: "clone" | "setup";
        /** The 15 minute limit ran out; nothing says the host refused. */
        timedOut?: boolean;
        status?: number;
        message: string;
        steps: ProjectPrepareStep[];
      }
  >;
  /** Parses a `.env` file the owner chose and says, per variable, whether it
   * is a secret and how it compares with what the project already has. */
  projectEnvReviewText(
    id: string,
    text: string,
  ): Promise<
    {
      name: string;
      value: string;
      secret: boolean;
      status: "exists" | "differs" | "new" | "same";
    }[]
  >;
  /** Whether each variable name looks like a secret (the one classifier). */
  projectEnvClassify(names: string[]): Promise<boolean[]>;
  /** Reads a local checkout's .env files, .mcp.json and Claude config into
   * the project, adding what is missing. Values stay in the main process. */
  projectImportLocal(
    id: string,
    cwd: string,
    options?: { endpoint?: string; force?: boolean },
  ): Promise<ProjectImportResult>;
  projectImportPreview(
    id: string,
    cwd: string,
    options?: { endpoint?: string },
  ): Promise<ProjectImportPreview>;
  /** The project of a folder, made when it has none: by its git remote, else
   * by its host and path. */
  projectAttach(input: {
    endpoint?: string;
    cwd: string;
    name?: string;
  }): Promise<Project>;
  projectImportMcpText(id: string, text: string): Promise<ProjectImportResult>;
  /** Reads a source before its project exists. Secret values are held in the
   * main process under `token`; pass it to `projectsUpsert` as `importToken`. */
  projectScanSource(input: {
    endpoint?: string;
    root?: string;
    local?: boolean;
    example?: string;
    mcp?: string;
  }): Promise<{
    token: string;
    /** The install command the source's lock file implies, or empty. */
    install: string;
    servers: Record<string, Record<string, unknown>>;
    variables: {
      name: string;
      secret: boolean;
      availableTo?: string[];
      value?: string;
      held?: boolean;
    }[];
  }>;
  projectsResolve(
    remote: string | { remote?: string; endpoint?: string; cwd?: string },
  ): Promise<Project | null>;
  projectInspect(
    endpoint: string | undefined,
    options: Record<string, unknown>,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- generic inspection result, callers narrow it themselves
  ): Promise<any>;
  projectLocalCreate(input: {
    url?: string;
    cwd: string;
    branch?: string;
    empty?: boolean;
  }): Promise<{ cwd: string; pull: string; message: string }>;
  projectLocalInstall(
    id: string,
    cwd: string,
  ): Promise<{ ran: boolean; seconds: number }>;
  projectSourceInspect(url: string): Promise<{
    branch: string;
    envExample: string;
    mcp: string;
    lockFile: string;
    /** The install command that lock file implies, or empty. */
    install: string;
  }>;
  projectPreview(
    endpoint: string | undefined,
    root: string,
    path: string,
  ): Promise<string>;
}
declare global {
  interface Window {
    bridge?: Bridge;
  }
}
