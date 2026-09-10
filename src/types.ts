export type PanelKind = "agent" | "terminal" | "browser" | "chat" | "files";
export type ConnectionProfile = {
  id: string;
  name: string;
  host: string;
  port?: number;
  socket: string;
  connected?: boolean;
};
export type Message = {
  id: string;
  role: "user" | "assistant";
  text: string;
  attachments?: string[];
  /** The model that actually answered, as the CLI resolved it. */
  model?: string;
};
export type Panel = {
  id: string;
  kind: PanelKind;
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
  pinned?: boolean;
  updatedAt?: number;
  note?: string;
  usage?: ChatUsage;
  /** What the CLI reported using for the last turn, which may differ from `model`. */
  resolvedModel?: string;
  model?: string;
  effort?: string;
  permission?: string;
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
export interface Bridge {
  agentProviders(): Promise<import("./agents/types").AgentProvider[]>;
  agentCall: import("./agents/types").AgentCall;
  agentOpenExternal(url: string): Promise<void>;
  onAgents(
    callback: (event: import("./agents/types").AgentEvent) => void,
  ): () => void;
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
  terminalOpen(options: {
    panelId: string;
    cwd: string;
    command?: string;
    cols?: number;
    rows?: number;
    endpoint?: string;
    herdrId?: string;
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
  ): Promise<any>;
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
  window(action: string): Promise<void>;
  connectionsList(): Promise<ConnectionProfile[]>;
  connectionsSave(
    profile: Partial<ConnectionProfile>,
  ): Promise<ConnectionProfile>;
  connectionsDelete(endpoint: string): Promise<void>;
  connectionsConnect(endpoint: string): Promise<void>;
  connectionsDisconnect(endpoint: string): Promise<void>;
  connectionsForward(endpoint: string, url: string): Promise<string>;
  projectInspect(
    endpoint: string | undefined,
    options: Record<string, unknown>,
  ): Promise<any>;
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
