const { contextBridge, ipcRenderer, webUtils } = require("electron");
const invoke =
  (channel) =>
  (...args) =>
    ipcRenderer.invoke(channel, ...args);
contextBridge.exposeInMainWorld("bridge", {
  agentProviders: invoke("agent-providers"),
  agentCall: invoke("agent-call"),
  agentOpenExternal: invoke("agent-open-external"),
  onAgents: (callback) => {
    const listener = (_, data) => callback(data);
    ipcRenderer.on("agent-event", listener);
    return () => ipcRenderer.removeListener("agent-event", listener);
  },
  system: invoke("system"),
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
  chooseDirectory: invoke("choose-directory"),
  chooseAttachments: invoke("choose-attachments"),
  pathForFile: (file) => webUtils.getPathForFile(file),
  terminalOpen: invoke("terminal-open"),
  terminalWrite: invoke("terminal-write"),
  terminalAttach: invoke("terminal-attach"),
  terminalResize: invoke("terminal-resize"),
  terminalClose: invoke("terminal-close"),
  terminalScroll: invoke("terminal-scroll"),
  herdr: invoke("herdr"),
  chat: invoke("chat"),
  chatModels: invoke("chat-models"),
  cancelChat: invoke("chat-cancel"),
  catalog: invoke("catalog"),
  skillsManage: invoke("skills-manage"),
  claudeMcpList: invoke("claude-mcp-list"),
  claudeMcpToggle: invoke("claude-mcp-toggle"),
  claudePluginsList: invoke("claude-plugins-list"),
  claudePluginsToggle: invoke("claude-plugins-toggle"),
  providersList: invoke("providers-list"),
  providersUpsert: invoke("providers-upsert"),
  providersDelete: invoke("providers-delete"),
  providersSetKey: invoke("providers-set-key"),
  providersClearKey: invoke("providers-clear-key"),
  providersTest: invoke("providers-test"),
  providersModels: invoke("providers-models"),
  modelProfilesList: invoke("model-profiles-list"),
  modelProfilesUpsert: invoke("model-profiles-upsert"),
  modelProfilesDelete: invoke("model-profiles-delete"),
  modelSettingsStage: invoke("model-settings-stage"),
  window: invoke("window"),
  connectionsList: invoke("connections-list"),
  connectionsSave: invoke("connections-save"),
  connectionsDelete: invoke("connections-delete"),
  connectionsConnect: invoke("connections-connect"),
  connectionsDisconnect: invoke("connections-disconnect"),
  connectionsForward: invoke("connections-forward"),
  projectInspect: invoke("project-inspect"),
  projectPreview: invoke("project-preview"),
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
