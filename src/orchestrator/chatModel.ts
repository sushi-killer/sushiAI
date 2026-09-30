// Pure helpers for the Chat view: session list grouping and times, task
// references in a reply, the modes and the proposed-task card.
import type {
  ChatMessage,
  ChatMode,
  ChatProposal,
  ChatQuestion,
  ProposedTask,
  Task,
} from "./types";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function startOfDay(ts: number): number {
  const d = new Date(ts);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "now", "2m", "3h" today; a weekday within the week; else "Sep 3";
 * nothing for a session an older daemon sent without a time. */
export function sessionTime(ts: number | undefined, now = Date.now()): string {
  if (ts == null || !Number.isFinite(ts)) return "";
  const age = now - ts;
  if (ts >= startOfDay(now)) {
    if (age < MINUTE) return "now";
    if (age < HOUR) return `${Math.floor(age / MINUTE)}m`;
    return `${Math.floor(age / HOUR)}h`;
  }
  if (age < 6 * DAY)
    return new Date(ts).toLocaleDateString("en-US", { weekday: "short" });
  return new Date(ts).toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
  });
}

const timeOf = (session: { updatedAt?: number }): number | null =>
  session.updatedAt != null && Number.isFinite(session.updatedAt)
    ? session.updatedAt
    : null;

/** Newest first by orchd's `updatedAt`; a session without one sorts last. */
export function newestFirst(
  a: { updatedAt?: number },
  b: { updatedAt?: number },
): number {
  const [x, y] = [timeOf(a), timeOf(b)];
  if (x === null || y === null) return x === y ? 0 : x === null ? 1 : -1;
  return y - x;
}

/** Newest first, split into TODAY and EARLIER (a session without a time is
 * earlier). */
export function groupSessions<S extends { updatedAt?: number }>(
  sessions: S[],
  now = Date.now(),
): { label: "TODAY" | "EARLIER"; sessions: S[] }[] {
  const today = startOfDay(now);
  const newest = [...sessions].sort(newestFirst);
  const isToday = (s: S) => (timeOf(s) ?? -Infinity) >= today;
  return [
    {
      label: "TODAY" as const,
      sessions: newest.filter(isToday),
    },
    {
      label: "EARLIER" as const,
      sessions: newest.filter((s) => !isToday(s)),
    },
  ].filter((group) => group.sessions.length > 0);
}

export function matchesQuery(
  query: string,
  ...fields: (string | undefined)[]
): boolean {
  const q = query.trim().toLowerCase();
  return !q || fields.some((f) => f?.toLowerCase().includes(q));
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Known tasks a reply names: by id (whole, or its first 8 characters) or by
 * a title of at least 12 characters, in the order they first appear. */
export function taskRefs(text: string, tasks: Task[], max = 3): Task[] {
  const lower = text.toLowerCase();
  const found: { task: Task; at: number }[] = [];
  for (const task of tasks) {
    const byId = new RegExp(
      `\\b${escapeRegExp(task.id.slice(0, 8).toLowerCase())}`,
    ).exec(lower);
    const title = task.title.trim().toLowerCase();
    const byTitle = title.length >= 12 ? lower.indexOf(title) : -1;
    const hits = [byId?.index ?? -1, byTitle].filter((i) => i >= 0);
    if (hits.length) found.push({ task, at: Math.min(...hits) });
  }
  return found
    .sort((a, b) => a.at - b.at)
    .slice(0, max)
    .map((f) => f.task);
}

/** A session row's preview: its last message with any text, on one line. */
export function lastMessageText(thread: {
  messages: { text: string }[];
}): string {
  const last = [...thread.messages].reverse().find((m) => m.text.trim());
  return last ? last.text.replace(/\s+/g, " ").trim() : "";
}

/** A session changed after the owner last looked at it. `since` stands in
 * for a session never opened here, so an old history never reads as new. */
export function isUnread(
  session: { updatedAt?: number },
  seenAt: number | undefined,
  since: number,
): boolean {
  const at = timeOf(session);
  return at !== null && at > (seenAt ?? since);
}

export const CHAT_MODES: { value: ChatMode; label: string }[] = [
  { value: "chat", label: "Chat" },
  { value: "brainstorm", label: "Brainstorm" },
  { value: "plan", label: "Plan" },
];

/** The label above a reply: the mode's name, nothing for the plain chat. */
export function modeLabel(mode: ChatMode | undefined): string {
  return (
    CHAT_MODES.find((m) => m.value === mode && m.value !== "chat")?.label ?? ""
  );
}

/** The composer's placeholder for the selected mode. A brainstorm that has
 * asked or proposed something is answered, not started. */
export function composerPlaceholder(
  mode: ChatMode,
  messages: Pick<ChatMessage, "role" | "mode" | "questions" | "proposal">[],
): string {
  if (mode === "plan") return "Describe the goal to plan…";
  if (mode === "chat") return "Ask the orchestrator…";
  const asked = messages.some(
    (m) =>
      m.role === "assistant" &&
      m.mode === "brainstorm" &&
      ((m.questions?.length ?? 0) > 0 || m.proposal),
  );
  return asked ? "Answer, or push back on the list…" : "Describe an idea…";
}

/** "hard · 2 criteria · after Add the schema": what a proposal row shows
 * under its title. A dependency is another row's title, or an existing
 * task's. */
export function proposalMeta(
  task: ProposedTask,
  proposal: ChatProposal,
  tasks: Pick<Task, "id" | "title">[],
): string {
  const after = task.dependsOn
    .map((dep) =>
      typeof dep === "number"
        ? proposal.tasks[dep - 1]?.title
        : tasks.find((t) => t.id === dep)?.title,
    )
    .filter(Boolean);
  const n = task.criteria.length;
  return [
    task.tier,
    `${n} ${n === 1 ? "criterion" : "criteria"}`,
    after.length ? `after ${after.join(", ")}` : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

export type ProposalRowState = "created" | "skipped" | "open";

/** A row is created once it has a task, skipped once passed on (or unchecked
 * here), else open. */
export function rowState(
  task: ProposedTask,
  checked: boolean,
): ProposalRowState {
  if (task.taskId) return "created";
  return task.skipped || !checked ? "skipped" : "open";
}

/** The 1-based rows a create call acts on: the checked rows that are not
 * tasks yet, and the unchecked ones to record as skipped. */
export function proposalSelection(
  proposal: ChatProposal,
  checked: boolean[],
): { create: number[]; skip: number[] } {
  const create: number[] = [];
  const skip: number[] = [];
  proposal.tasks.forEach((task, i) => {
    if (task.taskId) return;
    (checked[i] ? create : skip).push(i + 1);
  });
  return { create, skip };
}

/** A card's checkboxes at first sight: every row not passed on or created. */
export function initialChecks(proposal: ChatProposal): boolean[] {
  return proposal.tasks.map((task) => !task.skipped);
}

/** "Create 3 tasks": what the footer's main button says. */
export function createLabel(count: number): string {
  return `Create ${count} ${count === 1 ? "task" : "tasks"}`;
}

/** The `chat.createProposal` params for a click: one row's Create (`row`,
 * 1-based) or the footer's buttons (every checked row), parked in the plan
 * with `backlog`. Unchecked rows are recorded as skipped either way; `null`
 * when nothing would be created. */
export function proposalRequest(
  proposal: ChatProposal,
  checked: boolean[],
  { row, backlog = false }: { row?: number; backlog?: boolean } = {},
): { indices: number[]; skip: number[]; backlog: boolean } | null {
  const { create, skip } = proposalSelection(proposal, checked);
  const indices = row === undefined ? create : [row];
  if (!indices.length) return null;
  return {
    indices,
    skip: skip.filter((n) => !indices.includes(n)),
    backlog,
  };
}

/** The one message that answers several questions: `<question>: <answer>` per
 * line, unanswered questions left out. `null` when nothing is answered. */
export function composeAnswers(
  questions: ChatQuestion[],
  picks: (string | undefined)[],
): string | null {
  const lines = questions.flatMap((q, i) =>
    picks[i] ? [`${q.text}: ${picks[i]}`] : [],
  );
  return lines.length ? lines.join("\n") : null;
}

/** What a sent answers message picked, per question, read back from its text
 * so the chips stay selected after a reload. */
export function pickedAnswers(
  questions: ChatQuestion[],
  sent: string | undefined,
): (string | undefined)[] {
  const lines = (sent ?? "").split("\n");
  return questions.map((q) => {
    const prefix = `${q.text}: `;
    const line = lines.find((l) => l.startsWith(prefix));
    const answer = line?.slice(prefix.length);
    return answer && q.options.includes(answer) ? answer : undefined;
  });
}

/** What a key does in the inline editor of an own message: Enter sends,
 * Shift+Enter is a newline, Esc cancels. */
export function editKeyAction(key: {
  key: string;
  shiftKey: boolean;
  isComposing?: boolean;
}): "send" | "cancel" | null {
  if (key.isComposing) return null;
  if (key.key === "Escape") return "cancel";
  if (key.key === "Enter" && !key.shiftKey) return "send";
  return null;
}

/** An edit sends when the text is not empty; it only changes the thread
 * when it differs from the message (either way Send is allowed to resend). */
export function canSendEdit(text: string, busy: boolean): boolean {
  return !busy && text.trim().length > 0;
}

/** True for the messages that follow the one being edited: they are dimmed
 * until Send or Cancel. */
export function isAfterEdit(
  messages: { id: string }[],
  editingId: string | null,
  index: number,
): boolean {
  if (!editingId) return false;
  const at = messages.findIndex((m) => m.id === editingId);
  return at >= 0 && index > at;
}
