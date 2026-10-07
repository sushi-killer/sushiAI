import type { Terminal } from "@xterm/xterm";

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

/** Line-editing shortcuts and copy for a terminal; selection, scrolling and
 * wrapping are xterm's own. Returns the function that removes them. */
export function installTerminalInteractions(
  terminal: Terminal,
  element: HTMLElement,
  send: (data: string) => void,
) {
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
        void navigator.clipboard
          .writeText(terminal.getSelection())
          .catch(() => document.execCommand("copy"));
      }
      return false;
    }
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
    event.clipboardData.setData("text/plain", text);
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  document.addEventListener("copy", copy, true);
  return () => document.removeEventListener("copy", copy, true);
}
