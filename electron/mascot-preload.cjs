const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("mascot", {
  onNotices: (callback) => {
    const listener = (_event, notices) => callback(notices);
    ipcRenderer.on("mascot-notices", listener);
    // The page subscribes after main's first publish may have gone out.
    ipcRenderer.send("mascot-sync");
    return () => ipcRenderer.removeListener("mascot-notices", listener);
  },
  answer: (taskId, text) => ipcRenderer.invoke("mascot-answer", taskId, text),
  open: (taskId, focus) => ipcRenderer.invoke("mascot-open", taskId, focus),
  onToggle: (callback) => {
    const listener = () => callback();
    ipcRenderer.on("mascot-toggle", listener);
    return () => ipcRenderer.removeListener("mascot-toggle", listener);
  },
  onPresenting: (callback) => {
    const listener = (_event, presenting) => callback(presenting === true);
    ipcRenderer.on("mascot-presenting", listener);
    return () => ipcRenderer.removeListener("mascot-presenting", listener);
  },
  rerun: (taskId) => ipcRenderer.invoke("mascot-rerun", taskId),
  openInbox: () => ipcRenderer.invoke("mascot-inbox"),
  focus: () => ipcRenderer.invoke("mascot-focus"),
  land: (taskId) => ipcRenderer.invoke("mascot-land", taskId),
  restart: () => ipcRenderer.invoke("mascot-restart"),
  resize: (height) => {
    if (typeof height === "number") ipcRenderer.send("mascot-resize", height);
  },
  dismiss: (id) => ipcRenderer.invoke("mascot-dismiss", id),
});
