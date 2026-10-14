import React from "react";
import { createRoot } from "react-dom/client";
import { Terminal } from "@xterm/xterm";
import { TerminalPanel } from "../src/TerminalPanel";
import "../src/styles.css";

// Page side of scripts/terminal-perf.mjs: the real TerminalPanel over a fake
// bridge. Node pushes daemon events in through perfHarness.deliver and answers
// attach and ack through the exposed perfAttach / perfAck bindings.
const PANEL = "terminal-perf";
// xterm 6 has no .xterm-scroll-area: the buffer length comes from the instance
// the panel opens, with the scroll area as the fallback of older xterm.
let opened: Terminal | undefined;
const open = Terminal.prototype.open;
Terminal.prototype.open = function (this: Terminal, ...args) {
  opened = this;
  return open.apply(this, args);
};
const win = window as any;
let output: (event: any) => void = () => {};

win.bridge = {
  onDaemonState: () => () => {},
  onDaemonTerminal: (callback: typeof output) => {
    output = callback;
    return () => {};
  },
  daemonTerminalAttach: (input: unknown) => win.perfAttach(input),
  daemonTerminalResize: async () => {},
  daemonTerminalAck: (panelId: string, bytes: number) =>
    win.perfAck(panelId, bytes),
  daemonTerminalDetach: async () => {},
  daemonTerminalWrite: async () => {},
  pathForFile: () => "",
  daemonTerminalAttachData: async () => {},
  agentOpenExternal: async () => {},
} as any;

document.body.innerHTML =
  '<div id="test-root" style="width:800px;height:500px;display:flex"></div>';
createRoot(document.getElementById("test-root")!).render(
  <TerminalPanel
    panel={{
      id: PANEL,
      kind: "terminal",
      title: "Terminal",
      agent: "claude",
      sessionId: "session-perf",
    }}
    onStart={() => {}}
    onReopen={() => {}}
  />,
);

const percentile = (sorted: number[], fraction: number) =>
  sorted[
    Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)
  ] ?? 0;

let running = false;
let last = 0;
let intervals: number[] = [];
let longTasks: number[] = [];
let observer: PerformanceObserver | undefined;

function tick(now: number) {
  if (!running) return;
  if (last) intervals.push(now - last);
  last = now;
  requestAnimationFrame(tick);
}

win.perfHarness = {
  panelId: PANEL,
  deliver: (event: any) => output({ panelId: PANEL, ...event }),
  begin() {
    intervals = [];
    longTasks = [];
    last = 0;
    running = true;
    observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) longTasks.push(entry.duration);
    });
    observer.observe({ type: "longtask", buffered: false });
    requestAnimationFrame(tick);
  },
  end() {
    running = false;
    observer?.disconnect();
    const sorted = [...intervals].sort((a, b) => a - b);
    const rows = document.querySelector(".xterm-rows");
    const area = document.querySelector(".xterm-scroll-area") as HTMLElement;
    const row = rows?.firstElementChild as HTMLElement | null;
    const rowHeight = row?.getBoundingClientRect().height || 0;
    return {
      frameP50Ms: percentile(sorted, 0.5),
      frameP95Ms: percentile(sorted, 0.95),
      frameMaxMs: sorted[sorted.length - 1] ?? 0,
      blockedMs: longTasks.reduce((sum, d) => sum + Math.max(0, d - 50), 0),
      longTaskMaxMs: Math.max(0, ...longTasks),
      domNodes: document.querySelectorAll("*").length,
      bufferLines:
        opened?.buffer.active.length ??
        (rowHeight && area
          ? Math.round(area.getBoundingClientRect().height / rowHeight)
          : 0),
    };
  },
  contains: (text: string) =>
    !!document.querySelector(".xterm-rows")?.textContent?.includes(text),
};
