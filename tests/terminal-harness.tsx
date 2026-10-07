import React from "react";
import { createRoot } from "react-dom/client";
import { TerminalPanel, disposeTerminal } from "../src/TerminalPanel";
import "../src/styles.css";

const calls = {
  writes: [] as string[],
  files: [] as string[],
  closed: 0,
  drops: 0,
  opened: [] as string[],
  attaches: [] as { cols: number; rows: number }[],
};
let output: (event: any) => void;
let failAttachment = false;
window.bridge = {
  onDaemonState: () => () => {},
  onDaemonTerminal: (callback: typeof output) => {
    output = callback;
    return () => {};
  },
  daemonTerminalAttach: async (input: { cols: number; rows: number }) => {
    calls.attaches.push({ cols: input.cols, rows: input.rows });
    // The daemon answers an attach with its screen as a snapshot.
    setTimeout(
      () =>
        output({
          panelId: "terminal-test",
          snapshot: "Ready for images\r\n\x1b[?2004h",
        }),
      0,
    );
  },
  daemonTerminalResize: async () => {},
  daemonTerminalAck: async () => {},
  daemonTerminalDetach: async () => {
    calls.closed++;
  },
  daemonTerminalWrite: async (_id: string, data: string) => {
    calls.writes.push(data);
  },
  pathForFile: () => "",
  daemonTerminalAttachData: async (_id: string, name: string) => {
    calls.files.push(name);
    await new Promise((resolve) => setTimeout(resolve, 20));
    if (failAttachment) throw Error("Test attachment failed");
  },
  agentOpenExternal: async (url: string) => {
    calls.opened.push(url);
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
      sessionId: "session-test",
    }}
    onStart={() => {}}
    onReopen={() => {}}
  />,
);
(window as any).terminalHarness = {
  calls,
  output: (data: string) => output({ panelId: "terminal-test", data }),
  fail: () => {
    failAttachment = true;
  },
  dispose: () => {
    root.unmount();
    disposeTerminal("terminal-test");
  },
};
