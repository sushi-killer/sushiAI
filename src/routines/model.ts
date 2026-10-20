// The Routines page's logic, free of React: filters, status words, the agent
// pickers and the payload of a save. The page only draws what this returns.
import { agentTitle } from "../lib/agent-title.ts";
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
  { id: "claude", label: agentTitle("claude") },
  { id: "codex", label: agentTitle("codex") },
];

/** The levels a harness offers a routine, from the catalog the daemon served.
 * An empty list offers nothing, which the page repairs rather than inventing. */
export const effortsFor = (
  harness: Harness,
  catalog: RoutinesCatalog | null,
): string[] => catalog?.efforts?.[harness] ?? [];

/** The levels a picker shows: the catalog's, or only `current` while the
 * catalog is missing, so the control keeps its value instead of going blank. */
export const effortChoices = (
  harness: Harness,
  catalog: RoutinesCatalog | null,
  current: string,
): string[] => {
  const levels = effortsFor(harness, catalog);
  return levels.length ? levels : [current];
};

export const hostOf = (
  catalog: RoutinesCatalog | null,
  hostId: string,
): CatalogHost | undefined => catalog?.hosts.find((host) => host.id === hostId);

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
  const levels = effortsFor(agent.harness, catalog);
  if (agent.effort && levels.length && !levels.includes(agent.effort))
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

// ---- the builder's questions ----

export type AskOption = { value: string; label: string };
export type AskQuestion = {
  id: string;
  type: "choice" | "multi" | "text";
  label: string;
  options: AskOption[];
  /** Values (or, for text, the one string) selected at the start. */
  default: string[];
  allowOther: boolean;
  placeholder: string;
};
/** A button of the card. The kinds are a fixed whitelist the UI maps to its
 * own handlers; an unknown kind becomes a plain reply with the label. */
export type AskAction = {
  id: string;
  label: string;
  kind: "reply" | "dry-run" | "open-routine";
  reply: string;
  style: "primary" | "secondary";
};
export type Ask = {
  title: string;
  questions: AskQuestion[];
  actions: AskAction[];
};
/** What the owner picked (`picked`) and typed (`other`) for one question. */
export type AskAnswer = { picked: string[]; other: string };
export type AskAnswers = Record<string, AskAnswer>;

const ASK_MAX_JSON = 4000;
const ASK_MAX_QUESTIONS = 4;
const ASK_MAX_ACTIONS = 4;
const ASK_MAX_OPTIONS = 8;
const ASK_MAX_TEXT = 120;
const ASK_BLOCK = /```ask[ \t]*\r?\n([\s\S]*?)\r?\n?```/g;

const askText = (value: unknown, required: boolean): string | null => {
  if (value === undefined && !required) return "";
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (text.length > ASK_MAX_TEXT || (required && !text)) return null;
  return text;
};

function readQuestion(raw: unknown): AskQuestion | null {
  if (!raw || typeof raw !== "object") return null;
  const item = raw as Record<string, unknown>;
  const id = askText(item.id, true);
  const label = askText(item.label, true);
  const placeholder = askText(item.placeholder, false);
  const type = item.type;
  if (id === null || label === null || placeholder === null) return null;
  if (!/^[A-Za-z][\w-]{0,31}$/.test(id)) return null;
  if (type !== "choice" && type !== "multi" && type !== "text") return null;
  if (item.allowOther !== undefined && typeof item.allowOther !== "boolean")
    return null;
  const options: AskOption[] = [];
  if (type !== "text") {
    if (!Array.isArray(item.options)) return null;
    if (item.options.length < 1 || item.options.length > ASK_MAX_OPTIONS)
      return null;
    for (const entry of item.options) {
      const option = entry as Record<string, unknown> | null;
      const value = askText(option?.value, true);
      const text = askText(option?.label, true);
      if (value === null || text === null) return null;
      if (options.some((seen) => seen.value === value)) return null;
      options.push({ value, label: text });
    }
  }
  const raw_default = item.default;
  const wanted =
    typeof raw_default === "string"
      ? [raw_default]
      : Array.isArray(raw_default) &&
          raw_default.every((one) => typeof one === "string")
        ? (raw_default as string[])
        : [];
  const chosen =
    type === "text"
      ? wanted.slice(0, 1).map((one) => one.slice(0, ASK_MAX_TEXT))
      : wanted.filter((one) => options.some((seen) => seen.value === one));
  return {
    id,
    type,
    label,
    options,
    default: type === "choice" ? chosen.slice(0, 1) : chosen,
    allowOther: type !== "text" && item.allowOther === true,
    placeholder,
  };
}

const ASK_KINDS = ["reply", "dry-run", "open-routine"];

function readAction(raw: unknown): AskAction | null {
  if (!raw || typeof raw !== "object") return null;
  const item = raw as Record<string, unknown>;
  const id = askText(item.id, true);
  const label = askText(item.label, true);
  const reply = askText(item.reply, false);
  if (id === null || label === null || reply === null) return null;
  if (!/^[A-Za-z][\w-]{0,31}$/.test(id)) return null;
  const known = typeof item.kind === "string" && ASK_KINDS.includes(item.kind);
  return {
    id,
    label,
    kind: known ? (item.kind as AskAction["kind"]) : "reply",
    reply: known && reply ? reply : label,
    style: item.style === "primary" ? "primary" : "secondary",
  };
}

/** Splits an agent message into its prose and the owner card it asks for.
 * The card is the last ```ask block holding valid JSON of 1-4 questions;
 * anything else (bad JSON, too long, wrong types) gives no card and the whole
 * text back unchanged. Never throws. */
export function parseAsk(text: string): { body: string; ask: Ask | null } {
  const blocks = [...text.matchAll(ASK_BLOCK)];
  const found = blocks[blocks.length - 1];
  const none = { body: text, ask: null };
  if (!found || found[1].length > ASK_MAX_JSON) return none;
  let data: unknown;
  try {
    data = JSON.parse(found[1]);
  } catch {
    return none;
  }
  if (!data || typeof data !== "object") return none;
  const {
    title,
    questions = [],
    actions = [],
  } = data as Record<string, unknown>;
  const heading = askText(title, false);
  if (heading === null) return none;
  if (!Array.isArray(questions) || !Array.isArray(actions)) return none;
  if (questions.length > ASK_MAX_QUESTIONS) return none;
  if (actions.length > ASK_MAX_ACTIONS) return none;
  if (questions.length + actions.length < 1) return none;
  const read = questions.map(readQuestion);
  const buttons = actions.map(readAction);
  if (read.some((one) => one === null)) return none;
  if (buttons.some((one) => one === null)) return none;
  const list = read as AskQuestion[];
  const acts = buttons as AskAction[];
  if (new Set(list.map((one) => one.id)).size !== list.length) return none;
  if (new Set(acts.map((one) => one.id)).size !== acts.length) return none;
  const start = found.index ?? 0;
  const body = (
    text.slice(0, start) + text.slice(start + found[0].length)
  ).trim();
  return { body, ask: { title: heading, questions: list, actions: acts } };
}

/** The answer a question starts with: its defaults. */
export function initialAnswers(ask: Ask): AskAnswers {
  const answers: AskAnswers = {};
  for (const question of ask.questions)
    answers[question.id] =
      question.type === "text"
        ? { picked: [], other: question.default[0] ?? "" }
        : { picked: [...question.default], other: "" };
  return answers;
}

/** The words of one answer: option labels, then the typed text. */
export function answerLabels(
  question: AskQuestion,
  answer: AskAnswer | undefined,
): string[] {
  if (!answer) return [];
  const labels = answer.picked.flatMap((value) => {
    const option = question.options.find((one) => one.value === value);
    return option ? [option.label] : [];
  });
  const typed = answer.other.trim();
  if (typed) labels.push(typed);
  return question.type === "choice" && typed ? [typed] : labels;
}

export const isAnswered = (
  question: AskQuestion,
  answer: AskAnswer | undefined,
) => answerLabels(question, answer).length > 0;

/** The reply turn: one plain `Label: answer` line per question. */
export function askReplyText(ask: Ask, answers: AskAnswers): string {
  return ask.questions
    .filter((question) => isAnswered(question, answers[question.id]))
    .map((question) => {
      // A label that already ends a sentence ("When?") takes no extra colon.
      const label = question.label.replace(/[\s:]+$/, "");
      const colon = /[?!.…]$/.test(label) ? "" : ":";
      return `${label}${colon} ${answerLabels(question, answers[question.id]).join(", ")}`;
    })
    .join("\n");
}

/** A builder message in readable form. The builder should reply in prose, but
 * when a message is (or ends with) the run-result JSON (a `status` with a
 * `title` and/or `summary`), show the title in bold and the summary as normal
 * markdown instead of raw JSON. Any other text, including other JSON, comes
 * back unchanged. Never throws. */
export function readableTurn(text: string): string {
  try {
    const source = text.trim();
    for (
      let at = source.indexOf("{");
      at >= 0;
      at = source.indexOf("{", at + 1)
    ) {
      let data: unknown;
      try {
        data = JSON.parse(source.slice(at).replace(/\s*```\s*$/, ""));
      } catch {
        continue;
      }
      if (!data || typeof data !== "object" || Array.isArray(data)) return text;
      const one = data as Record<string, unknown>;
      const line = (key: string) =>
        typeof one[key] === "string" ? (one[key] as string).trim() : "";
      const title = line("title");
      const summary = line("summary");
      if (typeof one.status !== "string" || (!title && !summary)) return text;
      const lead = source
        .slice(0, at)
        .replace(/```(?:json)?\s*$/, "")
        .trim();
      const extra = ["next", "needs", "needs_you", "missing_tool"]
        .map(line)
        .filter(Boolean);
      return [lead, title && `**${title}**`, summary, ...extra]
        .filter(Boolean)
        .join("\n\n");
    }
  } catch {
    // fall through to the unchanged text
  }
  return text;
}
