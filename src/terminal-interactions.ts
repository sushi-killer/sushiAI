import type { Terminal } from "@xterm/xterm";
import { cleanTerminalCopy } from "./terminal-copy";
export { cleanTerminalCopy } from "./terminal-copy";

export function terminalShortcut(event: KeyboardEvent) {
  if (event.isComposing || event.ctrlKey) return undefined;
  if (event.metaKey && !event.altKey && !event.shiftKey) {
    if (event.key === "Backspace") return "\x15";
    if (event.key === "ArrowLeft") return "\x01";
    if (event.key === "ArrowRight") return "\x05";
  }
  if (event.altKey && !event.metaKey && !event.shiftKey) {
    if (event.key === "Backspace") return "\x1b\x7f";
    if (event.key === "ArrowLeft") return "\x1bb";
    if (event.key === "ArrowRight") return "\x1bf";
  }
  // Distinguish Shift+Enter from CR; modern agent CLIs recognize CSI-u.
  if (
    event.key === "Enter" &&
    event.shiftKey &&
    !event.metaKey &&
    !event.altKey
  )
    return "\x1b[13;2u";
  return undefined;
}

export function installTerminalInteractions(
  terminal: Terminal,
  element: HTMLElement,
  send: (data: string) => void,
  remoteScroll?: (
    direction: string,
    lines: number,
    position: { column: number; row: number; fast?: boolean },
  ) => void,
  selecting?: (active: boolean, hasSelection: boolean) => void,
) {
  const copyText = () => {
    const selectionPosition = terminal.getSelectionPosition();
    const context = selectionPosition
      ? terminal.buffer.active
          .getLine(selectionPosition.start.y)
          ?.translateToString(true)
      : "";
    return cleanTerminalCopy(terminal.getSelection(), terminal.cols, context);
  };
  let pressed = false;
  const resume = () => {
    if (!pressed && !terminal.hasSelection()) return;
    pressed = false;
    terminal.clearSelection();
    selecting?.(false, false);
  };
  const selectionListener = terminal.onSelectionChange(() => {
    selecting?.(pressed || terminal.hasSelection(), terminal.hasSelection());
  });
  terminal.attachCustomKeyEventHandler((event) => {
    if (
      event.metaKey &&
      !event.altKey &&
      !event.ctrlKey &&
      event.code === "KeyC" &&
      terminal.hasSelection()
    ) {
      if (event.type === "keydown") {
        event.preventDefault();
        event.stopPropagation();
        const text = copyText();
        void navigator.clipboard
          .writeText(text)
          .catch(() => document.execCommand("copy"));
      }
      return false;
    }
    if (
      event.type === "keydown" &&
      (event.key.length === 1 ||
        event.key === "Backspace" ||
        event.key === "Enter")
    )
      resume();
    const sequence = terminalShortcut(event);
    if (sequence === undefined) return true;
    if (event.type === "keydown") {
      event.preventDefault();
      event.stopPropagation();
      send(sequence);
    }
    return false;
  });
  const copy = (event: ClipboardEvent) => {
    if (!element.contains(document.activeElement) || !event.clipboardData)
      return;
    const text = terminal.getSelection();
    if (!text) return;
    event.clipboardData.setData("text/plain", copyText());
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  const position = (event: MouseEvent) => {
    const screen = element
      .querySelector(".xterm-screen")
      ?.getBoundingClientRect();
    if (!screen?.width || !screen.height) return undefined;
    const column = Math.floor(
      ((event.clientX - screen.left) * terminal.cols) / screen.width,
    );
    const row = Math.floor(
      ((event.clientY - screen.top) * terminal.rows) / screen.height,
    );
    return column >= 0 &&
      column < terminal.cols &&
      row >= 0 &&
      row < terminal.rows
      ? { column, row }
      : undefined;
  };
  const mouseDown = (event: MouseEvent) => {
    if (event.button !== 0) return;
    const cell = position(event);
    // Herdr sends rendered cells, not the source terminal's mouse modes.
    // Only intercept the explicit Claude button; ordinary text stays selectable.
    if (
      remoteScroll &&
      cell &&
      !event.shiftKey &&
      !event.altKey &&
      !event.metaKey
    ) {
      const line =
        terminal.buffer.active
          .getLine(terminal.buffer.active.viewportY + cell.row)
          ?.translateToString(true) || "";
      const label = "Jump to bottom (click)";
      const start = line.indexOf(label);
      if (
        start >= 0 &&
        cell.column >= start &&
        cell.column < start + label.length + 2
      ) {
        event.preventDefault();
        event.stopImmediatePropagation();
        // A transcript selection freezes incoming frames. The intercepted click
        // bypasses xterm's normal selection clearing, so release it explicitly
        // before sending the click or Claude's new bottom frame stays hidden.
        resume();
        terminal.focus();
        send(
          `\x1b[<0;${cell.column + 1};${cell.row + 1}M\x1b[<0;${cell.column + 1};${cell.row + 1}m`,
        );
        return;
      }
    }
    pressed = true;
    selecting?.(true, false);
  };
  const mouseUp = () => {
    if (!pressed) return;
    pressed = false;
    selecting?.(terminal.hasSelection(), terminal.hasSelection());
  };
  const wheel = (event: WheelEvent) => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || !event.deltaY)
      return;
    resume();
    // Local PTYs use xterm's original native wheel handling and sensitivity.
    if (!remoteScroll) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const delta =
      event.deltaY *
      (event.deltaMode === 1
        ? 20
        : event.deltaMode === 2
          ? terminal.rows * 20
          : 1);
    // The initial Herdr behavior: one command per wheel event. The server
    // decides whether to scroll host history or send one mouse report to a TUI.
    remoteScroll(
      event.deltaY < 0 ? "up" : "down",
      Math.min(50, Math.max(1, Math.round(Math.abs(delta) / 20))),
      {
        ...(position(event) || { column: 0, row: 0 }),
        ...(event.altKey ? { fast: true } : {}),
      },
    );
  };
  document.addEventListener("copy", copy, true);
  element.addEventListener("mousedown", mouseDown, true);
  document.addEventListener("mouseup", mouseUp);
  window.addEventListener("blur", mouseUp);
  element.addEventListener("wheel", wheel, { capture: true, passive: false });
  return Object.assign(
    () => {
      selectionListener.dispose();
      document.removeEventListener("copy", copy, true);
      element.removeEventListener("mousedown", mouseDown, true);
      document.removeEventListener("mouseup", mouseUp);
      window.removeEventListener("blur", mouseUp);
      element.removeEventListener("wheel", wheel, true);
    },
    { resume },
  );
}
