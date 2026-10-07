// Reads the visible screen of a blocked agent CLI (Claude Code, Codex) into
// the question it asks and the choices it offers, plus the raw input that
// picks each one. Pure: the screens it knows are captured from real sessions
// (tests/session-prompt.test.cjs), not written by hand.

export type PromptOption = {
  label: string;
  /** The lines under the label: a description or the label's wrapped tail. */
  hint: string;
  /** Raw terminal input, sent in order, that picks this option. */
  steps: string[];
  /** Set on a multi-select menu: whether the box is ticked on screen. */
  checked?: boolean;
};

export type SessionPrompt = {
  question: string;
  /** What the question is about: the tool, command or diff Claude Code
   * frames above it. Empty when the screen has no such frame. */
  detail: string[];
  options: PromptOption[];
  /** Options toggle instead of answering; `advance` moves past the question. */
  multi: boolean;
  advance: string[];
  /** The input that opens a free-text answer before the text itself: empty
   * when the screen takes text directly, `null` when typing would land in a
   * menu and pick something by accident. */
  typeSteps: string[] | null;
};

const ENTER = "\r";
const UP = "\x1b[A";
const DOWN = "\x1b[B";
const RIGHT = "\x1b[C";
const CURSOR = /^(\s*)([❯›])\s+(\S.*)$/;
const NUMBERED = /^(\s*)(?:([❯›])\s*)?(\d{1,2})\.\s+(\S.*)$/;
const CHECKBOX = /^\[([^\]])\]\s*/;
const RULE = /^\s*[─━]{8,}\s*$/;
const DASHED = /^\s*[╌┄]{8,}\s*$/;
const TYPE_SOMETHING = /^type something\.?$/i;
/** Menu lines never sit further than this below the last option. */
const TAIL = 6;

const blank = (line: string) => line.trim() === "";
const indent = (line: string) => line.length - line.trimStart().length;

/** Arrow keys from the highlighted option to `target`, then Enter: how any
 * select menu is answered without trusting digit shortcuts. */
function navigate(cursor: number, target: number): string {
  const delta = target - cursor;
  return (delta > 0 ? DOWN : UP).repeat(Math.abs(delta)) + ENTER;
}

type Menu = {
  first: number;
  cursor: number;
  numbered: boolean;
  entries: { label: string; hint: string; checked?: boolean }[];
};

/** The last numbered menu on screen (`1.`, `2.`, ... with one highlighted),
 * or null. Options may be separated by descriptions, wrapped text or a rule;
 * any other numbering in between means it is not a menu. */
function numberedMenu(lines: string[]): Menu | null {
  let last = -1;
  for (let i = lines.length - 1, seen = 0; i >= 0 && seen <= TAIL; i--) {
    if (NUMBERED.test(lines[i])) {
      last = i;
      break;
    }
    if (!blank(lines[i])) seen++;
  }
  if (last < 0) return null;
  const rows: number[] = [last];
  let expect = Number(NUMBERED.exec(lines[last])![3]) - 1;
  for (let i = last - 1; i >= 0 && expect >= 1 && last - i < 40; i--) {
    const match = NUMBERED.exec(lines[i]);
    if (!match) {
      if (rows[0] - i > 4) return null;
      continue;
    }
    if (Number(match[3]) !== expect) return null;
    rows.unshift(i);
    expect--;
  }
  if (expect !== 0 || rows.length < 2) return null;
  let cursor = -1;
  const entries = rows.map((row, index) => {
    const [, lead, mark, , rest] = NUMBERED.exec(lines[row])!;
    if (mark) cursor = index;
    const hint: string[] = [];
    const end = rows[index + 1] ?? lines.length;
    for (let i = row + 1; i < end; i++) {
      const line = lines[i];
      if (blank(line) || RULE.test(line) || indent(line) <= lead.length) break;
      if (line.trim() !== "Submit") hint.push(line.trim());
    }
    const box = CHECKBOX.exec(rest);
    return {
      label: box ? rest.slice(box[0].length).trim() : rest.trim(),
      hint: hint.join(" "),
      ...(box ? { checked: box[1] !== " " } : {}),
    };
  });
  if (cursor < 0) return null;
  return { first: rows[0], cursor, numbered: true, entries };
}

/** A highlighted line with sibling lines at its column and no numbers:
 * Claude Code's folder-trust menu. */
function plainMenu(lines: string[]): Menu | null {
  for (let i = lines.length - 1, seen = 0; i >= 0 && seen <= TAIL; i--) {
    const match = CURSOR.exec(lines[i]);
    if (!match) {
      if (!blank(lines[i])) seen++;
      continue;
    }
    const column = lines[i].length - match[3].length;
    const sibling = (line: string | undefined) =>
      line !== undefined && !blank(line) && indent(line) === column;
    let first = i;
    while (sibling(lines[first - 1])) first--;
    let end = i + 1;
    while (sibling(lines[end])) end++;
    if (end - first < 2) return null;
    return {
      first,
      cursor: i - first,
      numbered: false,
      entries: lines.slice(first, end).map((line, index) => ({
        label: index === i - first ? match[3].trim() : line.trim(),
        hint: "",
      })),
    };
  }
  return null;
}

/** The question above the menu: the nearest of the few paragraphs there
 * that asks something, else the nearest one. A rule or the echoed prompt
 * ends the search. */
function questionAbove(lines: string[], first: number) {
  const paragraphs: { text: string; top: number }[] = [];
  let i = first - 1;
  while (i >= 0 && paragraphs.length < 3) {
    while (i >= 0 && blank(lines[i])) i--;
    const text: string[] = [];
    for (; i >= 0 && !blank(lines[i]); i--) {
      if (RULE.test(lines[i]) || DASHED.test(lines[i]) || CURSOR.test(lines[i]))
        break;
      text.unshift(lines[i].trim());
    }
    if (text.length) paragraphs.push({ text: text.join(" "), top: i + 1 });
    if (i >= 0 && !blank(lines[i])) break;
  }
  const pick = paragraphs.find((p) => p.text.includes("?")) ?? paragraphs[0];
  return { question: pick?.text ?? "", top: pick?.top ?? first };
}

/** Claude Code frames a permission prompt under a full-width rule: the tool,
 * the command or the diff, then the question. */
function detailAbove(lines: string[], top: number): string[] {
  for (let i = top - 1; i >= 0 && top - i <= 16; i--) {
    if (!RULE.test(lines[i])) continue;
    return lines
      .slice(i + 1, top)
      .filter((line) => !blank(line) && !DASHED.test(line))
      .map((line) => line.trim())
      .filter((line) => !/^[←☐☒]/.test(line));
  }
  return [];
}

const CHROME = /esc to|for shortcuts|^[✻⏸]/i;

/** The last line that says something, above the input box when there is
 * one: what an unknown screen is waiting for. */
function lastMeaningful(lines: string[]): string {
  let end = lines.length;
  for (let i = lines.length - 1; i >= 0; i--)
    if (CURSOR.test(lines[i]) || /^\s*[❯›]\s*$/.test(lines[i])) {
      end = i;
      break;
    }
  for (let i = end - 1; i >= 0; i--) {
    const line = lines[i].replace(/^[\s⎿⏺•↳└│]+/, "").trim();
    if (!line || RULE.test(lines[i]) || DASHED.test(lines[i])) continue;
    if (CHROME.test(line)) continue;
    return line;
  }
  return "";
}

/** Parses a pane's visible text. `agent` is the session's agent id: Claude Code
 * answers a numbered menu on its digit alone (captured), anything else is
 * driven with arrows and Enter. */
export function parseSessionPrompt(
  screen: string,
  agent?: string,
): SessionPrompt {
  const lines = screen.replace(/\r/g, "").split("\n");
  while (lines.length && blank(lines[lines.length - 1])) lines.pop();
  const menu = numberedMenu(lines) ?? plainMenu(lines);
  if (!menu)
    return {
      question: lastMeaningful(lines),
      detail: [],
      options: [],
      multi: false,
      advance: [],
      typeSteps: [],
    };
  const digits = agent === "claude" && menu.numbered;
  const multi = menu.entries.some((entry) => entry.checked !== undefined);
  const stepsFor = (index: number) =>
    digits ? [String(index + 1)] : [navigate(menu.cursor, index)];
  let typeSteps: string[] | null = null;
  const options: PromptOption[] = [];
  menu.entries.forEach((entry, index) => {
    if (TYPE_SOMETHING.test(entry.label)) {
      if (!multi) typeSteps = stepsFor(index);
      return;
    }
    options.push({ ...entry, steps: stepsFor(index) });
  });
  const { question, top } = questionAbove(lines, menu.first);
  return {
    question,
    detail: detailAbove(lines, top),
    options,
    multi,
    advance: multi ? [RIGHT] : [],
    typeSteps,
  };
}

/** The input that sends `text` as a free-text answer, or null when the
 * screen has nowhere to type it. Newlines would submit early; they become
 * spaces. */
export function replySteps(
  prompt: SessionPrompt,
  text: string,
): string[] | null {
  const body = text.replace(/\s*\n\s*/g, " ").trim();
  if (!body || !prompt.typeSteps) return null;
  return [...prompt.typeSteps, body + ENTER];
}
