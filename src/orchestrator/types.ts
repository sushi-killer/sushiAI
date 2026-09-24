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
/** Pushed over `onOrchestrator` from the daemon's one `subscribe` connection. */
export type OrchestratorEvent = TaskEvent | LogEvent | ChatEvent | MessageEvent;
