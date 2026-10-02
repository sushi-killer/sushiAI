const { contextBridge, ipcRenderer, webUtils } = require("electron");
const invoke =
  (channel) =>
  (...args) =>
    ipcRenderer.invoke(channel, ...args);
// The workspace snapshot is read and flushed synchronously: restore() runs
// before the first render, and a flush runs while the window is closing.
const sendSync = (channel, ...args) => {
  const reply = ipcRenderer.sendSync(channel, ...args);
  if (reply?.error) throw new Error(reply.error);
  return reply?.value ?? null;
};
contextBridge.exposeInMainWorld("nativeBridge", {
  workspaceStateRead: () => {
    try {
      return sendSync("workspace-state-read");
    } catch {
      return null;
    }
  },
  workspaceStateWrite: invoke("workspace-state-write"),
  workspaceStateFlush: (text) => {
    sendSync("workspace-state-flush", text);
  },
  agentProviders: invoke("agent-providers"),
  agentCall: invoke("agent-call"),
  agentOpenExternal: invoke("agent-open-external"),
  onAgents: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("agent-event", listener);
    return () => ipcRenderer.removeListener("agent-event", listener);
  },
  system: invoke("system"),
  keepAwake: invoke("keep-awake"),
  updatesState: invoke("updates-state"),
  updatesCheck: invoke("updates-check"),
  updatesDownload: invoke("updates-download"),
  updatesConfigure: invoke("updates-configure"),
  updatesOpen: invoke("updates-open"),
  updatesInstall: invoke("updates-install"),
  updatesReleasePage: invoke("updates-release-page"),
  onUpdates: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("updates-state", listener);
    return () => ipcRenderer.removeListener("updates-state", listener);
  },
  attentionNotify: invoke("attention-notify"),
  attentionBadge: invoke("attention-badge"),
  appPreferences: invoke("app-preferences"),
  appPreferencesSet: invoke("app-preferences-set"),
  mascotShortcutStatus: invoke("mascot-shortcut-status"),
  onAttentionOpen: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("attention-open", listener);
    return () => ipcRenderer.removeListener("attention-open", listener);
  },
  chooseDirectory: invoke("choose-directory"),
  chooseAttachments: invoke("choose-attachments"),
  pathForFile: (file) => webUtils.getPathForFile(file),
  terminalOpen: invoke("terminal-open"),
  projectSessionEnv: invoke("project-session-env"),
  terminalWrite: invoke("terminal-write"),
  terminalAttach: invoke("terminal-attach"),
  terminalResize: invoke("terminal-resize"),
  terminalClose: invoke("terminal-close"),
  terminalScroll: invoke("terminal-scroll"),
  terminalAck: invoke("terminal-ack"),
  herdr: invoke("herdr"),
  sessionLaunch: invoke("session-launch"),
  herdrCompatibility: invoke("herdr-compatibility"),
  herdrInstall: invoke("herdr-install"),
  herdrSubscribe: invoke("herdr-events-subscribe"),
  herdrUnsubscribe: invoke("herdr-events-unsubscribe"),
  onHerdr: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("herdr-event", listener);
    return () => ipcRenderer.removeListener("herdr-event", listener);
  },
  orchestrator: invoke("orchestrator"),
  onOrchestrator: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("orchestrator-event", listener);
    return () => ipcRenderer.removeListener("orchestrator-event", listener);
  },
  orchestratorHosts: invoke("orchestrator-hosts"),
  orchestratorPreflight: invoke("orchestrator-preflight"),
  orchestratorHostSetup: invoke("orchestrator-host-setup"),
  orchestratorProbe: invoke("orchestrator-probe"),
  onOrchestratorHosts: (callback) => {
    const listener = () => callback();
    ipcRenderer.on("orchestrator-hosts-changed", listener);
    return () =>
      ipcRenderer.removeListener("orchestrator-hosts-changed", listener);
  },
  onOrchestratorOpen: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("orchestrator-open", listener);
    return () => ipcRenderer.removeListener("orchestrator-open", listener);
  },
  onOpenInbox: (callback) => {
    const listener = () => callback();
    ipcRenderer.on("open-inbox", listener);
    return () => ipcRenderer.removeListener("open-inbox", listener);
  },
  chat: invoke("chat"),
  chatModels: invoke("chat-models"),
  cancelChat: invoke("chat-cancel"),
  catalog: invoke("catalog"),
  skillsManage: invoke("skills-manage"),
  claudeMcpList: invoke("claude-mcp-list"),
  claudeMcpUsage: invoke("claude-mcp-usage"),
  claudeMcpToggle: invoke("claude-mcp-toggle"),
  claudePluginsList: invoke("claude-plugins-list"),
  claudePluginsToggle: invoke("claude-plugins-toggle"),
  providersList: invoke("providers-list"),
  claudeAccountsList: invoke("claude-accounts-list"),
  claudeAccountsUpsert: invoke("claude-accounts-upsert"),
  claudeAccountValueSet: invoke("claude-accounts-value-set"),
  claudeAccountDelete: invoke("claude-accounts-delete"),
  codexAccountsList: invoke("codex-accounts-list"),
  codexAccountAdd: invoke("codex-accounts-add"),
  codexAccountLogin: invoke("codex-accounts-login"),
  codexAccountDelete: invoke("codex-accounts-delete"),
  providersUpsert: invoke("providers-upsert"),
  providersDelete: invoke("providers-delete"),
  providersSetKey: invoke("providers-set-key"),
  providersClearKey: invoke("providers-clear-key"),
  providersTest: invoke("providers-test"),
  providersModels: invoke("providers-models"),
  modelProfilesList: invoke("model-profiles-list"),
  modelProfilesUpsert: invoke("model-profiles-upsert"),
  modelProfilesDelete: invoke("model-profiles-delete"),
  modelLaunch: invoke("model-launch"),
  extensionsList: invoke("extensions-list"),
  extensionsRefresh: invoke("extensions-refresh"),
  extensionsStateRead: invoke("extensions-state-read"),
  extensionsStateAggregate: invoke("extensions-state-aggregate"),
  extensionsStateWrite: invoke("extensions-state-write"),
  onExtensionState: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("extensions-state-changed", listener);
    return () =>
      ipcRenderer.removeListener("extensions-state-changed", listener);
  },
  extensionsSetEnabled: invoke("extensions-set-enabled"),
  window: invoke("window"),
  connectionsList: invoke("connections-list"),
  connectionsSave: invoke("connections-save"),
  connectionsSetHidden: invoke("connections-set-hidden"),
  connectionsDelete: invoke("connections-delete"),
  connectionsConnect: invoke("connections-connect"),
  connectionsDisconnect: invoke("connections-disconnect"),
  connectionsForward: invoke("connections-forward"),
  projectInspect: invoke("project-inspect"),
  projectIdentify: invoke("projects:identify"),
  projectSourceInspect: invoke("projects:source-inspect"),
  projectLocalCreate: invoke("projects:local-create"),
  projectLocalInstall: invoke("projects:local-install"),
  projectsList: invoke("projects:list"),
  projectsGet: invoke("projects:get"),
  projectsUpsert: invoke("projects:upsert"),
  projectsDelete: invoke("projects:delete"),
  projectEnvUpdate: invoke("projects:env:update"),
  projectGitTokenSet: invoke("projects:git-token:set"),
  projectMcpUpdate: invoke("projects:mcp:update"),
  projectSecretSet: invoke("projects:secret:set"),
  projectHostSecretSet: invoke("projects:host:secret:set"),
  projectSecretClear: invoke("projects:secret:clear"),
  projectHostWithhold: invoke("projects:host:withhold"),
  projectHostReady: invoke("projects:host:ready"),
  projectHostOverrides: invoke("projects:host:overrides"),
  projectHostCheck: invoke("projects:host:check"),
  projectHostPrepare: invoke("projects:host:prepare"),
  projectHostGitKey: invoke("projects:host:git-key"),
  projectHostGitCopyKey: invoke("projects:host:git-copy-key"),
  projectHostGitValidate: invoke("projects:host:git-validate"),
  projectHostGitScan: invoke("projects:host:git-scan"),
  projectHostGitTrust: invoke("projects:host:git-trust"),
  projectEnvReviewText: invoke("projects:env:review-text"),
  projectEnvClassify: invoke("projects:env:classify"),
  projectImportLocal: invoke("projects:import-local"),
  projectImportPreview: (id, cwd, options) =>
    ipcRenderer.invoke("projects:import-local", id, cwd, {
      ...options,
      preview: true,
    }),
  projectImportMcpText: invoke("projects:import-mcp-text"),
  projectScanSource: invoke("projects:scan-source"),
  projectsResolve: invoke("projects:resolve"),
  projectAttach: invoke("projects:attach"),
  projectPreview: invoke("project-preview"),
  worktreeCreate: invoke("worktree-create"),
  worktreesList: invoke("worktrees:list"),
  worktreeRemove: invoke("worktrees:remove"),
  onTerminal: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("terminal-data", listener);
    return () => ipcRenderer.removeListener("terminal-data", listener);
  },
  onChat: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("chat-data", listener);
    return () => ipcRenderer.removeListener("chat-data", listener);
  },
});
