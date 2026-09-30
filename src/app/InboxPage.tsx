import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowUpRight,
  ChevronDown,
  ChevronRight,
  GitBranch,
  ListChecks,
  RefreshCw,
} from "lucide-react";
import {
  ExtensionNavSlot,
  ExtensionSectionSlot,
} from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import type { InboxGroup, InboxRow } from "./attention.ts";
import { LOCAL_GROUP, groupKey, groupLabel } from "./workspaceMerge.ts";
import {
  KINDS,
  ageLabel,
  agentName,
  cleanupCandidates,
  diffFacts,
  inScope,
  inboxEnterAnswer,
  inboxItems,
  landTargets,
  needsYou,
  plural,
  scopedHeadline,
  zeroLine,
  type Item,
  type Kind,
  type SessionItem,
  type TaskItem,
} from "./inboxModel.ts";
import {
  parseSessionPrompt,
  replySteps,
  type SessionPrompt,
} from "./sessionPrompt.ts";
import { Icon } from "../PanelIcon.tsx";
import { Character } from "../mascot/Character.tsx";
import { orchestratorClientFor } from "../orchestrator/client.ts";
import { hostOf } from "../orchestrator/hosts.ts";
import {
  criteriaMet,
  errorText,
  formatCost,
  implementAttemptCount,
  latestImplementAttempt,
  reviewOf,
  stageTrack,
  taskReason,
} from "../orchestrator/helpers.ts";
import { shortBranch } from "../orchestrator/taskDetailModel.ts";
import {
  clickAnswer,
  ownerTarget,
  shownPick,
  stepSelection,
  togglePick,
  type AnswerChoice,
} from "../orchestrator/ownerAttention.ts";
import type { TaskTarget } from "../orchestrator/notices.ts";
import type { Task } from "../orchestrator/types.ts";
import {
  AttentionItem,
  Chip,
  Criterion,
  GroupLabel,
  StageTrack,
} from "../orchestrator/ui/index.ts";
import type { ConnectionProfile, Workspace } from "../types";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import "./inbox.css";

type Filter = "all" | "answer" | "decide" | "land" | "panels";

const LABELS: Record<Kind, string> = {
  answer: "ANSWER",
  decide: "DECIDE",
  land: "LAND & REVIEW",
  panels: "PANELS",
  working: "WORKING",
  idle: "IDLE",
};
const CHIPS: { filter: Filter; label: string }[] = [
  { filter: "all", label: "All" },
  { filter: "answer", label: "Answer" },
  { filter: "decide", label: "Decide" },
  { filter: "land", label: "Land" },
  { filter: "panels", label: "Panels" },
];
const TONE = {
  answer: "warning",
  decide: "danger",
  land: "ok",
  panels: "info",
} as const;
/** How often a blocked session's screen is re-read while it waits. */
const SCREEN_REFRESH_MS = 2500;

/** orchd's `diffStat`: the task's branch against its base, absent until
 * its first implement attempt. */
type DiffStat = { files: number; added: number; removed: number };
const diffOf = (task: Task): DiffStat | undefined =>
  (task as Task & { diffStat?: DiffStat }).diffStat;

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** A Herdr pane the Inbox can read and type into. */
const answerable = (row: InboxRow) =>
  !!row.panel.herdrId && !!row.workspace.connection && !row.panel.ended;

/** Whether a global key belongs to something else: an open dialog, a text
 * field, or - for anything but J/K - a focused button or link, where Enter is
 * that button's own click, not a second answer. */
function ownsKey(target: EventTarget | null, key: string): boolean {
  if (document.querySelector("[role=dialog], dialog[open]")) return true;
  const el = target as HTMLElement | null;
  if (!el?.closest) return false;
  if (el.isContentEditable || el.closest("input, textarea, select"))
    return true;
  return key !== "j" && key !== "k" && !!el.closest("button, a");
}

/** The visible text of every blocked session, re-read while it waits. */
function usePaneScreens(rows: InboxRow[]) {
  const [screens, setScreens] = useState<Record<string, string>>({});
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const read = useCallback(async (row: InboxRow) => {
    const result = await window.bridge?.herdr(
      row.workspace.connection!,
      "pane.read",
      {
        pane_id: row.panel.herdrId,
        source: "visible",
        format: "text",
        strip_ansi: true,
      },
    );
    const text: unknown = result?.read?.text;
    if (typeof text !== "string") return;
    setScreens((old) =>
      old[row.panel.id] === text ? old : { ...old, [row.panel.id]: text },
    );
  }, []);
  const signature = rows.map((row) => row.panel.id).join("|");
  useEffect(() => {
    if (!signature) return;
    const tick = () => {
      for (const row of rowsRef.current) void read(row).catch(() => {});
    };
    tick();
    const timer = setInterval(tick, SCREEN_REFRESH_MS);
    return () => clearInterval(timer);
  }, [signature, read]);
  return { screens, read };
}

/** Everything that needs you, across every project and host: agent sessions
 * waiting on an answer or finished unseen, orchestrator questions and
 * decisions, finished work waiting to land - then what is still working or
 * idle. One queue on the left, the selected item on the right. */
export function InboxPage({
  groups,
  markSeen,
  ownerTasks,
  allTasks,
  openOrchestratorTask,
  switchWorkspace,
  ws,
  connectionProfiles,
  registry,
  openExtensionTarget,
  cwd,
  connection,
  workspaces,
}: {
  groups: InboxGroup[];
  markSeen(panelId: string): void;
  ownerTasks: Task[];
  allTasks: Task[];
  openOrchestratorTask(target: TaskTarget): void;
  switchWorkspace(id: string): void;
  ws: WorkspaceController;
  connectionProfiles: ConnectionProfile[];
  registry: ExtensionRegistry;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
  cwd: string;
  connection?: string;
  workspaces: Workspace[];
}) {
  const [query, setQuery] = useState(""),
    [project, setProject] = useState(""),
    [hostFilter, setHostFilter] = useState(""),
    [filter, setFilter] = useState<Filter>("all"),
    [selectedKey, setSelectedKey] = useState<string | null>(null),
    [picks, setPicks] = useState<Record<string, string>>({}),
    [notes, setNotes] = useState<Record<string, string>>({}),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [landAll, setLandAll] = useState(false),
    [cleanup, setCleanup] = useState<Set<string> | null>(null),
    [fixing, setFixing] = useState<string | null>(null),
    [fixNote, setFixNote] = useState(""),
    [idleOpen, setIdleOpen] = useState(false);
  const inFlight = useRef(false);

  const items = useMemo(
    () => inboxItems(ownerTasks, allTasks, groups),
    [ownerTasks, allTasks, groups],
  );
  const blockedRows = useMemo(
    () =>
      items.flatMap((item) =>
        item.source === "session" &&
        item.kind === "answer" &&
        answerable(item.row)
          ? [item.row]
          : [],
      ),
    [items],
  );
  const { screens, read } = usePaneScreens(blockedRows);
  const prompts = useMemo(() => {
    const out: Record<string, SessionPrompt> = {};
    for (const row of blockedRows) {
      const screen = screens[row.panel.id];
      if (screen != null)
        out[row.panel.id] = parseSessionPrompt(screen, row.panel.agent);
    }
    return out;
  }, [blockedRows, screens]);

  const projects = useMemo(
    () => [...new Set(items.map((item) => item.project))].sort(),
    [items],
  );
  const hosts = useMemo(() => {
    const seen = new Map<string, string>();
    const keys = [
      ...items.map((item) => item.host),
      ...(groups.find((group) => group.key === "shells")?.rows ?? []).map(
        (row) => groupKey(row.workspace.connection),
      ),
    ];
    for (const key of keys)
      if (!seen.has(key)) seen.set(key, groupLabel(key, connectionProfiles));
    return [...seen.entries()].sort(([a], [b]) =>
      a === LOCAL_GROUP ? -1 : b === LOCAL_GROUP ? 1 : a.localeCompare(b),
    );
  }, [items, groups, connectionProfiles]);

  const scope = { project, host: hostFilter, query };
  const scoped = items.filter((item) => inScope(item, scope));
  const counts = (value: Filter) =>
    scoped.filter((item) =>
      value === "all" ? needsYou(item) : item.kind === value,
    ).length;
  const visible = scoped.filter(
    (item) => filter === "all" || item.kind === filter,
  );
  const keys = visible
    .filter((item) => item.kind !== "idle" || idleOpen)
    .map((item) => item.key);
  const keysSignature = keys.join("\n");
  // The selection is pinned: a newly arrived item never takes it over; only
  // when the selected item leaves does it move to the first one.
  useEffect(() => {
    if (selectedKey == null || !keys.includes(selectedKey))
      setSelectedKey(keys[0] ?? null);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keysSignature is keys by value
  }, [keysSignature, selectedKey]);
  const selected =
    visible.find((item) => item.key === selectedKey) ??
    visible.find((item) => item.key === keys[0]) ??
    null;
  // When the selected item last changed (or the page opened): Enter is
  // ignored for a moment after, so a held or doubled Enter never answers the
  // item the selection moved to.
  const selectedAt = useRef(0);
  const shownKey = selected?.key ?? null;
  useEffect(() => {
    selectedAt.current = Date.now();
  }, [shownKey]);
  /** When the owner last picked an option on each item, by digit or click. */
  const pickedAt = useRef<Record<string, number>>({});
  const pickOption = (item: TaskItem, option: string) => {
    pickedAt.current[item.key] = Date.now();
    setPicks((old) => ({ ...old, [item.key]: option }));
  };

  /** The selected task's host's attempt cap, for "attempt 1/4". */
  const [maxAttempts, setMaxAttempts] = useState<Record<string, number>>({});
  const selectedHost =
    selected?.source === "task" ? hostOf(selected.task) : undefined;
  const selectedHostKey = selected?.source === "task" ? selected.host : null;
  useEffect(() => {
    if (selectedHostKey == null || maxAttempts[selectedHostKey] != null) return;
    let live = true;
    orchestratorClientFor(selectedHost)
      .settingsGet()
      .then((settings) => {
        if (live)
          setMaxAttempts((old) => ({
            ...old,
            [selectedHostKey]: settings.maxAttempts,
          }));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [selectedHostKey, selectedHost, maxAttempts]);

  const needs = items.filter(needsYou);
  const headline = scopedHeadline(
    scoped.filter(needsYou),
    !!(project || hostFilter || query.trim()),
    Date.now(),
  );
  const candidates = cleanupCandidates(groups, {
    project,
    host: hostFilter,
  });

  async function act(run: () => Promise<unknown>) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError("");
    try {
      await run();
    } catch (cause) {
      setError(errorText(cause));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }
  const openTask = (task: Task) => openOrchestratorTask(ownerTarget(task));
  /** A task is acted on through the daemon of the host it runs on. */
  const clientOf = (task: Task) => orchestratorClientFor(hostOf(task));
  const noteOf = (item: Item) => notes[item.key] ?? "";
  const setNote = (item: Item, text: string) =>
    setNotes((old) => ({ ...old, [item.key]: text }));
  /** The first option shows picked until the owner picks or unpicks one;
   * only a click on Answer takes that preselection, Enter never does. */
  const choiceOf = (item: TaskItem): AnswerChoice => ({
    pick: picks[item.key],
    preselected: item.task.question?.options[0] ?? "",
    note: noteOf(item),
  });
  const answerTask = (item: TaskItem, text: string) => {
    if (!text) return;
    void act(async () => {
      await clientOf(item.task).taskAnswer(item.task.id, text);
      const drop = (old: Record<string, string>) => {
        const next = { ...old };
        delete next[item.key];
        return next;
      };
      setNotes(drop);
      setPicks(drop);
      delete pickedAt.current[item.key];
    });
  };
  const answerByKey = (item: TaskItem) =>
    answerTask(
      item,
      inboxEnterAnswer(
        choiceOf(item),
        selectedAt.current,
        pickedAt.current[item.key],
        Date.now(),
      ),
    );
  /** Types into the session, one step at a time, then re-reads its screen. */
  const send = (item: SessionItem, steps: string[], clearNote = false) =>
    void act(async () => {
      const { row } = item;
      for (const [index, raw] of steps.entries()) {
        if (index > 0) await sleep(150);
        await window.bridge!.herdr(
          row.workspace.connection!,
          "pane.send_input",
          {
            pane_id: row.panel.herdrId,
            raw,
          },
        );
      }
      if (clearNote) setNote(item, "");
      await sleep(300);
      await read(row).catch(() => {});
    });
  const replySession = (item: SessionItem) => {
    const prompt = prompts[item.row.panel.id];
    const steps = prompt && replySteps(prompt, noteOf(item));
    if (steps) send(item, steps, true);
  };
  const runAgain = (task: Task) =>
    void act(() => clientOf(task).taskStart(task.id));
  const archive = (task: Task) =>
    void act(() => clientOf(task).taskArchive(task.id));
  const land = (task: Task) => void act(() => clientOf(task).taskLand(task.id));
  const jump = (row: InboxRow) => {
    switchWorkspace(row.workspace.id);
    ws.setSelected(row.panel.id);
    ws.setZoomed(row.panel.id);
  };
  const landEverything = (targets: Task[]) => {
    if (!landAll) return setLandAll(true);
    setLandAll(false);
    void act(async () => {
      for (const task of targets) await clientOf(task).taskLand(task.id);
    });
  };

  const handlers = useRef<(event: KeyboardEvent) => void>(() => {});
  handlers.current = (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    const key = event.key.toLowerCase();
    if (ownsKey(event.target, key)) return;
    const item = selected;
    const move = (delta: number) => {
      event.preventDefault();
      setSelectedKey(stepSelection(keys, item?.key ?? null, delta));
    };
    if (key === "j") return move(1);
    if (key === "k") return move(-1);
    if (!item || busy) return;
    if (item.source === "session") {
      const prompt = prompts[item.row.panel.id];
      if (item.kind === "answer" && prompt && /^[1-9]$/.test(key)) {
        const option = prompt.options[Number(key) - 1];
        if (option) send(item, option.steps);
      } else if (key === "e" && item.kind === "panels")
        markSeen(item.row.panel.id);
      return;
    }
    const { task } = item;
    if (item.kind === "answer" && /^[1-9]$/.test(key)) {
      const option = task.question?.options[Number(key) - 1];
      if (option) pickOption(item, option);
    } else if (key === "enter" && item.kind === "answer") answerByKey(item);
    else if (key === "l" && item.kind === "land") land(task);
    else if (key === "r" && item.kind === "decide" && task.status !== "landing")
      runAgain(task);
    else if (key === "e" && item.kind !== "answer") archive(task);
  };
  useEffect(() => {
    const listener = (event: KeyboardEvent) => handlers.current(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  const recent = (predicate: (task: Task) => boolean) =>
    allTasks
      .filter((task) => !task.archived && predicate(task))
      .sort((a, b) => b.updatedAt - a.updatedAt)[0];

  const hostName = (item: Item) => groupLabel(item.host, connectionProfiles);
  const sessionKind = (row: InboxRow) =>
    row.panel.agent
      ? agentName(row)
      : row.panel.kind === "terminal"
        ? "terminal"
        : "agent panel";

  /** Needed a fix / Clean, as in the task view: a fix takes a note (the
   * follow-up task is made from it), pressing an active mark clears it. */
  function landMarks(task: Task) {
    const mark = task.leadTouch;
    if (fixing === task.id)
      return (
        <form
          className="inbox-fix"
          onSubmit={(event) => {
            event.preventDefault();
            void act(async () => {
              await clientOf(task).taskLeadTouch(task.id, true, fixNote.trim());
              setFixing(null);
              setFixNote("");
            });
          }}
        >
          <input
            aria-label="What had to be fixed"
            placeholder="What is missing or wrong? A follow-up task will be created."
            value={fixNote}
            autoFocus
            onChange={(event) => setFixNote(event.target.value)}
          />
          <button type="submit" className="ui-button primary" disabled={busy}>
            Save
          </button>
          <button
            type="button"
            className="ui-button ghost"
            onClick={() => setFixing(null)}
          >
            Cancel
          </button>
        </form>
      );
    return (
      <>
        <button
          className="ui-button ghost"
          disabled={busy}
          aria-pressed={mark?.touched === true}
          onClick={() => {
            if (mark?.touched === true)
              void act(() => clientOf(task).taskLeadTouch(task.id));
            else {
              setFixNote("");
              setFixing(task.id);
            }
          }}
        >
          Needed a fix
        </button>
        <button
          className="ui-button ghost"
          disabled={busy}
          aria-pressed={mark?.touched === false}
          onClick={() =>
            void act(() =>
              clientOf(task).taskLeadTouch(
                task.id,
                mark?.touched === false ? undefined : false,
              ),
            )
          }
        >
          Clean
        </button>
      </>
    );
  }

  function optionChips(item: SessionItem, prompt: SessionPrompt) {
    return prompt.options.map((option, index) => (
      <span
        key={`${index}:${option.label}`}
        className="inbox-option"
        title={option.hint ? `${option.label} ${option.hint}` : option.label}
      >
        <Chip
          selected={option.checked === true}
          disabled={busy}
          onClick={() => send(item, option.steps)}
        >
          {prompt.multi ? `${option.checked ? "☑" : "☐"} ` : ""}
          {option.label}
        </Chip>
      </span>
    ));
  }

  function sessionView(item: SessionItem) {
    const { row } = item;
    const prompt = prompts[row.panel.id];
    const meta = `${item.project} · ${hostName(item)} · ${sessionKind(row)}`;
    const context =
      item.kind === "answer"
        ? prompt?.question ||
          `Waiting for your input in the ${row.panel.kind === "terminal" ? "terminal" : "panel"}`
        : `Finished “${row.panel.title}”`;
    const actions = (
      <>
        {item.kind === "answer" && prompt && optionChips(item, prompt)}
        <button className="ui-button secondary" onClick={() => jump(row)}>
          <ArrowUpRight size={14} />{" "}
          {item.kind === "answer" ? "Jump to panel" : "Open panel"}
        </button>
        {item.kind === "panels" && (
          <button
            className="ui-button ghost"
            onClick={() => markSeen(row.panel.id)}
          >
            Mark seen
          </button>
        )}
      </>
    );
    return { meta, context, actions };
  }

  function taskView(item: TaskItem) {
    const { task } = item;
    const choice = choiceOf(item);
    const attempts = implementAttemptCount(task);
    const meta =
      item.kind === "land"
        ? `${item.project} · orchestrator → ${task.baseRef}`
        : `${item.project} · orchestrator${item.kind === "answer" && attempts > 0 ? ` · attempt ${attempts}` : ""}`;
    let context: string;
    if (item.kind === "answer")
      context = (task.question?.text ?? "").split("\n")[0];
    else if (item.kind === "land") {
      const files = latestImplementAttempt(task)?.changedFiles.length;
      const review = reviewOf(task);
      context = [
        review ? `review ${review.verdict}` : "done",
        files ? plural(files, "file") : "",
        formatCost(task.costUsd),
        "not landed",
      ]
        .filter(Boolean)
        .join(" · ");
    } else
      context = `${taskReason(task, allTasks)} · ${formatCost(task.costUsd)}`;

    let actions;
    if (item.kind === "answer")
      actions = (task.question?.options ?? []).map((option) => (
        <Chip
          key={option}
          selected={shownPick(choice) === option}
          onClick={() => {
            pickOption(item, togglePick(choice, option));
            setNote(item, "");
          }}
        >
          {option}
        </Chip>
      ));
    else if (item.kind === "decide")
      actions = (
        <>
          {task.status !== "landing" && (
            <button
              className="ui-button secondary"
              disabled={busy}
              onClick={() => runAgain(task)}
            >
              <RefreshCw size={14} /> Run again
            </button>
          )}
          <button className="ui-button ghost" onClick={() => openTask(task)}>
            Run with a note
          </button>
          <button
            className="ui-button ghost"
            disabled={busy}
            onClick={() => archive(task)}
          >
            Archive
          </button>
        </>
      );
    else
      actions = (
        <>
          <button
            className="ui-button primary"
            disabled={busy}
            onClick={() => land(task)}
          >
            Land
          </button>
          {landMarks(task)}
        </>
      );
    return { meta, context, actions };
  }

  function itemView(item: Item) {
    if (item.kind === "working" || item.kind === "idle")
      return compactRow(item);
    const { meta, context, actions } =
      item.source === "session" ? sessionView(item) : taskView(item);
    return (
      <div
        key={item.key}
        className="inbox-item"
        title={item.title}
        onClick={() => setSelectedKey(item.key)}
      >
        <AttentionItem
          tone={TONE[item.kind]}
          title={item.title}
          time={item.at != null ? ageLabel(Date.now() - item.at) : undefined}
          context={context}
          question={item.kind === "answer"}
          actions={actions}
          meta={meta}
          selected={selected?.key === item.key}
          onOpen={() => setSelectedKey(item.key)}
        />
      </div>
    );
  }

  /** A working or idle session: one bordered row (Figma "working-row"). */
  function compactRow(item: SessionItem) {
    const { row } = item;
    return (
      <div
        key={item.key}
        className={`inbox-session-row${selected?.key === item.key ? " selected" : ""}`}
        onClick={() => setSelectedKey(item.key)}
        onDoubleClick={() => jump(row)}
      >
        <span
          className={`ui-dot ui-tone-${item.kind === "working" ? "info" : "neutral"}`}
          aria-hidden
        />
        <span className="inbox-session-name" title={item.title}>
          {item.title}
        </span>
        <span className="inbox-session-activity">{hostName(item)}</span>
        {item.at != null && (
          <span className="inbox-session-meta">
            {ageLabel(Date.now() - item.at)}
          </span>
        )}
        <button
          className="inbox-link inbox-session-jump"
          aria-label={`Jump to ${row.panel.title} in ${row.workspace.name}`}
          onClick={(event) => {
            event.stopPropagation();
            jump(row);
          }}
        >
          <ArrowUpRight size={12} />
        </button>
      </div>
    );
  }

  function group(kind: Kind, rows: Item[]) {
    if (kind === "idle") return idleSection(rows as SessionItem[]);
    if (rows.length === 0) return null;
    const targets = kind === "land" ? landTargets(rows) : [];
    return (
      <section key={kind} className="inbox-group">
        <div className="inbox-group-head">
          <GroupLabel label={LABELS[kind]} count={rows.length} />
          {targets.length > 1 && (
            <button
              className="inbox-link"
              disabled={busy}
              onClick={() => landEverything(targets)}
              onBlur={() => setLandAll(false)}
            >
              {landAll
                ? `Confirm: land ${targets.length}`
                : `Land ${targets.length}`}
            </button>
          )}
        </div>
        {rows.map(itemView)}
      </section>
    );
  }

  function sessionPreview(item: SessionItem) {
    const { row } = item;
    const prompt = prompts[row.panel.id];
    const head = (
      <div className="inbox-preview-head">
        <h2 title={item.title}>{item.title}</h2>
        <button className="inbox-link" onClick={() => jump(row)}>
          {item.kind === "answer" ? "Jump to panel" : "Open panel"}
          <ArrowUpRight size={12} />
        </button>
      </div>
    );
    if (item.kind !== "answer")
      return (
        <>
          {head}
          <p className="inbox-session-line">
            {`${item.project} · ${hostName(item)} · ${sessionKind(row)} · ${item.kind === "panels" ? "finished, not seen" : item.kind}`}
          </p>
        </>
      );
    if (!answerable(row) || !prompt)
      return (
        <>
          {head}
          <p className="inbox-session-line">
            {answerable(row)
              ? "Reading the session…"
              : "Waiting for your input in the panel."}
          </p>
        </>
      );
    const note = noteOf(item);
    const reply = replySteps(prompt, note);
    return (
      <>
        {head}
        <p className="inbox-prompt-question">
          {prompt.question || "Waiting for your input"}
        </p>
        {prompt.detail.length > 0 && (
          <pre className="inbox-prompt-detail">{prompt.detail.join("\n")}</pre>
        )}
        {prompt.options.length > 0 && (
          <div className="inbox-prompt-options">
            {optionChips(item, prompt)}
            {prompt.multi && (
              <button
                className="ui-button secondary"
                disabled={busy}
                onClick={() => send(item, prompt.advance)}
              >
                Continue
              </button>
            )}
          </div>
        )}
        <div className="inbox-preview-spacer" />
        {prompt.typeSteps && (
          <form
            className="inbox-reply"
            onSubmit={(event) => {
              event.preventDefault();
              replySession(item);
            }}
          >
            <input
              aria-label="Reply to the session"
              placeholder="Reply in the session…"
              value={note}
              onChange={(event) => setNote(item, event.target.value)}
            />
            <button
              type="submit"
              className="ui-button primary"
              disabled={busy || !reply}
            >
              Send
            </button>
          </form>
        )}
      </>
    );
  }

  function preview(item: Item) {
    if (item.source === "session") return sessionPreview(item);
    const { task } = item;
    const attempts = implementAttemptCount(task);
    const diff = diffOf(task);
    const files =
      diff?.files ?? latestImplementAttempt(task)?.changedFiles.length;
    const met = criteriaMet(task);
    const facts = diffFacts(
      files,
      attempts,
      maxAttempts[item.host],
      task.costUsd,
    );
    return (
      <>
        <div className="inbox-preview-head">
          <h2 title={task.title}>{task.title}</h2>
          <button className="inbox-link" onClick={() => openTask(task)}>
            Open task
            <ArrowUpRight size={12} />
          </button>
        </div>
        <StageTrack steps={stageTrack(task)} />
        {task.criteria.length > 0 && (
          <div className="inbox-criteria">
            <span className="inbox-eyebrow">ACCEPTANCE</span>
            {task.criteria.map((text) => (
              <Criterion key={text} state={met ? "met" : "pending"}>
                {text}
              </Criterion>
            ))}
          </div>
        )}
        <div className="inbox-diff">
          <GitBranch size={13} />
          <span className="inbox-diff-branch" title={task.branch}>
            {shortBranch(task.branch)}
          </span>
          {!!diff?.added && (
            <span className="inbox-diff-add">+{diff.added}</span>
          )}
          {!!diff?.removed && (
            <span className="inbox-diff-del">{`−${diff.removed}`}</span>
          )}
          {facts && <span className="inbox-diff-rest">{`· ${facts}`}</span>}
        </div>
        <div className="inbox-preview-spacer" />
        {item.kind === "answer" && (
          <form
            className="inbox-reply"
            onSubmit={(event) => {
              // Enter in the field: typed text or an explicit pick only.
              event.preventDefault();
              answerByKey(item);
            }}
          >
            <input
              aria-label="Answer note"
              placeholder="Answer with a note, or pick above…"
              value={noteOf(item)}
              onChange={(event) => setNote(item, event.target.value)}
            />
            <button
              type="button"
              className="ui-button primary"
              disabled={busy || !clickAnswer(choiceOf(item))}
              onClick={() => answerTask(item, clickAnswer(choiceOf(item)))}
            >
              Answer
            </button>
          </form>
        )}
      </>
    );
  }

  const errorLine = error && (
    <p className="inbox-error" role="alert">
      {error}
    </p>
  );

  function zero() {
    const latest = recent(() => true);
    const landed = recent((task) => task.status === "done" && !!task.landedSha);
    const running = visible.filter((item) => !needsYou(item));
    return (
      <div className="inbox-zero-page">
        <div className="inbox-zero">
          <div className="inbox-zero-mascot">
            <Character mood="idle" />
            <span className="inbox-zero-shadow" />
          </div>
          <h2>Inbox zero</h2>
          <p>{zeroLine(allTasks)}</p>
          <div className="inbox-zero-actions">
            <button
              className="ui-button secondary"
              onClick={() =>
                openOrchestratorTask(
                  latest
                    ? { taskId: latest.id, repo: latest.repo, focus: "summary" }
                    : { taskId: "", repo: cwd, focus: "summary" },
                )
              }
            >
              <ListChecks size={14} /> Open orchestrator
            </button>
            <button
              className="ui-button ghost"
              disabled={!landed}
              title={landed ? undefined : "Nothing has landed yet"}
              onClick={() =>
                landed &&
                openOrchestratorTask({
                  taskId: landed.id,
                  repo: landed.repo,
                  focus: "report",
                })
              }
            >
              See what landed
            </button>
          </div>
          {errorLine}
        </div>
        {(running.length > 0 ||
          candidates.agents.length + candidates.shells.length > 0) && (
          <div className="inbox-zero-sessions">
            {(["working", "idle"] as const).map((kind) =>
              group(
                kind,
                running.filter((item) => item.kind === kind),
              ),
            )}
          </div>
        )}
      </div>
    );
  }

  function cleanupRow(row: InboxRow) {
    const on = cleanup?.has(row.panel.id) ?? false;
    const name = row.panel.agent ? agentName(row) : row.panel.title;
    return (
      <div key={row.panel.id} className="inbox-cleanup-row">
        <input
          type="checkbox"
          aria-label={`End ${row.panel.title} in ${row.workspace.name}`}
          checked={on}
          onChange={(event) =>
            setCleanup((old) => {
              const next = new Set(old);
              if (event.target.checked) next.add(row.panel.id);
              else next.delete(row.panel.id);
              return next;
            })
          }
        />
        <Icon kind={row.panel.kind} agent={row.panel.agent} />
        <span className="inbox-session-name" title={row.panel.title}>
          {`${row.workspace.name} · ${name}`}
        </span>
        <span className="inbox-session-meta">
          {groupLabel(groupKey(row.workspace.connection), connectionProfiles)}
        </span>
        <button
          className="inbox-link"
          aria-label={`Jump to ${row.panel.title}`}
          onClick={() => jump(row)}
        >
          Jump <ArrowUpRight size={12} />
        </button>
      </div>
    );
  }

  /** Idle sessions and shells: one "› N idle sessions   Clean up" row
   * (Figma "idle") that opens into the idle rows, or into the cleanup list. */
  function idleSection(rows: SessionItem[]) {
    const { agents, shells } = candidates;
    if (cleanup) return cleanupView();
    if (rows.length + agents.length + shells.length === 0) return null;
    const idleCount = Math.max(rows.length, agents.length);
    return (
      <section key="idle" className="inbox-group" aria-label="Idle">
        <div className="inbox-idle">
          <button
            className="inbox-idle-toggle"
            aria-expanded={idleOpen}
            disabled={rows.length === 0}
            onClick={() => setIdleOpen((open) => !open)}
          >
            {idleOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            <span>
              {[
                idleCount ? plural(idleCount, "idle session") : "",
                shells.length ? plural(shells.length, "shell") : "",
              ]
                .filter(Boolean)
                .join(" · ")}
            </span>
          </button>
          {agents.length + shells.length > 0 && (
            <button
              className="inbox-link"
              onClick={() =>
                setCleanup(new Set(agents.map((row) => row.panel.id)))
              }
            >
              Clean up
            </button>
          )}
        </div>
        {idleOpen && rows.map(compactRow)}
      </section>
    );
  }

  function cleanupView() {
    const { agents, shells } = candidates;
    if (!cleanup || agents.length + shells.length === 0) return null;
    const chosen = [...agents, ...shells].filter((row) =>
      cleanup.has(row.panel.id),
    );
    return (
      <div className="inbox-cleanup" aria-label="Sessions to end">
        {agents.length > 0 && (
          <>
            <span className="inbox-eyebrow">
              IDLE SESSIONS · {agents.length}
            </span>
            {agents.map(cleanupRow)}
          </>
        )}
        {shells.length > 0 && (
          <>
            <span className="inbox-eyebrow">
              SHELLS · {shells.length} · may be running
            </span>
            {shells.map(cleanupRow)}
          </>
        )}
        <div className="inbox-cleanup-foot">
          <span>Ending stops running commands; files stay on disk.</span>
          <button className="inbox-link" onClick={() => setCleanup(null)}>
            Cancel
          </button>
          <button
            className="inbox-link danger"
            disabled={busy || chosen.length === 0}
            onClick={() =>
              void act(async () => {
                await ws.endSessions(
                  chosen.map(({ workspace, panel }) => ({ workspace, panel })),
                );
                setCleanup(null);
              })
            }
          >
            {busy ? "Closing…" : `End ${chosen.length}`}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="section-page inbox-page">
      <div className="inbox-head">
        <div className="inbox-head-title">
          <h1>Inbox</h1>
          {needs.length > 0 && <p>{headline}</p>}
        </div>
        <label className="inbox-search">
          <input
            aria-label="Search inbox"
            placeholder="Search title or project…"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
        <span className="inbox-select">
          <select
            aria-label="Inbox project"
            value={project}
            onChange={(event) => setProject(event.target.value)}
          >
            <option value="">All projects</option>
            {projects.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
          <ChevronDown size={12} aria-hidden />
        </span>
        <span className="inbox-select">
          <select
            aria-label="Inbox host"
            value={hostFilter}
            onChange={(event) => setHostFilter(event.target.value)}
          >
            <option value="">All hosts</option>
            {hosts.map(([key, label]) => (
              <option key={key} value={key}>
                {label}
              </option>
            ))}
          </select>
          <ChevronDown size={12} aria-hidden />
        </span>
        <ExtensionNavSlot
          registry={registry}
          placement="sessions.navigation"
          className="secondary extension-nav"
          onOpen={openExtensionTarget}
        />
      </div>
      <ExtensionSectionSlot
        registry={registry}
        host="sessions.section"
        cwd={cwd}
        connection={connection}
        workspaces={workspaces}
      />
      {needs.length === 0 ? (
        zero()
      ) : (
        <>
          <div className="inbox-filters">
            {CHIPS.map(({ filter: value, label }) => (
              <Chip
                key={value}
                selected={filter === value}
                onClick={() => setFilter(value)}
              >
                {`${label} ${counts(value)}`}
              </Chip>
            ))}
          </div>
          <div className="inbox-split">
            <div className="inbox-queue">
              {KINDS.map((kind) =>
                group(
                  kind,
                  visible.filter((item) => item.kind === kind),
                ),
              )}
              {visible.length === 0 && (
                <p className="inbox-none">Nothing matches.</p>
              )}
              {errorLine}
            </div>
            <div className="inbox-preview">
              {selected ? preview(selected) : null}
            </div>
          </div>
          <div className="inbox-keys">
            {[
              ["J K", "move"],
              ["1–9", "pick"],
              ["⏎", "answer"],
              ["L", "land"],
              ["R", "run again"],
              ["E", "done"],
            ].map(([key, text]) => (
              <span key={text}>
                <kbd>{key}</kbd>
                {text}
              </span>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
