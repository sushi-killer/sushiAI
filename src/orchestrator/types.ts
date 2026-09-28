// Mirrors artifacts/tasks/orchestrator-mvp.md's "Types (JSON, camelCase)"
// section exactly - the daemon (orchd, Lane A) and this file are two
// independent renderings of the same contract, not one importing the other.

export type Harness = "claude" | "codex";

export type Route = {
  id: string;
  label: string;
  harness: Harness;
  model?: string;
  effort?: string;
  /** Claude only: routes through a custom model provider instead of the
   * Anthropic API. */
  profileId?: string;
};

export type Tier = "mechanical" | "standard" | "hard";

export type ClassifierBackend = "none" | "openrouter" | "typesafe" | "openai";

export type Settings = {
  routes: Route[];
  tiers: Record<Tier, string>;
  /** Route id; "" turns review off, "auto" picks the first route whose
   * harness differs from the implementing attempt's. */
  review: string;
  classifier: {
    backend: ClassifierBackend;
    /** Model id only - the endpoint is always the selected provider's own
     * `baseUrl` (`openai` backend included: it has no base URL of its own). */
    model: string;
    providerId: string;
  };
  sandbox: "native" | "host";
  /** Claude's sandbox only: a per-domain allowlist. */
  allowedDomains: string[];
  /** Codex has no per-domain filter - network is all-or-nothing. */
  codexNetwork: boolean;
  protectedPaths: string[];
  maxAttempts: number;
  parallel: number;
  /** Route id that drafts a plan (title/goal/criteria/verify) from a bare
   * request; "" turns drafting off, so `task.create` always needs the full
   * form instead. */
  planner: string;
  /** Route the orchestrator agent runs on (its chat and, with `autoAnswer`,
   * its answers to stuck questions); "" means the standard tier's route. */
  orchestrator: string;
  /** The orchestrator answers a stuck agent's question before it reaches
   * the owner, logged in the task as `"Orchestrator: ..."`. */
  autoAnswer: boolean;
  /** Experiment flags new tasks start with; `task.create` can override them. */
  experiments: Variant;
  /** Model id -> per-million-token prices, for harnesses (Codex) that
   * report tokens but no cost. */
  prices: Record<
    string,
    {
      input: number;
      cachedInput: number;
      output: number;
      /** Cache writes; absent = billed at the input price. */
      cacheWrite?: number;
    }
  >; /** Repository-specific path fragments for `orchd ab`'s work breakdown. */
  workBuckets?: { process: string[]; evidence: string[] };
};

export type TaskStatus =
  "drafting" | "queued" | "running" | "waiting" | "done" | "stopped" | "failed";

export type Question = { text: string; options: string[] };

export type VerifyResult = {
  command: string;
  code: number | null;
  tail: string;
  ms: number;
};

export type ReviewResult = { verdict: "PASS" | "FAIL"; findings: string[] };

export type Variant = {
  retryMode: "resume" | "fresh";
  stallTimeoutSecs: number;
  plannerTier: boolean;
  contract: boolean;
  reviewOtherFamily: boolean;
  reviewEvidence: boolean;
  deferHeavyChecks: boolean;
  leanOutput: boolean;
  reviewBlind: boolean;
  advisor: boolean;
  loopDetect: boolean;
  /** Route id the plan stage runs on instead of `settings.planner`. */
  plannerRoute?: string;
  /** Route ids the implement stage uses instead of `settings.tiers`. */
  tierRoutes?: Partial<Record<Tier, string>>;
  /** Dollar budget: once the task has spent it, the task waits for the
   * owner ("raise" adds it again, "stop") before its next run. Absent or
   * 0 = none. */
  maxCostUsd?: number;
  /** Dollar cap on one implement attempt: past it the run is stopped and
   * fails with kind "budget". Absent or 0 = none. */
  maxAttemptCostUsd?: number;
};

export type FailureKind =
  | "no_deliverable"
  | "stall"
  | "loop"
  | "budget"
  | "verify"
  | "review"
  | "protected"
  | "blocked"
  | "error";

export type Failure = {
  kind: FailureKind;
  detail: string;
  signature: string;
};

export type Usage = { input: number; output: number; cached: number };

export type AttemptStatus =
  "running" | "passed" | "failed" | "interrupted" | "blocked";

/** What a run actually used, as the harness reported it; `model` on the
 * attempt is only the alias orchd asked for. */
export type Fingerprint = {
  models: string[];
  harness: Harness;
  harnessVersion?: string;
  promptHash: string;
};

export type Attempt = {
  n: number;
  stage: "plan" | "implement" | "review";
  routeId: string;
  harness: Harness;
  model: string;
  /** Why this route was chosen - a rule name, or a classifier probability. */
  reason: string;
  sessionId?: string;
  resumed: boolean;
  startedAt: number;
  endedAt?: number;
  status: AttemptStatus;
  summary?: string;
  /** The agent's note for the next attempt: done, tried, next. */
  handoff?: string;
  changedFiles: string[];
  verify: VerifyResult[];
  gateBlocks: number;
  /** First-turn prompt tokens of a fresh Claude implement attempt. */
  prefixTokens?: number;
  review?: ReviewResult;
  failure?: Failure;
  usage?: Usage;
  costUsd?: number;
  /** `costUsd` was priced from the run's per-message token usage because it
   * ended (stopped, stalled, daemon restart) before the CLI reported one. */
  costEstimated?: boolean;
  /** Cost of the review run(s) that reviewed this implement attempt - kept
   * separate from `costUsd` because that field is what a later resume of
   * the same session subtracts, and a review's cost must never join it. */
  reviewCostUsd?: number;
  /** The advisor's diagnosis of this attempt's failure, shown to the next
   * attempt. */
  advice?: string;
  /** Cost of the advisor run about this attempt's failure (also in the
   * task's `costUsd`); once set, the attempt is not advised again. */
  advisorCostUsd?: number;
  /** Absent on an attempt from before fingerprints. */
  fingerprint?: Fingerprint;
  reviewFingerprint?: Fingerprint;
  advisorFingerprint?: Fingerprint;
};

export type Task = {
  id: string;
  title: string;
  goal: string;
  criteria: string[];
  verify: string[];
  /** Slow checks run once, after review passes and before the commit. */
  finalVerify?: string[];
  /** The one-sentence ask a plan was drafted from, when the task started
   * that way instead of from the full manual form. */
  request?: string;
  repo: string;
  worktree: string;
  branch: string;
  baseSha: string;
  /** Branch the task started from; its work is carried onto it when it moves. */
  baseRef?: string;
  /** Ids of tasks that must be done before this one starts implementing. */
  dependsOn?: string[];
  /** The task this one is a part of: it branches from and lands on that
   * task's branch. A task with children runs no attempt of its own. */
  parent?: string;
  status: TaskStatus;
  tier: Tier;
  /** The planner's tier; routes the task only with `variant.plannerTier`. */
  plannedTier?: Tier;
  /** Set when `tier` was picked by falling back to `standard` instead of a
   * classified or planner choice; the fixed-set reason (e.g. "no classifier
   * key"). Absent when Jev or the planner picked the tier. */
  tierFallback?: string;
  /** Experiment flags this task runs with (an A/B arm); absent on tasks
   * created before variants existed. */
  variant?: Variant;
  /** Set on tasks created by `orchd eval run`: the eval set (file stem) and
   * the task's name in it. */
  evalSet?: string;
  evalName?: string;
  question?: Question;
  /** Owner and classifier decisions, newest last - includes "Owner: ..."
   * answers to a question. */
  decisions: string[];
  attempts: Attempt[];
  costUsd: number;
  /** Times the owner raised the `variant.maxCostUsd` budget; the budget in
   * force is that amount times `1 + budgetRaises`. */
  budgetRaises?: number;
  /** Hides the task from the default task list without deleting it. */
  archived: boolean;
  createdAt: number;
  updatedAt: number;
};

export type TaskEvent = { event: "task"; task: Task };
export type LogEvent = {
  event: "log";
  taskId: string;
  attempt: number;
  line: string;
};
/** One of the orchestrator agent's chat sessions for a repo, kept by the
 * daemon. `chat.get`/`chat.send` act on the repo's current session. */
export type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  ts: number;
};
export type ChatThread = {
  repo: string;
  id: string;
  /** Set from the session's first owner message; absent until then. */
  title?: string;
  messages: ChatMessage[];
  busy: boolean;
  note?: string;
  error?: string;
};
/** A session as the session list shows it - no message bodies. */
export type ChatSessionSummary = { id: string; title?: string; busy: boolean };
export type ChatSessionList = {
  current: string;
  sessions: ChatSessionSummary[];
};
/** `thread` is the current session, whole; `current`/`sessions` are what
 * `chat.list` would return at the same moment. */
export type ChatEvent = { event: "chat"; thread: ChatThread } & ChatSessionList;

/** One agent-to-agent (or agent-to-orchestrator) message, kept by the daemon
 * per repo. `from`/`to` are either a task id or the literal `"orchestrator"`.
 * `delivered` flips true once the recipient actually receives it on its next
 * attempt/turn - until then the message is only pending. */
export type Message = {
  id: string;
  repo: string;
  from: string;
  to: string;
  kind: "message" | "question" | "reply";
  text: string;
  /** Id of the message this one answers - only set when `kind` is "reply". */
  replyTo?: string;
  ts: number;
  delivered: boolean;
  deliveredAt?: number;
};
/** Pushed both when a message is created and again when it flips to
 * delivered - an upsert by id, same as `TaskEvent`. */
export type MessageEvent = { event: "message"; message: Message };

export type AuditStatus = "running" | "done" | "failed" | "stopped";
export type AuditGrade = "good" | "weak" | "missing";
export type AuditEffort = "small" | "medium" | "large";
export type AuditItem = {
  area: string;
  grade: AuditGrade;
  /** `path:line` references, commands run, or `unmeasured: ...`. */
  evidence: string[];
  recommendation: string;
  effort: AuditEffort;
};
/** The agent's parsed ```sushi-audit reply, stored as `report.json`. */
export type AuditReport = {
  summary: string;
  items: AuditItem[];
  /** At most 5, most valuable for autonomous agents first. */
  topFixes: string[];
};
/** One read-only `repo.audit` run, as `repo.audit.get`/`repo.audit.list`
 * return it. `report` is null until the run is done, and stays null when
 * the reply held no parsable report (`error` says why). */
export type Audit = {
  id: string;
  repo: string;
  routeId: string;
  harness: Harness;
  model: string;
  status: AuditStatus;
  startedAt: number;
  endedAt?: number;
  error?: string;
  usage?: Usage;
  costUsd: number;
  /** Absent on an audit from before fingerprints. */
  fingerprint?: Fingerprint;
  report: AuditReport | null;
};
/** Pushed when an audit starts and when it ends; its progress lines are
 * `LogEvent`s whose `taskId` is the audit id (`attempt` 0). */
export type AuditEvent = { event: "audit"; audit: Audit };
/** Pushed over `onOrchestrator` from the daemon's one `subscribe` connection. */
export type OrchestratorEvent =
  TaskEvent | LogEvent | ChatEvent | MessageEvent | AuditEvent;

export type TimelineStage =
  "plan" | "implement" | "verify" | "review" | "advisor" | "final" | "wait";

/** One piece of a task's time, from `task.timeline`: computed on demand from
 * the task record and each run's events, never stored. */
export type TimelineSegment = {
  stage: TimelineStage;
  attempt: number;
  startedAt: number;
  endedAt: number;
  costUsd: number;
  outcome: string;
  failureKind?: FailureKind;
  /** Implement attempts only: tool calls by kind. */
  buckets?: {
    process: number;
    evidence: number;
    verify: number;
    task: number;
    explore: number;
  };
};

/** One recurring failure from `failures.catalogue`: every attempt whose
 * failure has this signature, across tasks. */
export type FailureRow = {
  signature: string;
  kind: FailureKind;
  count: number;
  tasks: { id: string; title: string }[];
  lastSeen: number;
  exampleDetail: string;
  /** The task whose attempt supplied `exampleDetail` and `lastSeen`. */
  exampleTaskId: string;
};
