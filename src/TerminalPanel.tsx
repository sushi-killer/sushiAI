import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { ArrowUpRight, Command, Play, RotateCcw } from "lucide-react";
import { agentTitle } from "./app/agent-title";
import { daemonHost } from "./daemonSessions";
import type { Panel } from "./types";
import {
  fitTerminal,
  queueTerminalFit,
  rememberTerminalSize,
  terminalDimensions,
} from "./terminal-sizing";
import { installTerminalInteractions } from "./terminal-interactions";
import { createTerminalLinkProvider, openTerminalLink } from "./terminal-links";
import { createTerminalInput, shouldReattach } from "./terminal-output";
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
  start?: () => void;
  transfer?: string;
  disposeInteractions?: () => void;
  cols: number;
  rows: number;
};
const cache = new Map<string, CachedTerminal>();
export function disposeTerminal(id: string) {
  const runtime = cache.get(id);
  if (runtime) {
    runtime.unsubscribe();
    void window.bridge?.daemonTerminalDetach(id);
    runtime.disposeInteractions?.();
    runtime.terminal.dispose();
    cache.delete(id);
  }
}
export function TerminalPanel({
  panel,
  endpoint,
  hostLabel,
  onStart,
  onReopen,
}: {
  panel: Panel;
  endpoint?: string;
  /** Pane provenance: set only when this pane's workspace is a member of a
   * merged sidebar row (flat mode). */
  hostLabel?: string;
  onStart(): void;
  /** Reopens an ended pane. */
  onReopen(): void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const attachmentsAllowed = useRef(false);
  attachmentsAllowed.current = !!panel.agent;
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [transfer, setTransfer] = useState("");
  const [disconnected, setDisconnected] = useState(false);
  // A process only runs in a daemon session; a panel without one waits for
  // Launch.
  const active = !panel.ended && !!panel.sessionId;
  // An ended panel keeps no attach in main.
  useEffect(() => {
    if (panel.ended) disposeTerminal(panel.id);
  }, [panel.ended, panel.id]);
  // A host that comes (back) up after the first attach failed, or after its
  // disconnect ended the pane's stream, gets another try.
  useEffect(() => {
    if (!panel.sessionId || !window.bridge?.onDaemonState) return;
    const hostId = daemonHost(endpoint);
    return window.bridge.onDaemonState((state) => {
      if (state.host !== hostId || state.state !== "ready") return;
      const runtime = cache.get(panel.id);
      if (runtime && shouldReattach(runtime, panel.ended)) {
        disposeTerminal(panel.id);
        setAttempt((a) => a + 1);
      }
    });
  }, [panel.id, panel.sessionId, panel.ended, endpoint]);
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
        linkHandler: { activate: openTerminalLink },
        cursorStyle: "bar",
        scrollback: 10000,
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
      element.className = "terminal-surface";
      host.current.appendChild(element);
      terminal.open(element);
      const links = terminal.registerLinkProvider(
        createTerminalLinkProvider(terminal),
      );
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
      const sessionId = panel.sessionId!;
      const exitedMessage = "Session ended. Reconnect to continue.";
      const bytesOf = (text: string) => new TextEncoder().encode(text).length;
      const acked = (text: string) => () => {
        window
          .bridge!.daemonTerminalAck(panel.id, bytesOf(text))
          .catch(() => {});
      };
      entry.unsubscribe = window.bridge.onDaemonTerminal((event) => {
        if (event.panelId !== panel.id) return;
        if (event.snapshot !== undefined) {
          terminal.reset();
          terminal.write(event.snapshot, acked(event.snapshot));
        }
        if (event.data) terminal.write(event.data, acked(event.data));
        if (event.exited) {
          entry.exited = true;
          entry.error = exitedMessage;
          entry.notify?.();
        }
      });
      const input = createTerminalInput(
        (data) => window.bridge!.daemonTerminalWrite(panel.id, data),
        (message) => {
          entry.error = message;
          entry.notify?.();
        },
      );
      const sendInput = (data: string) => {
        if (entry.ready && !entry.exited) input.send(data);
      };
      terminal.onData(sendInput);
      const interactions = installTerminalInteractions(
        terminal,
        element,
        sendInput,
      );
      entry.disposeInteractions = () => {
        input.close();
        links.dispose();
        interactions();
      };
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
              // The daemon writes the path on its host. A file with a path
              // on disk is handed over by path; pasted data goes to main.
              const path = window.bridge!.pathForFile(file);
              if (path)
                await window.bridge!.daemonTerminalAttachFile(panel.id, path);
              else
                await window.bridge!.daemonTerminalAttachData(
                  panel.id,
                  file.name || "pasted.png",
                  new Uint8Array(await file.arrayBuffer()),
                );
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
      // The attach carries the measured size: attaching at xterm's 80x24
      // default and resizing afterwards sends the shell two SIGWINCHes and
      // makes it redraw its prompt at the wrong width.
      let started = false;
      entry.start = () => {
        const size = terminalDimensions(fit.proposeDimensions());
        if (started || !size) return;
        started = true;
        terminal.resize(size.cols, size.rows);
        rememberTerminalSize(size);
        entry.cols = size.cols;
        entry.rows = size.rows;
        window
          .bridge!.daemonTerminalAttach({
            panelId: panel.id,
            host: daemonHost(endpoint),
            sessionId,
            cols: size.cols,
            rows: size.rows,
          })
          .then(() => {
            if (cache.get(panel.id) !== entry) return;
            entry.ready = true;
            entry.requestFit?.();
            entry.notify?.();
          })
          .catch((e) => {
            entry.error = e.message;
            entry.notify?.();
          });
      };
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
      if (!visible()) return;
      if (!current.ready) {
        current.start?.();
        return;
      }
      queueTerminalFit(
        current.terminal,
        current.fit,
        () => visible(),
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
            window
              .bridge!.daemonTerminalResize(panel.id, cols, rows)
              .catch((e) => {
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
  }, [panel.id, active, endpoint, attempt]);
  const currentErrorDismiss = () => {
    const runtime = cache.get(panel.id);
    if (runtime) runtime.error = "";
    setError("");
  };
  if (panel.ended)
    return (
      <div className="terminal-ended" role="status">
        <p>Session ended</p>
        <small>
          {panel.agent ? agentTitle(panel.agent) : "This terminal"} is no longer
          running on its host.
        </small>
        <button className="primary" onClick={onReopen}>
          <RotateCcw size={13} /> Reopen
        </button>
      </div>
    );
  if (!active)
    return (
      <div className="agent-intro">
        <div className="terminal-command">
          <span>❯</span>{" "}
          {panel.kind === "terminal" ? "zsh" : panel.agent || "claude"}
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
          <div>{error}</div>
          <button
            className="terminal-reconnect"
            onClick={() => {
              if (!disconnected) {
                currentErrorDismiss();
                return;
              }
              disposeTerminal(panel.id);
              setError("");
              setAttempt((a) => a + 1);
            }}
          >
            {disconnected ? "Reconnect" : "Dismiss"}
          </button>
        </div>
      )}
      {hostLabel && (
        <div className="pane-host-cluster">
          <span
            className="terminal-host-note"
            title={`This pane belongs to ${hostLabel}.`}
          >
            {hostLabel}
          </span>
        </div>
      )}
    </div>
  );
}
