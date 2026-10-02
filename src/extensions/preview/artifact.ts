// Pure helpers for the Artifacts Preview view: what a file is, what a plan
// says, and the text sent back to an agent. No React, no bridge.

export type ArtifactKind =
  "markdown" | "html" | "image" | "pdf" | "svg" | "text";

const EXTENSIONS: Record<string, ArtifactKind> = {
  md: "markdown",
  markdown: "markdown",
  html: "html",
  htm: "html",
  png: "image",
  jpg: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  avif: "image",
  ico: "image",
  pdf: "pdf",
  svg: "svg",
};

export function kindOf(path: string): ArtifactKind {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  if (dot < 0) return "text";
  return EXTENSIONS[name.slice(dot + 1).toLowerCase()] || "text";
}

/** Splits an absolute path into its folder and file name. The folder is the
 * root a read or preview is granted for; the name is the path inside it. */
export function splitPath(path: string): { dir: string; base: string } {
  const cut = path.lastIndexOf("/");
  if (cut < 0) return { dir: ".", base: path };
  return {
    dir: cut === 0 ? "/" : path.slice(0, cut),
    base: path.slice(cut + 1),
  };
}

/** Reads `key: value` lines between two leading `---` lines. Without a closing
 * line the text is not frontmatter and is returned whole. */
export function parseFrontmatter(text: string): {
  meta: Record<string, string>;
  body: string;
} {
  const lines = text.replace(/^\uFEFF/, "").split(/\r?\n/);
  if (lines[0]?.trim() !== "---") return { meta: {}, body: text };
  const end = lines.findIndex(
    (line, index) => index > 0 && line.trim() === "---",
  );
  if (end < 0) return { meta: {}, body: text };
  const meta: Record<string, string> = {};
  for (const line of lines.slice(1, end)) {
    const match = /^([A-Za-z0-9_-]+)\s*:\s*(.*)$/.exec(line);
    if (!match) continue;
    meta[match[1]] = match[2].trim().replace(/^(["'])(.*)\1$/, "$2");
  }
  return {
    meta,
    body: lines
      .slice(end + 1)
      .join("\n")
      .replace(/^\n+/, ""),
  };
}

/** A plan counts as verified only when the agent wrote `verified: yes`. */
export function isVerified(meta: Record<string, string>): boolean {
  return ["yes", "true"].includes((meta.verified || "").toLowerCase());
}

export type PlanSections = { title: string; goal: string; doneWhen: string[] };

export function planSections(body: string): PlanSections {
  const result: PlanSections = { title: "", goal: "", doneWhen: [] };
  let section = "";
  let fenced = false;
  const goal: string[] = [];
  for (const line of body.split(/\r?\n/)) {
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    const heading = fenced ? null : /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    if (heading) {
      if (heading[1].length === 1) {
        if (!result.title) result.title = heading[2];
        section = "";
      } else if (heading[1].length === 2) {
        const name = heading[2].toLowerCase();
        section = name === "goal" ? "goal" : name === "done when" ? "done" : "";
      }
      continue;
    }
    if (section === "goal") goal.push(line);
    else if (section === "done") {
      const item = /^\s*(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?(.*\S)\s*$/.exec(
        line,
      );
      if (item) result.doneWhen.push(item[1]);
    }
  }
  result.goal = goal
    .join("\n")
    .trim()
    .replace(/\n{2,}/g, "\n\n");
  return result;
}

/** The longest plan sent as the `/goal` text itself. A longer one is read
 * from its file, so the first prompt stays small. */
export const GOAL_INLINE_LIMIT = 12000;

/** Plan text that is safe to type into a terminal: tabs become two spaces,
 * and every other control character except a newline is dropped. */
export function plainText(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    .replace(/\t/g, "  ")
    .replace(/[\x00-\x09\x0b-\x1f\x7f-\x9f]/g, "");
}

export function goalPrompt(planBody: string, absPath: string): string {
  const body = plainText(planBody).trim();
  if (body.length > GOAL_INLINE_LIMIT)
    return `/goal Carry out the plan in ${plainText(absPath)}. Read it first: it has the goal, context, steps, done-when, verify and stop rules.`;
  return `/goal ${body}`;
}

export type Comment = {
  quote?: string;
  /** Where in the page the comment points: "slide #/2", a heading, "point at
   * 42%, 18%". */
  where?: string;
  /** A pin on an image, as percentages of its size. */
  pin?: { x: number; y: number };
  note: string;
};

const QUOTE_LIMIT = 300;

/** Drops control characters but keeps tab and newline, so no field can carry
 * an escape sequence (a bracketed-paste end marker, a Ctrl key) into a pane. */
const clean = (text: string) =>
  text.replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, "");

/** One message for every comment, so the agent sees them together. */
export function commentMessage(path: string, comments: Comment[]): string {
  const entries = comments.map((comment, index) => {
    const note = clean(comment.note).trim().replace(/\n/g, "\n   ");
    const quote = clean(comment.quote || "")
      .replace(/\s+/g, " ")
      .trim();
    const where = clean(comment.where || "")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 120);
    const at = where ? `(${where}) ` : "";
    if (!quote) return `${index + 1}. ${at}${note}`;
    const shown =
      quote.length > QUOTE_LIMIT ? `${quote.slice(0, QUOTE_LIMIT)}…` : quote;
    return `${index + 1}. ${at}> ${shown}\n   ${note}`;
  });
  return `Comments on ${clean(path)}:\n\n${entries.join("\n")}`;
}

export type Annotation = {
  kind: "text" | "element" | "cancel";
  quote: string;
  where: string;
  rect: { x: number; y: number; w: number; h: number };
};

/** Validates a message from the annotate script in an HTML page. Only a
 * message from the page's own frame counts; every field is clipped and a
 * non-number rect value becomes 0. */
export function acceptAnnotation(
  data: unknown,
  fromFrame: boolean,
): Annotation | null {
  if (!fromFrame || !data || typeof data !== "object") return null;
  const value = data as Record<string, unknown>;
  if (value.sushiai !== "annotate") return null;
  if (
    value.kind !== "text" &&
    value.kind !== "element" &&
    value.kind !== "cancel"
  )
    return null;
  const clip = (item: unknown, limit: number) =>
    typeof item === "string" ? item.slice(0, limit) : "";
  const rect = (
    value.rect && typeof value.rect === "object" ? value.rect : {}
  ) as Record<string, unknown>;
  const num = (item: unknown) =>
    typeof item === "number" && Number.isFinite(item) ? item : 0;
  return {
    kind: value.kind,
    quote: clip(value.quote, 500),
    where: clip(value.where, 120),
    rect: { x: num(rect.x), y: num(rect.y), w: num(rect.w), h: num(rect.h) },
  };
}

/** The comment box is this wide (CSS `.pv-comment-box`). */
export const COMMENT_BOX_WIDTH = 300;

/** Left edge for a comment box that stays inside a pane `wrapWidth` wide. */
export function clampBoxX(
  x: number,
  wrapWidth: number,
  boxWidth = COMMENT_BOX_WIDTH,
): number {
  const width = Math.min(boxWidth, wrapWidth - 16);
  return Math.max(8, Math.min(x, wrapWidth - width - 8));
}

/** Unsent comments kept per file path; an empty list drops the entry. */
export function withComments(
  byPath: Record<string, Comment[]>,
  path: string,
  list: Comment[],
): Record<string, Comment[]> {
  const rest = { ...byPath };
  delete rest[path];
  return list.length ? { ...rest, [path]: list } : rest;
}

export function branchFor(title: string): string {
  const slug = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40)
    .replace(/-+$/, "");
  return `feature/${slug || "plan"}`;
}

/** The terminal paste of a message: bracketed, so its newlines stay literal
 * and an agent TUI takes it as one message. Submitting is a separate Enter. */
export function pasteOf(message: string): string {
  return `\x1b[200~${clean(message)}\x1b[201~`;
}

export function agoText(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return `${seconds} s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  return `${Math.floor(minutes / 60)} h ago`;
}

export const RECENT_LIMIT = 8;

export function parseRecent(raw: string | undefined): string[] {
  try {
    const value = JSON.parse(raw || "[]");
    return Array.isArray(value)
      ? value.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
  }
}

export function pushRecent(list: string[], path: string): string[] {
  return [path, ...list.filter((item) => item !== path)].slice(0, RECENT_LIMIT);
}

/** What Start task did for one file, kept in the pane's args so the button
 * survives a reload. */
export type StartRecord =
  | { kind: "orchestrator"; host: string; repo: string; taskId: string }
  | { kind: "agent"; agent: string; branch: string };

export function parseStarts(
  raw: string | undefined,
): Record<string, StartRecord> {
  try {
    const value = JSON.parse(raw || "{}");
    return value && typeof value === "object" && !Array.isArray(value)
      ? value
      : {};
  } catch {
    return {};
  }
}

export function parseComments(
  raw: string | undefined,
): Record<string, Comment[]> {
  try {
    const value = JSON.parse(raw || "{}");
    return value && typeof value === "object" && !Array.isArray(value)
      ? value
      : {};
  } catch {
    return {};
  }
}

/** The label after "#<id> ·" for an orchestrator task status. */
export function taskStatusLabel(status: string): string {
  if (status === "waiting") return "waiting for you";
  if (status === "drafting" || status === "queued") return "queued";
  if (status === "landing") return "landing";
  return status;
}

/** The path when it is absolute, free of `..` and inside one of the roots
 * (the project folder, the agent pane's own folder); otherwise null. The
 * Preview reads and serves only such files. */
export function insideProject(path: string, ...roots: string[]): string | null {
  if (!path.startsWith("/")) return null;
  if (path.includes("\0") || path.split("/").includes("..")) return null;
  const norm = (value: string) => value.replace(/\/+/g, "/").replace(/\/$/, "");
  const file = norm(path);
  for (const cwd of roots) {
    if (!cwd?.startsWith("/")) continue;
    const root = norm(cwd);
    if (!root || root.split("/").includes("..")) continue;
    if (file.startsWith(`${root}/`)) return file;
  }
  return null;
}

/** The agent pane's own folder as a second Preview root, only when it sits
 * inside the project's parent folder and is not that parent: the project,
 * a folder in it, or a sibling worktree. A shell that `cd`s to `/`, the home
 * folder or any other ancestor gets no wider scope. */
export function paneRoot(cwd: string, paneCwd: string | undefined): string {
  if (!paneCwd || !cwd.startsWith("/") || !paneCwd.startsWith("/")) return "";
  const norm = (value: string) => value.replace(/\/+/g, "/").replace(/\/$/, "");
  const project = norm(cwd);
  const parent = project.slice(0, project.lastIndexOf("/"));
  if (!parent) return "";
  const pane = norm(paneCwd);
  return pane.startsWith(`${parent}/`) ? pane : "";
}

type TaskClient = {
  taskCreate(
    repo: string,
    input: { request: string; start: boolean; source: string },
  ): Promise<{ id: string }>;
};

/** Hands a plan to the orchestrator. The source is `ui`: orchd accepts only
 * its own fixed list of task sources. */
export async function startOrchestratorTask(
  client: TaskClient,
  repo: string,
  body: string,
  path: string,
): Promise<string> {
  const task = await client.taskCreate(repo, {
    request: `${body.trim()}\n\nPlan file: ${path}`,
    start: true,
    source: "ui",
  });
  return task.id;
}

/** The path as the owner reads it: relative to the project when inside it. */
export function shownPath(path: string, cwd: string): string {
  const root = cwd.replace(/\/+$/, "");
  const relative =
    root && path.startsWith(root + "/") ? path.slice(root.length + 1) : path;
  // Agents write into `artifacts/`, so its files show by name alone.
  return relative.replace(/^artifacts\//, "");
}
