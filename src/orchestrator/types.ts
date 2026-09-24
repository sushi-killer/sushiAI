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
  /** Route id that self-answers a blocked question when Jev is confident
   * it's answerable from the repo, before escalating to the owner; ""
   * turns this off - same semantics as `planner`. Logged in a task's
   * `decisions` as `"Orchestrator: ..."`, distinct from Jev's own `"Jev:
   * ..."` lines. */
  orchestrator: string;
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

export type FailureKind =
  "no_deliverable" | "verify" | "review" | "protected" | "blocked" | "error";

export type Failure = {
  kind: FailureKind;
  detail: string;
  signature: string;
};

export type Usage = { input: number; output: number; cached: number };

export type AttemptStatus =
  "running" | "passed" | "failed" | "interrupted" | "blocked";

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
  changedFiles: string[];
  verify: VerifyResult[];
  gateBlocks: number;
  review?: ReviewResult;
  failure?: Failure;
  usage?: Usage;
  costUsd?: number;
};

export type Task = {
  id: string;
  title: string;
  goal: string;
  criteria: string[];
  verify: string[];
  /** The one-sentence ask a plan was drafted from, when the task started
   * that way instead of from the full manual form. */
  request?: string;
  repo: string;
  worktree: string;
  branch: string;
  baseSha: string;
  status: TaskStatus;
  tier: Tier;
  question?: Question;
  /** Owner and classifier decisions, newest last - includes "Owner: ..."
   * answers to a question. */
  decisions: string[];
  attempts: Attempt[];
  costUsd: number;
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
/** The orchestrator agent's conversation for one repo, kept by the daemon. */
export type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  ts: number;
};
export type ChatThread = {
  repo: string;
  messages: ChatMessage[];
  busy: boolean;
  note?: string;
  error?: string;
};
export type ChatEvent = { event: "chat"; thread: ChatThread };
/** Pushed over `onOrchestrator` from the daemon's one `subscribe` connection. */
export type OrchestratorEvent = TaskEvent | LogEvent | ChatEvent;
