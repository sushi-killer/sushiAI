import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";

// Match terminal-open/terminal-resize in the main process. FitAddon alone
// permits 2x1 cells, which the PTY bridge rejects, leaving two different grids.
export function terminalDimensions(size?: { cols: number; rows: number }) {
  if (!size || !Number.isFinite(size.cols) || !Number.isFinite(size.rows))
    return undefined;
  return {
    cols: Math.max(10, Math.min(500, Math.floor(size.cols))),
    rows: Math.max(3, Math.min(300, Math.floor(size.rows))),
  };
}

export function fitTerminal(terminal: Terminal, fit: FitAddon) {
  const size = terminalDimensions(fit.proposeDimensions());
  if (size) terminal.resize(size.cols, size.rows);
}

// xterm.write is asynchronous, while resize is synchronous. Drain output
// already received at the old size before changing the grid and notifying PTY.
export function queueTerminalFit(
  terminal: Terminal,
  fit: FitAddon,
  visible: () => boolean,
  resized: () => void,
) {
  terminal.write("", () => {
    if (!visible()) return;
    fitTerminal(terminal, fit);
    resized();
  });
}
