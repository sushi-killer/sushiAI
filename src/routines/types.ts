// Mirrors the daemon's `routines.*` JSON-RPC shapes
// (crates/sushiai-routines): two independent renderings of one contract.

export type Harness = "claude" | "codex";
export type Effort = "low" | "medium" | "high" | "xhigh" | "max";

export type RoutineAgent = {
  harness: Harness;
  model?: string;
  effort?: string;
  account?: string;
  extraArgs: string[];
};

export type Routine = {
  id: string;
  name: string;
  where: { host: string; project?: string };
  when: { cron: string; tz: string };
  agent: RoutineAgent;
  howItRuns: {
    mode: "ask-every-time" | "ask-first" | "on-its-own";
    promoteAfter: number;
    accepted: number;
  };
  limits: { timeoutMin: number };
  /** The tools the owner granted this routine, and those it still waits for.
   * Absent on a daemon that does not know grants. */
  access?: AccessEntry[];
  enabled: boolean;
};

/** One tool a routine proposes to use; `granted` is the owner's decision. */
export type AccessEntry = {
  tool: string;
  label: string;
  detail?: string | null;
  kind: "read" | "write" | string;
  granted: boolean;
};

export type RunKind = "scheduled" | "dry" | "setup";
export type RunStatus =
  "running" | "done" | "noop" | "needs_you" | "failed" | "missing_tool";

export type RunSummary = {
  n: number;
  kind: RunKind;
  status: RunStatus;
  session?: string;
  startedAt?: number;
  endedAt?: number;
  reason?: string | null;
  title?: string;
  caughtUp?: boolean;
  agent?: { harness: string; model?: string | null; effort?: string | null };
  /** A failed run that asks for a login: the host and the harness to log in. */
  loginHost?: string;
  loginHarness?: Harness;
  /** What the run proposed to change, one entry per action. `pending` waits
   * for the owner's OK (`routines.approve`, `proposal` is the index). */
  proposals?: RunProposal[];
  pendingApprovals?: number;
};

export type RunProposal = {
  status: "pending" | "applied" | "would_do" | "failed" | "discarded" | string;
  applied?: boolean;
  detail?: string;
  proposal?: {
    kind?: string;
    title?: string;
    message?: string;
    branch?: string;
    [key: string]: unknown;
  };
};

/** One routine of `routines.list`, tagged by the desktop with its host. */
export type RoutineEntry = {
  routine: Routine;
  lastRun: RunSummary | null;
  running: boolean;
  consecutiveFailures: number;
  /** "local" or "ssh:<profile id>". */
  host: string;
  hostName: string;
  /** The routine's access list and how many of it still wait for the owner. */
  access?: AccessEntry[];
  needsAccess?: number;
};

export type ResultAction = {
  kind: string;
  text: string;
  link?: string;
  applied: boolean;
};

/** `runs/<n>/result.json`, schema v1. */
export type RunResult = {
  status: RunStatus;
  title: string;
  summary: string;
  actions: ResultAction[];
  missing?: string[];
  next?: string;
  counts?: Record<string, number>;
};

export type RoutineDetail = {
  routine: Routine;
  brief: string;
  hash: string;
  access?: AccessEntry[];
  needsAccess?: number;
  runs: RunSummary[];
  consecutiveFailures: number;
  latest: RunResult | null;
};

export type CatalogModel = { id: string; label: string };
export type CatalogHarness = {
  installed: boolean | null;
  loggedIn: boolean | null;
  models: CatalogModel[];
};
export type CatalogHost = {
  id: string;
  name: string;
  state: string;
  /** The host's daemon serves the module. */
  routines: boolean;
  hostName: string;
  harnesses: Record<Harness, CatalogHarness>;
};
export type CatalogProject = {
  id: string;
  name: string;
  purpose: string;
  remote: string;
  folders: { host: string; path: string }[];
};
export type RoutinesCatalog = {
  hosts: CatalogHost[];
  projects: CatalogProject[];
  defaults: { agent: { harness: Harness; model: string; effort: string } };
};

export type BuilderSettings = {
  harness: Harness;
  model: string;
  effort: string;
  /** "local" or "ssh:<profile id>". */
  host: string;
  accountId: string;
  /** What the builder may read; both on when absent. */
  sees?: { projects: boolean; connections: boolean };
  /** How its tools reach the harness; the CLI when absent. */
  access?: "cli" | "mcp" | "both";
};

/** `routines.event`. `inbox` is true only for what needs the owner. */
export type RoutinesEvent = {
  kind: "changed" | "run_started" | "run_finished" | "skipped" | "paused";
  routineId: string;
  name?: string;
  run?: number;
  status?: string;
  title?: string;
  dry?: boolean;
  reason?: string | null;
  failures?: number;
  inbox: boolean;
};

export type ValidationReport = {
  ok: boolean;
  errors: { field: string; message: string }[];
  warnings: { field: string; message: string }[];
  hash: string;
};
