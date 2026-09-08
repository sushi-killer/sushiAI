import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowUpRight, Command, Play } from "lucide-react";
import type { Panel } from "./types";
import { fitTerminal, queueTerminalFit } from "./terminal-sizing";
import { installTerminalInteractions } from "./terminal-interactions";
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
  requestFit?: () => void;
  selectionPaused?: boolean;
  resumeSelection?: () => void;
  transfer?: string;
  disposeInteractions?: () => void;
  cols: number;
  rows: number;
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
    runtime.disposeInteractions?.();
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
  const attachmentsAllowed = useRef(false);
  attachmentsAllowed.current = !!panel.agent;
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [transfer, setTransfer] = useState("");
  const [disconnected, setDisconnected] = useState(false);
  const active = panel.kind === "terminal" || panel.started || !!panel.herdrId;
  useEffect(() => {
    if (!active || !host.current) return;
    if (!window.bridge) {
      setError("Open the desktop app to use a real terminal.");
      return;
    }
    let runtime = cache.get(panel.id);
    let disposed = false,
      resizeTimer = 0;
    if (!runtime) {
      const terminal = new Terminal({
        fontFamily:
          '"MesloLGS NF", "JetBrainsMono Nerd Font", "SFMono-Regular", Menlo, "Sushi Terminal Symbols", monospace',
        fontSize: 12,
        lineHeight: 1.35,
        cursorBlink: true,
        cursorStyle: "bar",
        scrollback: panel.herdrId ? 0 : 10000,
        scrollSensitivity: 1.3,
        fastScrollSensitivity: 5,
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
      element.className = panel.herdrId
        ? "terminal-surface terminal-herdr"
        : "terminal-surface";
      host.current.appendChild(element);
      terminal.open(element);
      fitTerminal(terminal, fit);
      runtime = {
        terminal,
        fit,
        element,
        ready: false,
        error: "",
        exited: false,
        unsubscribe: () => {},
        cols: terminal.cols,
        rows: terminal.rows,
      };
      cache.set(panel.id, runtime);
      const entry = runtime;
      const pending: string[] = [];
      let selecting = false;
      let selectedOutput = "";
      entry.unsubscribe = window.bridge.onTerminal((event) => {
        if (event.panelId !== panel.id) return;
        if (event.agent !== undefined)
          attachmentsAllowed.current = !!event.agent;
        if (event.data) {
          if (entry.ready && selecting && panel.herdrId) {
            selectedOutput += event.data;
            if (selectedOutput.length > 2 * 1024 * 1024) {
              entry.resumeSelection?.();
            }
          } else if (entry.ready) terminal.write(event.data);
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
      const sendInput = (data: string) => {
        if (entry.ready && !entry.exited)
          window.bridge!.terminalWrite(panel.id, data).catch((e) => {
            entry.error = e.message;
            entry.notify?.();
          });
      };
      terminal.onData(sendInput);
      const interactions = installTerminalInteractions(
        terminal,
        element,
        sendInput,
        panel.herdrId
          ? (direction, lines, position) => {
              window
                .bridge!.terminalScroll(panel.id, direction, lines, position)
                .catch((e) => {
                  entry.error = e.message;
                  entry.notify?.();
                });
            }
          : undefined,
        (active) => {
          if (selecting === active) return;
          selecting = active;
          entry.selectionPaused = !!panel.herdrId && active;
          entry.notify?.();
          if (!active) entry.requestFit?.();
          if (!active && selectedOutput) {
            terminal.write(selectedOutput);
            selectedOutput = "";
          }
        },
      );
      entry.disposeInteractions = interactions;
      entry.resumeSelection = interactions.resume;
      // Paste or drop a screenshot and the terminal receives its path, which is
      // what Claude Code and the other agent CLIs read an image from.
      let uploads = Promise.resolve();
      const attach = (files: File[]) => {
        if (!attachmentsAllowed.current) return;
        uploads = uploads.then(async () => {
          if (cache.get(panel.id) !== entry) return;
          try {
            if (!attachmentsAllowed.current) return;
            if (!entry.ready || entry.exited)
              throw new Error("This terminal is not running.");
            if (files.length > 8)
              throw new Error("Attach up to 8 files at a time.");
            if (
              files.some((file) => !file.size || file.size > 20 * 1024 * 1024)
            )
              throw new Error("Choose non-empty files up to 20 MB each.");
            entry.error = "";
            for (const file of files) {
              entry.transfer = `Attaching ${file.name || "image"}…`;
              entry.notify?.();
              const stored = await window.bridge!.terminalAttach({
                panelId: panel.id,
                name: file.name || "pasted.png",
                data: base64(new Uint8Array(await file.arrayBuffer())),
              });
              if (cache.get(panel.id) !== entry || entry.exited) return;
              terminal.paste(shellPath(stored));
            }
            if (element.isConnected) terminal.focus();
          } catch (e) {
            entry.error = e instanceof Error ? e.message : String(e);
          } finally {
            entry.transfer = "";
            entry.notify?.();
          }
        });
      };
      element.addEventListener("dragover", (event) => {
        if (!event.dataTransfer?.types.includes("Files")) return;
        event.preventDefault();
        event.stopPropagation();
        if (!attachmentsAllowed.current) {
          event.dataTransfer.dropEffect = "none";
          return;
        }
        element.classList.add("file-drop-active");
        event.dataTransfer.dropEffect = "copy";
      });
      element.addEventListener("dragleave", (event) => {
        if (!element.contains(event.relatedTarget as Node | null))
          element.classList.remove("file-drop-active");
      });
      element.addEventListener("drop", (event) => {
        element.classList.remove("file-drop-active");
        if (!event.dataTransfer?.types.includes("Files")) return;
        event.preventDefault();
        event.stopPropagation();
        const files = [...event.dataTransfer.files];
        if (files.length) attach(files);
      });
      element.addEventListener(
        "paste",
        (event) => {
          const files = [...(event.clipboardData?.files || [])];
          if (!files.length) return;
          event.preventDefault();
          event.stopImmediatePropagation();
          attach(files);
        },
        true,
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
          entry.exited = entry.exited || !!result.exited;
          if (entry.exited)
            entry.error = "Session ended. Reconnect to continue.";
          pending.length = 0;
          entry.requestFit?.();
          entry.notify?.();
        })
        .catch((e) => {
          entry.error = e.message;
          entry.notify?.();
        });
    } else host.current.appendChild(runtime.element);
    const current = runtime;
    current.notify = () => {
      if (!disposed) {
        setError(current.error);
        setTransfer(current.transfer || "");
        setDisconnected(current.exited || !current.ready);
      }
    };
    current.notify();
    const visible = () =>
      !disposed &&
      current.element.isConnected &&
      !!host.current?.clientWidth &&
      !!host.current.clientHeight;
    const fit = () => {
      if (!visible() || !current.ready || current.selectionPaused) return;
      queueTerminalFit(
        current.terminal,
        current.fit,
        () => visible() && !current.selectionPaused,
        () => {
          if (
            current.ready &&
            !current.exited &&
            (current.terminal.cols !== current.cols ||
              current.terminal.rows !== current.rows)
          ) {
            const { cols, rows } = current.terminal;
            current.cols = cols;
            current.rows = rows;
            window.bridge!.terminalResize(panel.id, cols, rows).catch((e) => {
              current.error = e.message;
              current.notify?.();
            });
          }
        },
      );
    };
    // Resizing on every drag frame makes cursor-based TUIs redraw repeatedly
    // against intermediate widths. Apply the settled layout once instead.
    const scheduleFit = () => {
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(fit, 100);
    };
    current.requestFit = scheduleFit;
    const observer = new ResizeObserver(scheduleFit);
    observer.observe(host.current);
    fit();
    void document.fonts
      .load('12px "Sushi Terminal Symbols"', "󰂺")
      .then(() => {
        if (!disposed) {
          current.terminal.refresh(0, current.terminal.rows - 1);
          scheduleFit();
        }
      })
      .catch(() => {});
    void document.fonts.ready.then(() => {
      if (!disposed) scheduleFit();
    });
    return () => {
      disposed = true;
      observer.disconnect();
      window.clearTimeout(resizeTimer);
      current.notify = undefined;
      current.requestFit = undefined;
      current.element.remove();
    };
  }, [panel.id, active, cwd, socket, endpoint, attempt]);
  const currentErrorDismiss = () => {
    const runtime = cache.get(panel.id);
    if (runtime) runtime.error = "";
    setError("");
  };
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
      {transfer && (
        <div className="terminal-transfer" role="status">
          {transfer}
        </div>
      )}
      {error && (
        <div className="panel-error" role="alert">
          {error}
          <button
            className="terminal-reconnect"
            onClick={async () => {
              if (!disconnected) {
                currentErrorDismiss();
                return;
              }
              try {
                await window.bridge?.terminalClose(panel.id);
                disposeTerminal(panel.id);
                setError("");
                setAttempt((a) => a + 1);
              } catch (e) {
                setError(e instanceof Error ? e.message : String(e));
              }
            }}
          >
            {disconnected ? "Reconnect" : "Dismiss"}
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
