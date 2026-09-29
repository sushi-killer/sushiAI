// Pure model behind the Plan and Brainstorm views: which drafts sit where,
// and how a brainstorm chat turns into a draft task. No React, no I/O.
import { dependencyTitles, planDrafts } from "./helpers.ts";
import type { Task } from "./types.ts";

/** Fields orchd may add to a draft task later; read defensively. */
type Ordered = { bucket?: unknown; order?: unknown };

export type PlanItem = {
  task: Task;
  /** Drafts the planner split out of this one, in planned order. */
  children: PlanChild[];
  /** Every dependency is done (or there is none). */
  ready: boolean;
};
export type PlanChild = {
  task: Task;
  /** Titles this draft comes after. */
  after: string[];
  waits: boolean;
};
export type PlanModel = {
  next: PlanItem[];
  later: PlanItem[];
  /** True when the daemon supplied NEXT/LATER buckets. */
  bucketed: boolean;
};

export function criteriaLabel(count: number): string {
  return `${count} ${count === 1 ? "criterion" : "criteria"}`;
}

/** "hard · 5 criteria": the meta a draft row carries. */
export function draftMeta(task: Task): string {
  return `${task.tier} · ${criteriaLabel(task.criteria.length)}`;
}

function depsDone(task: Task, tasks: Task[]): boolean {
  return (task.dependsOn ?? []).every(
    (id) => tasks.find((t) => t.id === id)?.status === "done",
  );
}

function orderOf(task: Task): number | null {
  const order = (task as Ordered).order;
  return typeof order === "number" && Number.isFinite(order) ? order : null;
}

function bucketOf(task: Task): "next" | "later" | null {
  const bucket = (task as Ordered).bucket;
  return bucket === "next" || bucket === "later" ? bucket : null;
}

function byPlanOrder(a: Task, b: Task): number {
  const ao = orderOf(a);
  const bo = orderOf(b);
  if (ao !== null && bo !== null && ao !== bo) return ao - bo;
  if (ao !== null && bo === null) return -1;
  if (ao === null && bo !== null) return 1;
  return a.createdAt - b.createdAt;
}

/** The Plan's lists. A draft whose planner-split children are drafts too
 * becomes a group; without daemon buckets everything is NEXT, oldest first. */
export function planModel(tasks: Task[]): PlanModel {
  const drafts = planDrafts(tasks);
  const ids = new Set(drafts.map((t) => t.id));
  const top = drafts
    .filter((t) => !t.parent || !ids.has(t.parent))
    .sort(byPlanOrder);
  const items: PlanItem[] = top.map((task) => ({
    task,
    children: drafts
      .filter((t) => t.parent === task.id)
      .sort((a, b) => a.createdAt - b.createdAt)
      .map((child) => ({
        task: child,
        after: dependencyTitles(child, tasks),
        waits: !depsDone(child, tasks),
      })),
    ready: depsDone(task, tasks),
  }));
  const bucketed = top.some((t) => bucketOf(t) !== null);
  if (!bucketed) return { next: items, later: [], bucketed };
  return {
    next: items.filter((i) => bucketOf(i.task) !== "later"),
    later: items.filter((i) => bucketOf(i.task) === "later"),
    bucketed,
  };
}

/** Drafts "Start n ready" starts: every NEXT top-level draft that is not
 * waiting on anything. */
export function readyDrafts(model: PlanModel): Task[] {
  return model.next.filter((i) => i.ready).map((i) => i.task);
}

/** The Autopilot setting when the daemon has one; `null` = not available. */
export function autopilotOf(settings: unknown): boolean | null {
  const value = (settings as { autopilot?: unknown } | null)?.autopilot;
  if (typeof value === "boolean") return value;
  if (value && typeof value === "object") {
    const enabled = (value as { enabled?: unknown }).enabled;
    if (typeof enabled === "boolean") return enabled;
  }
  return null;
}

// ---- Brainstorm ----------------------------------------------------------

export type DraftCriterion = { text: string; met: boolean };
export type DraftTask = {
  title: string;
  goal: string;
  criteria: DraftCriterion[];
  /** Titles or ids of the tasks it waits for. */
  dependsOn: string[];
  tier?: string;
  base?: string;
};

const FENCE = /```(?:sushi-plan|sushi-draft)[^\n]*\n([\s\S]*?)```/g;

function strings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((v): v is string => typeof v === "string" && v.trim() !== "")
    : [];
}

/** Reads a draft from an unknown object (a daemon `draft`, or a parsed
 * block); `null` when it has no title. */
export function readDraft(value: unknown): DraftTask | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const title = typeof raw.title === "string" ? raw.title.trim() : "";
  if (!title) return null;
  const criteria: DraftCriterion[] = [];
  if (Array.isArray(raw.criteria)) {
    for (const item of raw.criteria) {
      if (typeof item === "string" && item.trim())
        criteria.push({ text: item.trim(), met: false });
      else if (item && typeof item === "object") {
        const c = item as { text?: unknown; met?: unknown };
        if (typeof c.text === "string" && c.text.trim())
          criteria.push({ text: c.text.trim(), met: c.met === true });
      }
    }
  }
  return {
    title,
    goal: typeof raw.goal === "string" ? raw.goal.trim() : "",
    criteria,
    dependsOn: strings(raw.dependsOn),
    ...(typeof raw.tier === "string" && raw.tier ? { tier: raw.tier } : {}),
    ...(typeof raw.base === "string" && raw.base ? { base: raw.base } : {}),
  };
}

/** The last fenced `sushi-plan` block in `text`, parsed; `null` when there is
 * none or it is not valid JSON with a title. */
export function parseDraftBlock(text: string): DraftTask | null {
  let found: DraftTask | null = null;
  for (const match of text.matchAll(FENCE)) {
    try {
      found = readDraft(JSON.parse(match[1])) ?? found;
    } catch {
      // A half-written block is skipped.
    }
  }
  return found;
}

/** `text` without its draft blocks, for showing in a chat bubble. */
export function stripDraftBlock(text: string): string {
  return text.replace(FENCE, "").trim();
}

/** The chat message that asks the orchestrator for a draft block. */
export const DRAFT_REQUEST =
  "Summarise what we settled in this brainstorm as one draft task. Reply with a short sentence and one fenced ```sushi-plan block holding JSON: " +
  '{"title": string, "goal": string, "criteria": string[], "dependsOn": string[], "tier": "mechanical"|"standard"|"hard", "base": string}. ' +
  "Use an empty array or omit a field you do not know yet.";

export type ChatLike = {
  role: string;
  text: string;
  options?: unknown;
};

/** The draft the panel shows: a structured `draft` on the thread when orchd
 * sends one, else the newest block an orchestrator reply carries. */
export function draftOfThread(thread: {
  draft?: unknown;
  messages: ChatLike[];
}): DraftTask | null {
  const structured = readDraft(thread.draft);
  if (structured) return structured;
  for (let i = thread.messages.length - 1; i >= 0; i--) {
    const message = thread.messages[i];
    if (message.role !== "assistant") continue;
    const parsed = parseDraftBlock(message.text);
    if (parsed) return parsed;
  }
  return null;
}

/** Option chips a turn carries (`options: string[]` from orchd). */
export function optionsOf(message: ChatLike): string[] {
  return strings(message.options);
}

/** Resolves dependency titles to ids of listed tasks; unknown ones drop. */
export function dependencyIds(draft: DraftTask, tasks: Task[]): string[] {
  const ids: string[] = [];
  for (const ref of draft.dependsOn) {
    const found = tasks.find(
      (t) => t.id === ref || t.title.toLowerCase() === ref.toLowerCase(),
    );
    if (found && !ids.includes(found.id)) ids.push(found.id);
  }
  return ids;
}

/** `task.create` params for a draft: `start` false adds it to the plan. */
export function draftCreateParams(
  draft: DraftTask,
  tasks: Task[],
  start: boolean,
): Record<string, unknown> {
  const dependsOn = dependencyIds(draft, tasks);
  return {
    title: draft.title,
    goal: draft.goal || draft.title,
    ...(draft.criteria.length
      ? { criteria: draft.criteria.map((c) => c.text) }
      : {}),
    ...(draft.base ? { base: draft.base } : {}),
    ...(dependsOn.length ? { dependsOn } : {}),
    start,
  };
}
