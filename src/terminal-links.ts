import type {
  IBufferRange,
  ILink,
  ILinkProvider,
  Terminal,
} from "@xterm/xterm";

export type TerminalLinkPosition = { row: number; column: number };
export type TerminalLinkRange = {
  url: string;
  start: TerminalLinkPosition;
  /** Position of the last character of the URL (inclusive). */
  end: TerminalLinkPosition;
};

const CANDIDATE = /https?:\/\/[^\s<>"`]+/gi;
const TRAILING = ".,;:!?'\"";
const PAIRS: Record<string, string> = { ")": "(", "]": "[" };

const count = (text: string, char: string) => text.split(char).length - 1;

const isWebUrl = (value: string) => {
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
};

const trimCandidate = (candidate: string) => {
  let text = candidate;
  for (;;) {
    const last = text[text.length - 1];
    if (last && TRAILING.includes(last)) text = text.slice(0, -1);
    else if (
      last &&
      PAIRS[last] &&
      count(text, last) > count(text, PAIRS[last])
    )
      text = text.slice(0, -1);
    else return text;
  }
};

/** `lines` are the buffer rows of one logical (wrapped) line, in order. */
export function findTerminalLinks(lines: string[]): TerminalLinkRange[] {
  const starts: number[] = [];
  let total = 0;
  for (const line of lines) {
    starts.push(total);
    total += line.length;
  }
  const at = (offset: number): TerminalLinkPosition => {
    let row = starts.length - 1;
    while (row > 0 && starts[row] > offset) row--;
    return { row, column: offset - starts[row] };
  };
  const found: TerminalLinkRange[] = [];
  for (const match of lines.join("").matchAll(CANDIDATE)) {
    const url = trimCandidate(match[0]);
    if (!isWebUrl(url)) continue;
    found.push({
      url,
      start: at(match.index),
      end: at(match.index + url.length - 1),
    });
  }
  return found;
}

const isMac = () =>
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent || "");

/** Opens a http(s) link through the desktop bridge, only with Cmd/Ctrl held. */
export function openTerminalLink(event: MouseEvent, uri: string) {
  if (!(isMac() ? event.metaKey : event.ctrlKey)) return;
  if (!isWebUrl(uri)) return;
  window.bridge?.agentOpenExternal(uri).catch(() => {});
}

export function createTerminalLinkProvider(terminal: Terminal): ILinkProvider {
  return {
    provideLinks(y, callback) {
      const buffer = terminal.buffer.active;
      const target = buffer.getLine(y - 1);
      if (!target) return callback(undefined);
      let first = y - 1;
      while (first > 0 && buffer.getLine(first)?.isWrapped) first--;
      const lines: string[] = [];
      for (let i = first; ; i++) {
        const line = buffer.getLine(i);
        if (!line || (i > first && !line.isWrapped)) break;
        lines.push(line.translateToString(false));
      }
      const links: ILink[] = findTerminalLinks(lines)
        .filter(
          (found) =>
            first + found.start.row <= y - 1 && y - 1 <= first + found.end.row,
        )
        .map((found) => {
          const range: IBufferRange = {
            start: {
              x: found.start.column + 1,
              y: first + found.start.row + 1,
            },
            end: { x: found.end.column + 1, y: first + found.end.row + 1 },
          };
          return { range, text: found.url, activate: openTerminalLink };
        });
      callback(links.length ? links : undefined);
    },
  };
}
