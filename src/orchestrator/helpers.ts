// Pure helpers the renderer bends the protocol into a screen with - no
// window.bridge access here, so they're cheap to unit test directly.
import { needsOwner, questionCount, questionsLabel } from "./ownerAttention.ts";
import type {
  Attempt,
  FailureKind,
  Fingerprint,
  Message,
  OrchestratorEvent,
  QuestionKind,
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
  source?: string;
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
  const minutes = Number.isFinite(ms) ? Math.floor(ms / 60_000) : 0;
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const rest = minutes % 60;
  return rest ? `${Math.floor(minutes / 60)}h ${rest}m` : `${minutes / 60}h`;
}

export function formatCost(costUsd: number | undefined): string {
  return `$${(costUsd || 0).toFixed(2)}`;
}

/** `Variant`'s own field order - `variantLabel` walks it in this order so a
 * task with several differing flags always reads the same way. */
const VARIANT_KEYS: Exclude<
  keyof Variant,
  | "plannerRoute"
  | "tierRoutes"
  | "maxCostUsd"
  | "maxAttemptCostUsd"
  | "batchQuestions"
  | "groundedChecks"
  | "bestOf"
  | "bestOfRoute"
  | "land"
>[] = ["stallTimeoutSecs", "reviewEvidence"];

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
 * in `Variant`'s own field order, then any differing route overrides and
 * dollar budget. */
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
  const budget = variant.maxCostUsd ?? 0;
  if (budget !== (experiments.maxCostUsd ?? 0)) {
    diffs.push(`maxCostUsd ${budget ? formatCost(budget) : "none"}`);
  }
  const attemptCap = variant.maxAttemptCostUsd ?? 0;
  if (attemptCap !== (experiments.maxAttemptCostUsd ?? 0)) {
    diffs.push(
      `maxAttemptCostUsd ${attemptCap ? formatCost(attemptCap) : "none"}`,
    );
  }
  if (
    (variant.batchQuestions ?? false) !== (experiments.batchQuestions ?? false)
  ) {
    diffs.push(`batchQuestions ${variant.batchQuestions ? "on" : "off"}`);
  }
  if (!!variant.groundedChecks !== !!experiments.groundedChecks) {
    diffs.push(`groundedChecks ${variantFlagText(!!variant.groundedChecks)}`);
  }
  const bestOf = variant.bestOf ?? 0;
  if (bestOf >= 2 !== (experiments.bestOf ?? 0) >= 2) {
    diffs.push(`bestOf ${bestOf >= 2 ? "on" : "off"}`);
  }
  if (variant.bestOfRoute !== experiments.bestOfRoute) {
    diffs.push(`bestOfRoute ${variant.bestOfRoute ?? "settings"}`);
  }
  if (diffs.length === 0) return "default";
  return diffs.join(" · ");
}

const STAGE_ORDER = [
  "plan",
  "implement",
  "review",
  "advisor",
  "brief_check",
  "pick",
  "final",
  "eval_check",
  "triage",
  "answer_judge",
  "finding_judge",
  "audit",
  "evolution",
  "chat",
];

/** `brief_check` -> `Brief check`. */
export function stageLabel(stage: string): string {
  const text = stage.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Cost records' stage rows in pipeline order, unknown stages last, rows
 * that round to $0.00 dropped. */
export function orderedStageRows<T extends { key: string; costUsd: number }>(
  rows: T[],
): T[] {
  const rank = (key: string) => {
    const index = STAGE_ORDER.indexOf(key);
    return index < 0 ? STAGE_ORDER.length : index;
  };
  return rows
    .filter((row) => formatCost(row.costUsd) !== "$0.00")
    .sort((a, b) => rank(a.key) - rank(b.key));
}

export type CostByStage = {
  plan: number;
  implement: { n: number; costUsd: number }[];
  review: number;
  other: number;
};

/** `model (harness version)` of what a run really used. */
export function fingerprintLabel(fp: Fingerprint): string {
  const models = fp.models.length > 0 ? fp.models.join("+") : "?";
  return fp.harnessVersion ? `${models} (${fp.harnessVersion})` : models;
}

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
 * tier was picked by falling back instead of a planner choice,
 * "standard tier (fallback: no planner tier)". */
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

const STATUS_BADGE_LABELS: Record<Task["status"], string> = {
  drafting: "Drafting",
  queued: "Queued",
  running: "Running",
  waiting: "Waiting",
  landing: "Landing",
  done: "Done",
  stopped: "Stopped",
  failed: "Failed",
};

/** The short word a task-list pill badge shows. */
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
    event.event === "audit" ||
    event.event === "proposal"
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

/** How a participant id reads in the Messages view: the orchestrator's own
 * name, a task's title when it's still known, or a short id fallback for a
 * deleted/unknown task. */
export function participantLabel(id: string, tasks: Task[]): string {
  if (id === "orchestrator") return "Orchestrator";
  const task = tasks.find((t) => t.id === id);
  return task ? task.title : `Task ${id.slice(0, 8)}`;
}

/** Every setting the panel shows a default marker for. `routes` is left out
 * (the owner's own route list), and so are `experiments`/`prices` (not shown
 * in the panel). */
export type DefaultableSetting =
  | "tiers.mechanical"
  | "tiers.standard"
  | "tiers.hard"
  | "review"
  | "planner"
  | "orchestrator"
  | "autoAnswer"
  | "sandbox"
  | "codexNetwork"
  | "allowedDomains"
  | "protectedPaths"
  | "maxAttempts"
  | "parallel"
  | "childParallel";

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
  "sandbox",
  "codexNetwork",
  "allowedDomains",
  "protectedPaths",
  "maxAttempts",
  "parallel",
  "childParallel",
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
    default:
      return { ...saved, [field]: defaults[field] };
  }
}

/** Electron wraps a rejected IPC call as "Error invoking remote method
 * 'orchestrator': Error: <daemon message>"; the owner only needs the last
 * part. */
export function errorText(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(
    /^Error invoking remote method '[^']*': (Error: )?/,
    "",
  );
}

/** Repos where landing on the default branch is allowed, for the Settings list. */
export function landRepos(map: Record<string, boolean> | undefined): string[] {
  return Object.entries(map ?? {})
    .filter(([, allowed]) => allowed)
    .map(([repo]) => repo);
}

/** The map for an edited list: listed repos are allowed, removed ones fall
 * back to the global switch, and explicit `false` entries stay untouched. */
export function withLandRepos(
  map: Record<string, boolean> | undefined,
  repos: string[],
): Record<string, boolean> {
  const next: Record<string, boolean> = {};
  for (const [repo, allowed] of Object.entries(map ?? {}))
    if (!allowed) next[repo] = false;
  for (const repo of repos) next[repo] = true;
  return next;
}

/** One tone per status family, shared by a row's dot and its pill badge:
 * in flight reads blue, needs-you yellow, finished well green, went wrong
 * red, not started yet muted. */
export function statusTone(task: Task): string {
  switch (task.status) {
    case "drafting":
    case "running":
      return "blue";
    case "waiting":
    case "landing":
      return "yellow";
    case "done":
      return "green";
    case "failed":
    case "stopped":
      return "red";
    default:
      return "muted";
  }
}

// What needs the owner first, then what's in flight, then queued, then
// whatever already finished either way.
const STATUS_RANK: Record<Task["status"], number> = {
  waiting: 0,
  drafting: 1,
  running: 1,
  landing: 1,
  queued: 2,
  done: 3,
  failed: 3,
  stopped: 3,
};

export function sortTasks(tasks: Task[]): Task[] {
  return [...tasks].sort((a, b) => {
    const byStatus = STATUS_RANK[a.status] - STATUS_RANK[b.status];
    return byStatus !== 0 ? byStatus : b.updatedAt - a.updatedAt;
  });
}

/** The four words the design system colours by (`--tone-*`). */
export type Tone = "ok" | "warning" | "danger" | "info" | "neutral";

/** A rail row's dot and reason colour: needs you amber, went wrong red, in
 * flight blue, finished green. */
export function taskTone(task: Task): Tone {
  switch (task.status) {
    case "waiting":
    case "landing":
      return "warning";
    case "failed":
    case "stopped":
      return "danger";
    case "done":
      return "ok";
    default:
      return "info";
  }
}

function startOfDay(now: number): number {
  const day = new Date(now);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

/** Landed on its base today. orchd keeps no `landedAt`, so the last update
 * of a done task with a landed commit stands in for it. */
export function isLandedToday(task: Task, now = Date.now()): boolean {
  return (
    task.status === "done" &&
    !!task.landedSha &&
    task.updatedAt >= startOfDay(now)
  );
}

export type RailGroups = {
  needsYou: Task[];
  running: Task[];
  landedToday: Task[];
  /** Everything else not archived: finished earlier, done but not landed,
   * stopped by the owner. */
  earlier: Task[];
};

/** The rail's groups, newest update first. Only top-level tasks: a subtask
 * is listed under its parent (`childrenOf`), unless that parent is gone. */
export function railGroups(tasks: Task[], now = Date.now()): RailGroups {
  const live = tasks.filter((t) => !t.archived);
  const ids = new Set(live.map((t) => t.id));
  const groups: RailGroups = {
    needsYou: [],
    running: [],
    landedToday: [],
    earlier: [],
  };
  const top = live
    .filter((t) => !t.parent || !ids.has(t.parent))
    .sort((a, b) => b.updatedAt - a.updatedAt);
  for (const task of top) {
    if (needsOwner(task)) groups.needsYou.push(task);
    else if (
      task.status === "running" ||
      task.status === "drafting" ||
      task.status === "queued"
    )
      groups.running.push(task);
    else if (isLandedToday(task, now)) groups.landedToday.push(task);
    else groups.earlier.push(task);
  }
  return groups;
}

/** Drafts waiting to run: not started, no implement attempt, not parked
 * behind a lease. A parent a planner split for review counts too. */
export function planDrafts(tasks: Task[]): Task[] {
  return tasks.filter(
    (t) =>
      !t.archived &&
      (t.status === "queued" || t.status === "stopped") &&
      !t.queueReason &&
      !t.attempts.some((a) => a.stage === "implement"),
  );
}

/** `2/4`, or a bare `5` once an answer granted attempts past the setting
 * (orchd keeps that raised budget in memory, not on the task). */
export function attemptOf(task: Task, maxAttempts?: number): string {
  const count = implementAttemptCount(task);
  const max = maxAttempts ?? count;
  return count > max ? `${count}` : `${count}/${max}`;
}

const FAILURE_WORDS: Partial<Record<FailureKind, string>> = {
  no_deliverable: "no changes",
  stall: "stalled",
  loop: "looped",
  budget: "over budget",
  verify: "verify failed",
  conflict: "conflict",
  heldout: "held-out check failed",
  review: "review failed",
  evidence: "no evidence",
  protected: "touched a protected path",
  blocked: "blocked",
  error: "error",
};

/** A rail row's second line: why it sits where it does, in its tone. */
export function taskReason(
  task: Task,
  tasks: Task[],
  maxAttempts?: number,
): string {
  const children = childrenOf(tasks, task.id);
  if (children.length > 0 && task.status !== "waiting") {
    const landed = children.filter((c) => c.status === "done").length;
    return `${children.length} subtask${children.length === 1 ? "" : "s"} · ${landed} landed`;
  }
  switch (task.status) {
    case "waiting":
      return questionsLabel(questionCount(task));
    case "landing":
      return "waiting for a clean checkout";
    case "drafting":
      return "drafting the brief";
    case "queued":
      return task.queueReason ? task.queueReason : "queued";
    case "running": {
      const stage = latestAttempt(task)?.stage;
      return `${stage === "plan" ? "brief" : (stage ?? "implement")} · attempt ${attemptOf(task, maxAttempts)}`;
    }
    case "failed": {
      const failure = latestAttempt(task)?.failure;
      const what = failure
        ? (FAILURE_WORDS[failure.kind] ?? failure.kind)
        : "failed";
      return `${what} · ${attemptOf(task, maxAttempts)} attempts`;
    }
    case "stopped":
      return shortReason(task) || "stopped";
    case "done":
      if (task.landedSha)
        return task.baseRef ? `landed on ${task.baseRef}` : "landed";
      return reviewOf(task)
        ? `review ${reviewOf(task)!.verdict} · not landed`
        : "done · not landed";
  }
}

/** A rail row's third, muted line: "attempt 1/4 · 3m 20s · $0.04", each part
 * only when it says something. */
export function taskMetaLine(
  task: Task,
  maxAttempts?: number,
  now = Date.now(),
): string {
  const parts: string[] = [];
  if (
    (task.status === "waiting" || task.status === "queued") &&
    implementAttemptCount(task) > 0
  )
    parts.push(`attempt ${attemptOf(task, maxAttempts)}`);
  if (task.status === "running" || task.status === "drafting") {
    const ms = totalDurationMs(task, now);
    if (ms >= 60_000) parts.push(`${Math.floor(ms / 60_000)}m`);
  }
  parts.push(formatCost(task.costUsd));
  return parts.join(" · ");
}

/** A subtask line under its parent in the rail: dot tone and a short state. */
export function subtaskState(
  task: Task,
  tasks: Task[],
  maxAttempts?: number,
): { tone: Tone; label: string } {
  switch (task.status) {
    case "done":
      return { tone: "ok", label: task.landedSha ? "landed" : "done" };
    case "running":
    case "drafting": {
      const stage = latestAttempt(task)?.stage;
      return {
        tone: "info",
        label: `${stage === "plan" || !stage ? "brief" : stage} · ${attemptOf(task, maxAttempts)}`,
      };
    }
    case "waiting":
      return { tone: "warning", label: "needs you" };
    case "landing":
      return { tone: "warning", label: "landing" };
    case "failed":
      return { tone: "danger", label: "failed" };
    case "stopped":
      return { tone: "danger", label: "stopped" };
    case "queued": {
      const waits = (task.dependsOn ?? []).some((id) => {
        const dep = tasks.find((t) => t.id === id);
        return !!dep && dep.status !== "done";
      });
      return {
        tone: "neutral",
        label: waits || task.queueReason ? "waits" : "queued",
      };
    }
  }
}

export const STAGES = [
  "brief",
  "implement",
  "verify",
  "review",
  "land",
] as const;
export type Stage = (typeof STAGES)[number];
export type StageState = "done" | "active" | "blocked" | "failed" | "pending";
export type StageStep = { stage: Stage; state: StageState };

const QUESTION_STAGE: Record<QuestionKind, Stage> = {
  plan_question: "brief",
  attempts_failing: "verify",
  preexisting_failure: "verify",
  review_no_verdict: "review",
  review_dispute: "review",
  dependency_ended: "implement",
  impossible: "implement",
  budget: "implement",
  daily_budget: "implement",
  protected_path: "implement",
  permission: "implement",
  agent_question: "implement",
};

function failureStage(kind: FailureKind | undefined): Stage {
  if (kind === "verify" || kind === "heldout" || kind === "evidence")
    return "verify";
  if (kind === "review") return "review";
  return "implement";
}

/** Where a running attempt is: a plan run writes the brief, a review run
 * reviews, an implement run implements until its verify has results. */
function runningStage(task: Task): Stage {
  const attempt = latestAttempt(task);
  if (!attempt || attempt.stage === "plan") return "brief";
  if (attempt.stage === "review") return "review";
  return attempt.verify.length > 0 ? "verify" : "implement";
}

/** The brief → implement → verify → review → land track for a task, from its
 * status and attempts: every stage before the current one is done, every one
 * after it pending. */
export function stageTrack(task: Task): StageStep[] {
  let current: Stage;
  let state: StageState;
  switch (task.status) {
    case "drafting":
      current = "brief";
      state = "active";
      break;
    case "queued":
      current = task.attempts.length > 0 ? runningStage(task) : "implement";
      state = "pending";
      break;
    case "running":
      current = runningStage(task);
      state = "active";
      break;
    case "waiting":
      current = task.question
        ? QUESTION_STAGE[task.question.kind]
        : "implement";
      state = "blocked";
      break;
    case "failed":
      current = failureStage(latestAttempt(task)?.failure?.kind);
      state = "failed";
      break;
    case "stopped": {
      const failure = latestAttempt(task)?.failure;
      current = failure ? failureStage(failure.kind) : runningStage(task);
      state = "blocked";
      break;
    }
    case "landing":
      current = "land";
      state = "active";
      break;
    case "done":
      current = "land";
      state = task.landedSha ? "done" : "pending";
      break;
  }
  const at = STAGES.indexOf(current);
  return STAGES.map((stage, index) => ({
    stage,
    state: index < at ? "done" : index === at ? state : "pending",
  }));
}

/** Assistant replies newer than the last time the owner looked at the chat. */
export function unreadChatCount(
  messages: { role: string; ts: number }[],
  lastSeen: number,
): number {
  return messages.filter((m) => m.role === "assistant" && m.ts > lastSeen)
    .length;
}

/** The main process says the daemon binary is absent in one of two ways: a
 * source checkout ("is not built") or a packaged app ("is missing from this
 * installation"). */
export function isDaemonMissing(message: string): boolean {
  return (
    message.includes("is not built") ||
    message.includes("is missing from this installation")
  );
}

/** Whether the missing daemon is a packaged app's broken install (reinstall)
 * rather than an unbuilt source checkout (`npm run build:orchd`). */
export function isPackagedInstall(message: string): boolean {
  return message.includes("is missing from this installation");
}

/** Whether an RPC error means the daemon itself is gone (not built, failed
 * to start, the socket refused or dropped the call), as opposed to the
 * daemon refusing one request. */
export function daemonDown(
  message: string,
): "not-built" | "unavailable" | null {
  if (isDaemonMissing(message)) return "not-built";
  return /failed to start|did not respond|disconnected before responding|ECONNREFUSED|ENOENT|ECONNRESET|EPIPE/.test(
    message,
  )
    ? "unavailable"
    : null;
}

/** The route the orchestrator chat runs on, named by its model: the one text
 * the composer chip and the conversation header both show. */
export function orchestratorRouteLabel(settings: Settings): string {
  const id = settings.orchestrator || settings.tiers.standard;
  const route = settings.routes.find((r) => r.id === id);
  return route?.label || id || "Orchestrator";
}
