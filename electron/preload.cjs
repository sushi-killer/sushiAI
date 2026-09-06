const { contextBridge, ipcRenderer } = require("electron");
const invoke =
  (channel) =>
  (...args) =>
    ipcRenderer.invoke(channel, ...args);
contextBridge.exposeInMainWorld("bridge", {
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
  terminalOpen: invoke("terminal-open"),
  terminalWrite: invoke("terminal-write"),
  terminalResize: invoke("terminal-resize"),
  terminalClose: invoke("terminal-close"),
  terminalScroll: invoke("terminal-scroll"),
  herdr: invoke("herdr"),
  chat: invoke("chat"),
  cancelChat: invoke("chat-cancel"),
  catalog: invoke("catalog"),
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
