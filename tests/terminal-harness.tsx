import React from "react";
import { createRoot } from "react-dom/client";
import { TerminalPanel, disposeTerminal } from "../src/TerminalPanel";
import "../src/styles.css";

const calls = {
  writes: [] as string[],
  files: [] as string[],
  closed: 0,
  drops: 0,
};
let output: (event: any) => void;
let failAttachment = false;
window.bridge = {
  onTerminal: (callback: typeof output) => {
    output = callback;
    return () => {};
  },
  terminalOpen: async () => ({ history: "Ready for images\r\n\x1b[?2004h" }),
  terminalResize: async () => {},
  terminalWrite: async (_id: string, data: string) => {
    calls.writes.push(data);
  },
  terminalAttach: async ({ name }: { name: string }) => {
    calls.files.push(name);
    await new Promise((resolve) => setTimeout(resolve, 20));
    if (failAttachment) throw Error("Test attachment failed");
    return `/tmp/attachments/${name}`;
  },
  terminalClose: async () => {
    calls.closed++;
  },
} as any;
document.body.innerHTML =
  '<div id="test-root" style="width:800px;height:500px;display:flex"></div>';
document.body.addEventListener("drop", () => calls.drops++);
const root = createRoot(document.getElementById("test-root")!);
root.render(
  <TerminalPanel
    panel={{
      id: "terminal-test",
      kind: "terminal",
      title: "Terminal",
      agent: "claude",
      herdrId: "test-herdr",
    }}
    cwd="/tmp"
    socket=""
    onStart={() => {}}
  />,
);
(window as any).terminalHarness = {
  calls,
  shell: () => output({ panelId: "terminal-test", data: "", agent: null }),
  fail: () => {
    failAttachment = true;
  },
  output: (data: string) => output({ panelId: "terminal-test", data }),
  dispose: () => {
    root.unmount();
    disposeTerminal("terminal-test");
  },
};
