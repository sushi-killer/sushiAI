// The Routines page's logic, free of React: filters, status words, the agent
// pickers and the payload of a save. The page only draws what this returns.
import type {
  AccessEntry,
  CatalogHost,
  Harness,
  Routine,
  RoutineEntry,
  RoutinesCatalog,
  RunResult,
  RunStatus,
  RunSummary,
} from "./types.ts";

/** What a routine is doing, in the one word the list shows. */
export type RoutineState =
  "running" | "needs_you" | "failed" | "paused" | "draft" | "ran" | "idle";

/** Failures in a row after which the daemon turns a routine off. */
export const PAUSE_AFTER = 3;

export function routineState(entry: RoutineEntry): RoutineState {
  const { routine, lastRun } = entry;
  if (entry.running || lastRun?.status === "running") return "running";
  if (!routine.enabled && entry.consecutiveFailures >= PAUSE_AFTER)
    return "paused";
  // A dry run is a rehearsal: it never marks the routine as needing the owner.
  const real = lastRun && lastRun.kind !== "dry" ? lastRun : null;
  if (real?.status === "needs_you") return "needs_you";
  if (real?.status === "failed" || real?.status === "missing_tool")
    return "failed";
  if (!routine.enabled) return "draft";
  if (real) return "ran";
  return "idle";
}

/** The three groups of the list: what needs the owner, what runs, what is
 * off. Empty groups are dropped. */
export const GROUP_LABELS = {
  needs: "Needs you",
  active: "Active",
  off: "Off",
} as const;
export type GroupId = keyof typeof GROUP_LABELS;

export function groupEntries(
  entries: RoutineEntry[],
): { id: GroupId; label: string; entries: RoutineEntry[] }[] {
  const place = (entry: RoutineEntry): GroupId => {
    const state = routineState(entry);
    if (state === "needs_you" || state === "failed" || state === "paused")
      return "needs";
    return entry.routine.enabled ? "active" : "off";
  };
  const sorted = sortEntries(entries);
  return (Object.keys(GROUP_LABELS) as GroupId[])
    .map((id) => ({
      id,
      label: GROUP_LABELS[id],
      entries: sorted.filter((entry) => place(entry) === id),
    }))
    .filter((group) => group.entries.length > 0);
}

export type Tone = "ok" | "warning" | "danger" | "info" | "neutral";

export const STATUS_LABELS: Record<RunStatus, string> = {
  running: "Running",
  done: "Done",
  noop: "Nothing to do",
  needs_you: "Needs you",
  failed: "Failed",
  missing_tool: "Missing a tool",
};

export function statusTone(status: RunStatus): Tone {
  switch (status) {
    case "done":
      return "ok";
    case "needs_you":
    case "missing_tool":
      return "warning";
    case "failed":
      return "danger";
    case "running":
      return "info";
    default:
      return "neutral";
  }
}

const sameText = (a: string | undefined, b: string | undefined) =>
  !!a && !!b && a.trim().toLowerCase() === b.trim().toLowerCase();

/** The second line of a list row: why it failed, the last result's line, or
 * when it runs next. It never repeats the routine's name. */
export function rowSubtitle(entry: RoutineEntry, now = Date.now()): string {
  const { lastRun, routine } = entry;
  if (!lastRun)
    return routine.enabled
      ? describeWhen(routine.when.cron, routine.when.tz)
      : "Draft";
  if (
    (lastRun.status === "failed" || lastRun.status === "missing_tool") &&
    lastRun.reason
  )
    return lastRun.reason;
  if (lastRun.title && !sameText(lastRun.title, routine.name))
    return lastRun.title;
  const when = relativeTime(lastRun.endedAt ?? lastRun.startedAt, now);
  return when
    ? `${STATUS_LABELS[lastRun.status]} · ${when}`
    : STATUS_LABELS[lastRun.status];
}

/** The title of a run row. The status pill and a dry-run mark say their own
 * words once: a title that only repeats them is replaced by "Run n". */
export function runRowTitle(run: RunSummary): string {
  const raw = run.title?.trim() ?? "";
  const echo =
    !raw ||
    sameText(raw, STATUS_LABELS[run.status]) ||
    sameText(raw, "dry run");
  const title = echo ? `Run ${run.n}` : raw;
  return run.kind === "dry" && !/dry run/i.test(title)
    ? `${title} · dry run`
    : title;
}

/** The text of the first `# ` heading of a Markdown body, or "". */
export function leadingHeading(markdown: string): string {
  for (const line of markdown.split("\n")) {
    if (!line.trim()) continue;
    const match = /^#\s+(.+?)\s*#*\s*$/.exec(line.trim());
    return match ? match[1] : "";
  }
  return "";
}

/** The first inline code span of a failure text: the command that fixes it. */
export function fixCommand(text: string): string {
  const fenced = /```[^\n]*\n([^`]+?)\n?```/.exec(text);
  const inline = /`([^`\n]+)`/.exec(text);
  return (fenced?.[1] ?? inline?.[1] ?? "").trim();
}

/** The tools a failed run still needs, minus those the fix text names. */
export function missingNotice(
  missing: string[] | undefined,
  fix: string,
): string {
  const rest = (missing ?? []).filter((tool) => !fix.includes(tool));
  return rest.length ? `Needs: ${rest.join(", ")}` : "";
}

/** "0 9 * * 1-5" as words for the common shapes; the raw cron otherwise. */
export function describeWhen(cron: string, tz = ""): string {
  const fields = cron.trim().split(/\s+/);
  const suffix = tz ? ` (${tz})` : "";
  if (fields.length !== 5) return `${cron}${suffix}`;
  const [minute, hour, dom, month, dow] = fields;
  const clock = (h: string, m: string) =>
    `${h.padStart(2, "0")}:${m.padStart(2, "0")}`;
  const plain = /^\d+$/;
  const everyN = /^\*\/(\d+)$/.exec(minute);
  if (everyN && hour === "*" && dom === "*" && month === "*" && dow === "*")
    return `Every ${everyN[1]} min${suffix}`;
  if (plain.test(minute) && plain.test(hour) && dom === "*" && month === "*") {
    const at = clock(hour, minute);
    if (dow === "*") return `Every day at ${at}${suffix}`;
    if (dow === "1-5") return `Weekdays at ${at}${suffix}`;
    if (plain.test(dow)) {
      const days = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
      ];
      return `${days[Number(dow) % 7]}s at ${at}${suffix}`;
    }
  }
  if (
    plain.test(minute) &&
    hour === "*" &&
    dom === "*" &&
    month === "*" &&
    dow === "*"
  )
    return `Every hour at :${minute.padStart(2, "0")}${suffix}`;
  return `${cron}${suffix}`;
}

export function relativeTime(ms: number | undefined, now = Date.now()): string {
  if (!ms) return "";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} h ago`;
  return `${Math.round(hours / 24)} d ago`;
}

// ---- agent pickers ----

export const HARNESSES: { id: Harness; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];

const EFFORTS: Record<Harness, string[]> = {
  claude: ["low", "medium", "high", "xhigh", "max"],
  codex: ["low", "medium", "high"],
};

/** The levels a harness offers: xhigh and max are Claude's alone. */
export const effortsFor = (harness: Harness): string[] => EFFORTS[harness];

export const hostOf = (
  catalog: RoutinesCatalog | null,
  hostId: string,
): CatalogHost | undefined => catalog?.hosts.find((host) => host.id === hostId);

/** One line under a well-known model in the picker; "" for any other. */
export function modelNote(id: string): string {
  if (/opus/i.test(id)) return "Hardest work, slowest";
  if (/sonnet/i.test(id)) return "Most routines";
  if (/haiku/i.test(id)) return "Simple digests, cheapest";
  return "";
}

/** The models a host offers for a harness, and only those. */
export function modelsFor(
  catalog: RoutinesCatalog | null,
  hostId: string,
  harness: Harness,
) {
  return hostOf(catalog, hostId)?.harnesses[harness]?.models ?? [];
}

/** Why a harness cannot run on a host, or "" when it can (login unknown is
 * not a reason). */
export function harnessProblem(
  catalog: RoutinesCatalog | null,
  hostId: string,
  harness: Harness,
): string {
  const host = hostOf(catalog, hostId);
  const entry = host?.harnesses[harness];
  if (!host || !entry) return "";
  const label = HARNESSES.find((item) => item.id === harness)?.label ?? harness;
  if (entry.installed === false)
    return `${label} is not installed on ${host.name}`;
  if (entry.loggedIn === false) return `Log in to ${label} on ${host.name}`;
  return "";
}

/** The agent after the owner changed one field: a model the host does not
 * offer for the harness and a level the harness lacks are dropped. */
export function withAgent(
  routine: Routine,
  patch: Partial<Pick<Routine["agent"], "harness" | "model" | "effort">>,
  catalog: RoutinesCatalog | null,
  hostId: string,
): Routine {
  const agent = { ...routine.agent, ...patch };
  const models = modelsFor(catalog, hostId, agent.harness);
  if (agent.model && !models.some((model) => model.id === agent.model))
    agent.model = "";
  if (agent.effort && !effortsFor(agent.harness).includes(agent.effort))
    agent.effort = "medium";
  return { ...routine, agent };
}

/** The `routines.save` payload. Only the fields the daemon builds argv from
 * come from the pickers; the rest of the routine travels unchanged. */
export function savePayload(routine: Routine, brief: string) {
  const { agent } = routine;
  return {
    routine: {
      ...routine,
      agent: {
        harness: agent.harness,
        ...(agent.model ? { model: agent.model } : {}),
        ...(agent.effort ? { effort: agent.effort } : {}),
        ...(agent.account ? { account: agent.account } : {}),
        extraArgs: agent.extraArgs ?? [],
      },
    },
    brief,
  };
}

/** A blank routine for the builder-less path (R1 "Your own" writes its brief
 * by hand): the default agent, weekdays at 09:00. */
export function newRoutine(
  catalog: RoutinesCatalog | null,
  hostId: string,
  tz: string,
  name: string,
): Routine {
  const agent = catalog?.defaults.agent ?? {
    harness: "claude" as Harness,
    model: "sonnet",
    effort: "medium",
  };
  const id =
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 48) || "routine";
  return withAgent(
    {
      id,
      name,
      where: { host: hostOf(catalog, hostId)?.hostName || "local" },
      when: { cron: "0 9 * * 1-5", tz },
      agent: { ...agent, extraArgs: [] },
      howItRuns: { mode: "ask-first", promoteAfter: 5, accepted: 0 },
      limits: { timeoutMin: 30 },
      enabled: false,
    },
    {},
    catalog,
    hostId,
  );
}

// ---- results ----

export function actionCounts(result: RunResult | null) {
  const actions = result?.actions ?? [];
  const applied = actions.filter((action) => action.applied).length;
  return { applied, wouldDo: actions.length - applied };
}

/** The tools a routine proposed that the owner has not granted yet. The
 * routine's own list wins over the list tagged onto its entry. */
export function ungrantedAccess(
  routine: Routine | null | undefined,
  fallback?: AccessEntry[],
): AccessEntry[] {
  return (routine?.access ?? fallback ?? []).filter((item) => !item.granted);
}

/** One change a run proposes and waits to apply (R5). `index` is the
 * proposal number `routines.approve` takes. */
export type PendingAction = {
  index: number;
  title: string;
  kind: string;
};

export function pendingActions(run: RunSummary | null): PendingAction[] {
  if (!run || run.kind === "dry") return [];
  if (run.proposals?.length)
    return run.proposals.flatMap((item, index) =>
      item.status === "pending"
        ? [
            {
              index,
              kind: item.proposal?.kind ?? "change",
              title:
                item.proposal?.title ||
                item.proposal?.message ||
                item.proposal?.branch ||
                item.proposal?.kind ||
                `Change ${index + 1}`,
            },
          ]
        : [],
    );
  // Only a proposal the module stashed as pending can be approved; an
  // action row in the result is a report, never an approve button.
  return [];
}

const APPROVAL_WORDS: Record<
  string,
  { verb: string; one: string; many: string }
> = {
  branch: { verb: "create", one: "branch", many: "branches" },
  commit: { verb: "make", one: "commit", many: "commits" },
  pr: { verb: "open", one: "pull request", many: "pull requests" },
  task: { verb: "create", one: "task", many: "tasks" },
};

/** The words of the approval card: "wants to create 3 tasks" over the rows
 * and "Create 2 tasks" on the button. */
export function approvalWords(items: PendingAction[], chosen: number) {
  const kinds = new Set(items.map((item) => item.kind));
  const word = kinds.size === 1 ? APPROVAL_WORDS[[...kinds][0]] : undefined;
  const { verb, one, many } = word ?? {
    verb: "make",
    one: "change",
    many: "changes",
  };
  const noun = (n: number) => (n === 1 ? one : many);
  return {
    heading: `wants to ${verb} ${items.length} ${noun(items.length)}`,
    button: `${verb[0].toUpperCase()}${verb.slice(1)} ${chosen} ${noun(chosen)}`,
  };
}

/** Only an http(s) link is drawn as one; the result is agent text. */
export const safeLink = (link: string | undefined): string | null =>
  link && /^https?:\/\//i.test(link) ? link : null;

/** The routine run being shown: the newest run that has a result. */
export function latestFinished(runs: RunSummary[]): RunSummary | null {
  return runs.find((run) => run.status !== "running") ?? null;
}

/** The fix text of a failed run (R6): the daemon's reason, or the result's
 * summary when the agent gave one. */
export function failureText(
  run: RunSummary | null,
  result: RunResult | null,
): string {
  if (run?.reason) return run.reason;
  if (result?.status === "failed" || result?.status === "missing_tool")
    return result.summary;
  return "";
}

/** Entries sorted for the list: what needs the owner first. */
export function sortEntries(entries: RoutineEntry[]): RoutineEntry[] {
  const rank: Record<RoutineState, number> = {
    needs_you: 0,
    failed: 1,
    paused: 2,
    running: 3,
    ran: 4,
    idle: 5,
    draft: 6,
  };
  return [...entries].sort(
    (a, b) =>
      rank[routineState(a)] - rank[routineState(b)] ||
      a.routine.name.localeCompare(b.routine.name),
  );
}

export const entryKey = (entry: { host: string; routine: { id: string } }) =>
  `${entry.host}:${entry.routine.id}`;

// ---- how it runs ----

export type RunMode = "every" | "first" | "own";
export const RUN_MODES: { id: RunMode; label: string }[] = [
  { id: "every", label: "Ask every time" },
  { id: "first", label: "Ask first" },
  { id: "own", label: "On its own" },
];

/** The daemon's three modes. An older daemon knows two: its `ask-first`
 * that never promotes (`promoteAfter` 0) is "ask every time". */
export function runModeOf(routine: Routine): RunMode {
  const { mode, promoteAfter } = routine.howItRuns;
  if (mode === "on-its-own") return "own";
  if (mode === "ask-every-time") return "every";
  return promoteAfter === 0 ? "every" : "first";
}
export function withRunMode(routine: Routine, mode: RunMode): Routine {
  const keep = routine.howItRuns.promoteAfter || 5;
  const next =
    mode === "own"
      ? ("on-its-own" as const)
      : mode === "every"
        ? ("ask-every-time" as const)
        : ("ask-first" as const);
  return {
    ...routine,
    howItRuns: { ...routine.howItRuns, mode: next, promoteAfter: keep },
  };
}
export function runModeHelp(routine: Routine): string {
  switch (runModeOf(routine)) {
    case "every":
      return "Every run waits for your OK. Change it any time.";
    case "first":
      return `Asks before a write; on its own after ${routine.howItRuns.promoteAfter} OKs.`;
    default:
      return "Never asks.";
  }
}

// ---- brief rows ----

/** The Read, Do and Result rows of "How it works", taken from the brief the
 * builder wrote: the first step, the goal and the result section. */
export function briefRows(brief: string): {
  read: string;
  doing: string;
  result: string;
} {
  const sections: Record<string, string[]> = {};
  let current = "";
  for (const line of brief.split("\n")) {
    const heading = /^##\s+(.+?)\s*$/.exec(line);
    if (heading) {
      current = heading[1].toLowerCase();
      sections[current] = [];
    } else if (current) sections[current].push(line);
  }
  const first = (name: string) =>
    (sections[name] ?? [])
      .map((line) =>
        line
          .replace(/^\s*(?:\d+[.)]|[-*])\s+/, "")
          .replace(/[*_`]/g, "")
          .trim(),
      )
      .find(Boolean) ?? "";
  return {
    read: first("steps"),
    doing: first("goal"),
    result: first("result"),
  };
}

/** "Fri 17 Oct" of a run's start. */
export function runDate(ms: number | undefined): string {
  if (!ms) return "";
  return new Date(ms).toLocaleDateString("en-GB", {
    weekday: "short",
    day: "numeric",
    month: "short",
  });
}

/** "Fri 17 Oct, 17:00": the heading date of one run. */
export function runDateTime(ms: number | undefined): string {
  if (!ms) return "";
  const time = new Date(ms).toLocaleTimeString("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
  });
  return `${runDate(ms)}, ${time}`;
}

/** "53 s", "1 min 12 s": how long a run took. */
export function durationText(run: {
  startedAt?: number;
  endedAt?: number;
}): string {
  if (!run.startedAt || !run.endedAt) return "";
  const total = Math.max(0, Math.round((run.endedAt - run.startedAt) / 1000));
  if (total < 60) return `${total} s`;
  return `${Math.floor(total / 60)} min ${total % 60} s`;
}
