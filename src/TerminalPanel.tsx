import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowUpRight, Command, Play } from "lucide-react";
import type { Panel } from "./types";
import "@xterm/xterm/css/xterm.css";

type CachedTerminal = {
  terminal: Terminal;
  fit: FitAddon;
  element: HTMLDivElement;
  ready: boolean;
  error: string;
  exited: boolean;
  unsubscribe: () => void;
  notify?: () => void;
};
const cache = new Map<string, CachedTerminal>();
const base64 = (bytes: Uint8Array) => {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000)
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
};
// A shell reads the path as one word however the file was named.
const shellPath = (value: string) => `'${value.replace(/'/g, "'\\''")}' `;
export function disposeTerminal(id: string) {
  const runtime = cache.get(id);
  if (runtime) {
    runtime.unsubscribe();
    runtime.terminal.dispose();
    cache.delete(id);
  }
}
export function TerminalPanel({
  panel,
  cwd,
  socket,
  endpoint,
  onStart,
}: {
  panel: Panel;
  cwd: string;
  socket: string;
  endpoint?: string;
  onStart(): void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const active = panel.kind === "terminal" || panel.started || !!panel.herdrId;
  useEffect(() => {
    if (!active || !host.current) return;
    if (!window.bridge) {
      setError("Open the desktop app to use a real terminal.");
      return;
    }
    let runtime = cache.get(panel.id);
    let disposed = false,
      resizeFrame = 0;
    if (!runtime) {
      const terminal = new Terminal({
        fontFamily:
          '"MesloLGS NF", "JetBrainsMono Nerd Font", "SFMono-Regular", Menlo, "Symbols Nerd Font Mono", monospace',
        fontSize: 12,
        lineHeight: 1.35,
        cursorBlink: true,
        cursorStyle: "bar",
        scrollback: 10000,
        theme: {
          background: "#0b0b0b",
          foreground: "#d5d5d5",
          cursor: "#799dff",
          selectionBackground: "#344262",
          red: "#e87578",
          green: "#80d9a4",
          yellow: "#e4ca79",
          blue: "#7da6fa",
          magenta: "#c39ae6",
          cyan: "#82cbd0",
          brightBlack: "#808080",
        },
      });
      const fit = new FitAddon();
      terminal.loadAddon(fit);
      const element = document.createElement("div");
      element.className = "terminal-surface";
      host.current.appendChild(element);
      terminal.open(element);
      fit.fit();
      runtime = {
        terminal,
        fit,
        element,
        ready: false,
        error: "",
        exited: false,
        unsubscribe: () => {},
      };
      cache.set(panel.id, runtime);
      const entry = runtime;
      const pending: string[] = [];
      entry.unsubscribe = window.bridge.onTerminal((event) => {
        if (event.panelId !== panel.id) return;
        if (event.data) {
          if (entry.ready) terminal.write(event.data);
          else pending.push(event.data);
        }
        if (event.exitCode !== undefined) {
          entry.exited = true;
          entry.error = panel.herdrId
            ? "Stream disconnected. Reconnect to resume. If another app owns this terminal, detach it first."
            : "Process exited. Reconnect to start a new shell.";
          entry.notify?.();
        }
      });
      terminal.onData((data) => {
        if (entry.ready && !entry.exited)
          window.bridge!.terminalWrite(panel.id, data).catch((e) => {
            entry.error = e.message;
            entry.notify?.();
          });
      });
      // Paste or drop a screenshot and the terminal receives its path, which is
      // what Claude Code and the other agent CLIs read an image from.
      const attach = async (files: File[]) => {
        for (const file of files.slice(0, 8))
          try {
            const stored = await window.bridge!.terminalAttach({
              panelId: panel.id,
              name: file.name || "pasted.png",
              data: base64(new Uint8Array(await file.arrayBuffer())),
            });
            await window.bridge!.terminalWrite(panel.id, shellPath(stored));
          } catch (e) {
            entry.error = e instanceof Error ? e.message : String(e);
            entry.notify?.();
            return;
          }
      };
      element.addEventListener("dragover", (event) => {
        if (!event.dataTransfer?.types.includes("Files")) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      });
      element.addEventListener("drop", (event) => {
        const files = [...(event.dataTransfer?.files || [])];
        if (!files.length) return;
        event.preventDefault();
        void attach(files);
      });
      element.addEventListener(
        "paste",
        (event) => {
          const files = [...(event.clipboardData?.files || [])];
          if (!files.length) return;
          event.preventDefault();
          event.stopPropagation();
          void attach(files);
        },
        true,
      );
      if (panel.herdrId)
        element.addEventListener(
          "wheel",
          (event) => {
            if (event.ctrlKey || event.metaKey || event.deltaY === 0) return;
            event.preventDefault();
            window
              .bridge!.terminalScroll(
                panel.id,
                event.deltaY < 0 ? "up" : "down",
                Math.min(
                  50,
                  Math.max(1, Math.round(Math.abs(event.deltaY) / 20)),
                ),
              )
              .catch(() => {});
          },
          { passive: false },
        );
      window.bridge
        .terminalOpen({
          panelId: panel.id,
          cwd,
          endpoint: panel.herdrId ? socket : endpoint,
          herdrId: panel.herdrId,
          command: panel.kind === "agent" ? panel.agent || "claude" : undefined,
          cols: terminal.cols,
          rows: terminal.rows,
        })
        .then((result) => {
          if (cache.get(panel.id) !== entry) return;
          terminal.write(result.history);
          pending.forEach((data) => terminal.write(data));
          entry.ready = true;
          entry.exited = !!result.exited;
          if (entry.exited)
            entry.error = "Session ended. Reconnect to continue.";
          if (element.isConnected) {
            fit.fit();
            window
              .bridge!.terminalResize(panel.id, terminal.cols, terminal.rows)
              .catch(() => {});
          }
          entry.notify?.();
        })
        .catch((e) => {
          entry.error = e.message;
          entry.notify?.();
        });
    } else host.current.appendChild(runtime.element);
    const current = runtime;
    current.notify = () => {
      if (!disposed) setError(current.error);
    };
    current.notify();
    let previousCols = 0,
      previousRows = 0;
    const fit = () => {
      if (disposed || !host.current?.clientWidth || !host.current.clientHeight)
        return;
      current.fit.fit();
      if (
        current.ready &&
        !current.exited &&
        (current.terminal.cols !== previousCols ||
          current.terminal.rows !== previousRows)
      ) {
        previousCols = current.terminal.cols;
        previousRows = current.terminal.rows;
        window
          .bridge!.terminalResize(panel.id, previousCols, previousRows)
          .catch((e) => {
            current.error = e.message;
            current.notify?.();
          });
      }
    };
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(fit);
    });
    observer.observe(host.current);
    fit();
    return () => {
      disposed = true;
      observer.disconnect();
      cancelAnimationFrame(resizeFrame);
      current.notify = undefined;
      current.element.remove();
    };
  }, [panel.id, active, cwd, socket, endpoint, attempt]);
  if (!active)
    return (
      <div className="agent-intro">
        <div className="terminal-command">
          <span>❯</span> {panel.agent || "claude"}
          <span className="idle-label">ready to launch</span>
        </div>
        <div className="agent-welcome">
          <span className="claude-mark">✳</span>
          <h2>Your next idea starts here.</h2>
          <p>
            Give an agent a little room.
            <br />
            Build something that matters.
          </p>
          <button className="primary" onClick={onStart}>
            <Play size={13} /> Launch {panel.title}
            <ArrowUpRight size={14} />
          </button>
          <small>Uses your locally installed CLI and account</small>
        </div>
        <div className="agent-hints">
          <div>
            <Command size={13} />
            <span>⌘ K</span>
            <p>Add a terminal, browser, or chat</p>
          </div>
          <div>
            <span className="hint-drag">⠿</span>
            <span>Drag</span>
            <p>Arrange panels your way</p>
          </div>
        </div>
        <div className="terminal-prompt">
          <span>❯</span>
          <i />
        </div>
      </div>
    );
  return (
    <div className="terminal-wrap">
      <div ref={host} className="terminal-host" />
      {error && (
        <div className="panel-error" role="alert">
          {error}
          <button
            className="terminal-reconnect"
            onClick={async () => {
              await window.bridge?.terminalClose(panel.id);
              disposeTerminal(panel.id);
              setError("");
              setAttempt((a) => a + 1);
            }}
          >
            Reconnect
          </button>
        </div>
      )}
      {panel.herdrId && (
        <span
          className="herdr-terminal-note"
          title="Direct Herdr stream: raw input, incremental frames and native terminal resizing."
        >
          Herdr · live stream
        </span>
      )}
    </div>
  );
}
