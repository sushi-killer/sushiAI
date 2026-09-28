// Pure helpers the renderer bends the protocol into a screen with - no
// window.bridge access here, so they're cheap to unit test directly.
import type {
  Attempt,
  Message,
  OrchestratorEvent,
  Settings,
  Task,
  Tier,
  Variant,
} from "./types";

export type TaskCreateParams = {
  request?: string;
  title?: string;
  goal?: string;
  criteria?: string[];
  verify?: string[];
  base?: string;
  start?: boolean;
};

/** Adds a trimmed `base` to a task.create params object, only when the field
 * isn't blank - an empty or whitespace-only base must send no `base` key at
 * all, so orchd's own HEAD fallback (the checked-out branch) applies instead
 * of an explicit empty string. */
export function taskCreateParams(
  params: Omit<TaskCreateParams, "base">,
  base: string,
): TaskCreateParams {
  const trimmed = base.trim();
  return trimmed ? { ...params, base: trimmed } : { ...params };
}

export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return "0s";
  const totalSeconds = Math.round(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours) return `${hours}h ${minutes}m`;
  if (minutes) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

export function formatCost(costUsd: number | undefined): string {
  return `$${(costUsd || 0).toFixed(2)}`;
}

/** `Variant`'s own field order - `variantLabel` walks it in this order so a
 * task with several differing flags always reads the same way. */
const VARIANT_KEYS: Exclude<keyof Variant, "plannerRoute" | "tierRoutes">[] = [
  "retryMode",
  "stallTimeoutSecs",
  "plannerTier",
  "contract",
  "reviewOtherFamily",
  "reviewEvidence",
  "deferHeavyChecks",
  "leanOutput",
];

function variantFlagText(value: boolean | string | number): string {
  return typeof value === "boolean" ? (value ? "on" : "off") : String(value);
}

function tierRoutesText(routes: Variant["tierRoutes"]): string {
  return Object.entries(routes ?? {})
    .map(([tier, route]) => `${tier}=${route}`)
    .join(",");
}

/** How a task's experiment arm reads in the detail view: "not recorded" for
 * a task from before variants existed, "default" when every flag matches
 * `experiments` (the current settings), or else only the flags that differ,
 * in `Variant`'s own field order, then any differing route overrides. */
export function variantLabel(task: Task, experiments: Variant): string {
  if (!task.variant) return "not recorded";
  const variant = task.variant;
  const diffs = VARIANT_KEYS.filter(
    (key) => variant[key] !== experiments[key],
  ).map((key) => `${key} ${variantFlagText(variant[key])}`);
  if (variant.plannerRoute !== experiments.plannerRoute) {
    diffs.push(`plannerRoute ${variant.plannerRoute ?? "settings"}`);
  }
  const tierRoutes = tierRoutesText(variant.tierRoutes);
  if (tierRoutes !== tierRoutesText(experiments.tierRoutes)) {
    diffs.push(`tierRoutes ${tierRoutes || "settings"}`);
  }
  if (diffs.length === 0) return "default";
  return diffs.join(" · ");
}

export type CostByStage = {
  plan: number;
  implement: { n: number; costUsd: number }[];
  review: number;
  other: number;
};

/** Splits a task's `costUsd` into where it went: the plan attempt(s), one
 * entry per implement attempt, the review runs, and `other` - the remainder
 * left by auto-answer runs and (for a task recorded before `reviewCostUsd`
 * existed) reviews with no cost attributed to any attempt. Clamped at 0
 * rather than going negative on rounding. */
export function costByStage(task: Task): CostByStage {
  const plan = task.attempts
    .filter((attempt) => attempt.stage === "plan")
    .reduce((sum, attempt) => sum + (attempt.costUsd || 0), 0);
  const implement = task.attempts
    .filter((attempt) => attempt.stage === "implement")
    .map((attempt) => ({ n: attempt.n, costUsd: attempt.costUsd || 0 }));
  const implementTotal = implement.reduce((sum, a) => sum + a.costUsd, 0);
  const review = task.attempts.reduce(
    (sum, attempt) => sum + (attempt.reviewCostUsd || 0),
    0,
  );
  const other = Math.max(0, task.costUsd - plan - implementTotal - review);
  return { plan, implement, review, other };
}

/** The detail-view meta line's tier text, e.g. "standard tier" or, when the
 * tier was picked by falling back instead of a classified/planner choice,
 * "standard tier (fallback: no classifier key)". */
export function formatTaskTier(task: Task): string {
  const base = `${task.tier} tier`;
  return task.tierFallback ? `${base} (fallback: ${task.tierFallback})` : base;
}

export function attemptDurationMs(attempt: Attempt, now = Date.now()): number {
  return (attempt.endedAt ?? now) - attempt.startedAt;
}

export function totalDurationMs(task: Task, now = Date.now()): number {
  return task.attempts.reduce(
    (sum, attempt) => sum + attemptDurationMs(attempt, now),
    0,
  );
}

export function latestAttempt(task: Task): Attempt | undefined {
  return task.attempts[task.attempts.length - 1];
}

/** The latest attempt that actually implements the task, skipping the
 * planning attempt - an attempt count or a "criteria met" read should never
 * count a still-open plan as if it were real implementation progress. */
export function latestImplementAttempt(task: Task): Attempt | undefined {
  for (let i = task.attempts.length - 1; i >= 0; i--) {
    if (task.attempts[i].stage === "implement") return task.attempts[i];
  }
  return undefined;
}

/** Number of implementation attempts, independent of the daemon's global
 * cross-stage attempt sequence. */
export function implementAttemptCount(task: Task): number {
  return task.attempts.filter((attempt) => attempt.stage === "implement")
    .length;
}

/** The newest review-stage verdict, if any attempt has reached one. */
export function reviewOf(task: Task) {
  for (let i = task.attempts.length - 1; i >= 0; i--) {
    const attempt = task.attempts[i];
    if (attempt.stage === "review" && attempt.review) return attempt.review;
  }
  return undefined;
}

/** "2/5"-style attempt progress for a task's list row - `null` once the task
 * is no longer actively working (there is nothing to count toward). */
export function attemptProgress(
  task: Task,
  maxAttempts?: number,
): string | null {
  if (task.status !== "running" && task.status !== "queued") return null;
  const n = implementAttemptCount(task);
  const max = maxAttempts ?? n;
  return `${n}/${max}`;
}

const STATUS_BADGE_LABELS: Record<Task["status"], string> = {
  drafting: "Drafting",
  queued: "Queued",
  running: "Running",
  waiting: "Waiting",
  done: "Done",
  stopped: "Stopped",
  failed: "Failed",
};

/** The short word a task-list pill badge shows - `statusDetail` carries
 * whatever else the row has to say underneath it. */
export function statusBadgeLabel(task: Task): string {
  return STATUS_BADGE_LABELS[task.status];
}

const REASON_MAX = 60;

function shortReason(task: Task): string {
  const detail =
    latestAttempt(task)?.failure?.detail ||
    task.decisions[task.decisions.length - 1] ||
    "";
  return detail.length > REASON_MAX
    ? `${detail.slice(0, REASON_MAX - 1)}…`
    : detail;
}

/** The task-list row's line under its pill: only what the pill and the
 * attempt progress don't already say, `""` when that is nothing. Never the
 * question text itself, so a row can't leak a private-looking question into
 * a list glanced at across a room. */
export function statusDetail(task: Task): string {
  switch (task.status) {
    case "running":
      return formatCost(task.costUsd);
    case "waiting":
      return "1 question for you";
    case "done": {
      const review = reviewOf(task);
      const cost = formatCost(task.costUsd);
      return review ? `review ${review.verdict} · ${cost}` : cost;
    }
    case "stopped":
    case "failed":
      return shortReason(task);
    default:
      return "";
  }
}

/** Whether a task's acceptance criteria can be shown as met: the task is
 * done, or its latest implement attempt already passed verify. */
export function criteriaMet(task: Task): boolean {
  return (
    task.status === "done" || latestImplementAttempt(task)?.status === "passed"
  );
}

/** A parent's subtasks in the order they were planned (oldest first). */
export function childrenOf(tasks: Task[], parentId: string): Task[] {
  return tasks
    .filter((t) => t.parent === parentId)
    .sort((a, b) => a.createdAt - b.createdAt);
}

/** Titles of the tasks `task` waits for; an id no longer listed is skipped. */
export function dependencyTitles(task: Task, tasks: Task[]): string[] {
  return (task.dependsOn ?? []).flatMap((id) => {
    const found = tasks.find((t) => t.id === id);
    return found ? [found.title] : [];
  });
}

/** Newest-updated first, matching `task.list`'s own order even after a live
 * `task` event patches one entry in place. */
export function upsertTask(tasks: Task[], task: Task): Task[] {
  const next = tasks.filter((item) => item.id !== task.id);
  next.push(task);
  next.sort((a, b) => b.updatedAt - a.updatedAt);
  return next;
}

export type OrchestratorLiveState = {
  tasks: Task[];
  /** Newest-last, capped, keyed by task id. */
  logLines: Record<string, string[]>;
};

export const emptyLiveState: OrchestratorLiveState = {
  tasks: [],
  logLines: {},
};

const MAX_LOG_LINES = 200;

/** Folds one `orchestrator-event` push into the panel's whole live state. */
export function applyOrchestratorEvent(
  state: OrchestratorLiveState,
  event: OrchestratorEvent,
): OrchestratorLiveState {
  if (event.event === "task")
    return { ...state, tasks: upsertTask(state.tasks, event.task) };
  if (
    event.event === "chat" ||
    event.event === "message" ||
    event.event === "audit"
  )
    return state;
  const lines = [...(state.logLines[event.taskId] || []), event.line].slice(
    -MAX_LOG_LINES,
  );
  return { ...state, logLines: { ...state.logLines, [event.taskId]: lines } };
}

/** Replaces a message by id (a `delivered` flip upserts in place) or appends
 * it, keeping the list sorted oldest-first - the same order `message.list`
 * returns and `MessagesView` renders threads in. */
export function upsertMessage(
  messages: Message[],
  message: Message,
): Message[] {
  const next = messages.filter((item) => item.id !== message.id);
  next.push(message);
  next.sort((a, b) => a.ts - b.ts);
  return next;
}

export type MessageThread = {
  /** The unordered participant pair, joined so both orderings hash alike. */
  key: string;
  participants: [string, string];
  /** Oldest first. */
  messages: Message[];
  lastTs: number;
  /** Count of messages not yet delivered. */
  pending: number;
};

function threadKey(a: string, b: string): string {
  return [a, b].sort().join("::");
}

/** Groups a flat message list into per-conversation threads, keyed by the
 * unordered {from, to} pair - a question and its reply flow the same way
 * whichever side sent which message. Threads are sorted newest-first by
 * their last message, and each thread's own messages stay oldest-first. */
export function messageThreads(messages: Message[]): MessageThread[] {
  const byKey = new Map<string, MessageThread>();
  for (const message of messages) {
    const key = threadKey(message.from, message.to);
    let thread = byKey.get(key);
    if (!thread) {
      thread = {
        key,
        participants: [message.from, message.to],
        messages: [],
        lastTs: message.ts,
        pending: 0,
      };
      byKey.set(key, thread);
    }
    thread.messages.push(message);
    thread.lastTs = Math.max(thread.lastTs, message.ts);
    if (!message.delivered) thread.pending++;
  }
  return [...byKey.values()].sort((a, b) => b.lastTs - a.lastTs);
}

/** How a participant id reads in the Messages view: the orchestrator's own
 * name, a task's title when it's still known, or a short id fallback for a
 * deleted/unknown task. */
export function participantLabel(id: string, tasks: Task[]): string {
  if (id === "orchestrator") return "Orchestrator";
  const task = tasks.find((t) => t.id === id);
  return task ? task.title : `Task ${id.slice(0, 8)}`;
}

/** Every setting the panel shows a default marker for. `routes` is left out
 * (the owner's own route list), and so are `classifier.providerId` (a
 * pointer to a stored key, not a choice with a default) and `experiments`/
 * `prices` (not shown in the panel). */
export type DefaultableSetting =
  | "tiers.mechanical"
  | "tiers.standard"
  | "tiers.hard"
  | "review"
  | "planner"
  | "orchestrator"
  | "autoAnswer"
  | "classifier.backend"
  | "classifier.model"
  | "sandbox"
  | "codexNetwork"
  | "allowedDomains"
  | "protectedPaths"
  | "maxAttempts"
  | "parallel";

/** One setting's value, read by its dotted `DefaultableSetting` path. */
export function settingValue(
  settings: Settings,
  field: DefaultableSetting,
): unknown {
  switch (field) {
    case "tiers.mechanical":
      return settings.tiers.mechanical;
    case "tiers.standard":
      return settings.tiers.standard;
    case "tiers.hard":
      return settings.tiers.hard;
    case "classifier.backend":
      return settings.classifier.backend;
    case "classifier.model":
      return settings.classifier.model;
    default:
      return settings[field];
  }
}

const DEFAULTABLE_SETTINGS: DefaultableSetting[] = [
  "tiers.mechanical",
  "tiers.standard",
  "tiers.hard",
  "review",
  "planner",
  "orchestrator",
  "autoAnswer",
  "classifier.backend",
  "classifier.model",
  "sandbox",
  "codexNetwork",
  "allowedDomains",
  "protectedPaths",
  "maxAttempts",
  "parallel",
];

/** The settings whose saved value differs from orchd's built-in default -
 * a save made before a default changed keeps the old value frozen. Route
 * fields compare route ids, never labels. */
export function settingsDifferingFromDefaults(
  saved: Settings,
  defaults: Settings,
): DefaultableSetting[] {
  return DEFAULTABLE_SETTINGS.filter(
    (field) =>
      JSON.stringify(settingValue(saved, field)) !==
      JSON.stringify(settingValue(defaults, field)),
  );
}

/** `saved` with one setting put back to its value in `defaults`. */
export function resetSettingToDefault(
  saved: Settings,
  defaults: Settings,
  field: DefaultableSetting,
): Settings {
  switch (field) {
    case "tiers.mechanical":
    case "tiers.standard":
    case "tiers.hard": {
      const tier = field.slice("tiers.".length) as Tier;
      return {
        ...saved,
        tiers: { ...saved.tiers, [tier]: defaults.tiers[tier] },
      };
    }
    case "classifier.backend":
      return {
        ...saved,
        classifier: {
          ...saved.classifier,
          backend: defaults.classifier.backend,
        },
      };
    case "classifier.model":
      return {
        ...saved,
        classifier: { ...saved.classifier, model: defaults.classifier.model },
      };
    default:
      return { ...saved, [field]: defaults[field] };
  }
}
