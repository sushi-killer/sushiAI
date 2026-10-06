import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ArrowUpRight, ChevronDown, ChevronRight } from "lucide-react";
import {
  ExtensionNavSlot,
  ExtensionSectionSlot,
} from "../extensions/ExtensionSlots.tsx";
import { moduleUis } from "../extensions/modules.ts";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import type { InboxGroup, InboxRow } from "./attention.ts";
import { LOCAL_GROUP, groupKey, groupLabel } from "./workspaceMerge.ts";
import {
  askDecision,
  asksOf,
  summarizeAskInput,
  type OpenAsk,
  type SessionsByHost,
} from "../daemonSessions.ts";
import { daemonHost } from "../daemonSessions.ts";
import { errorText } from "./errors.ts";
import {
  KINDS,
  ageLabel,
  agentName,
  cleanupCandidates,
  inScope,
  inboxItems,
  needsYou,
  ownsKey,
  reviewTargets,
  scopedHeadline,
  stepSelection,
  type Item,
  type Kind,
  type ModuleEntry,
  type ModuleItem,
  type SessionItem,
} from "./inboxModel.ts";
import { plural } from "../lib/text.ts";
import { AttentionItem, Chip, GroupLabel } from "../ui";
import {
  parseSessionPrompt,
  replySteps,
  type SessionPrompt,
} from "./sessionPrompt.ts";
import { Icon } from "../PanelIcon.tsx";
import { Character } from "../mascot/Character.tsx";
import type { ConnectionProfile, Workspace } from "../types";
import type { WorkspaceController } from "../workspace/useWorkspaces.ts";
import "./inbox.css";

type Filter = "all" | "answer" | "decide" | "review" | "panels";

const LABELS: Record<Kind, string> = {
  answer: "ANSWER",
  decide: "DECIDE",
  review: "REVIEW",
  panels: "PANELS",
  working: "WORKING",
  idle: "IDLE",
};
const CHIPS: { filter: Filter; label: string }[] = [
  { filter: "all", label: "All" },
  { filter: "answer", label: "Answer" },
  { filter: "decide", label: "Decide" },
  { filter: "review", label: "Review" },
  { filter: "panels", label: "Panels" },
];
const TONE = {
  answer: "warning",
  decide: "danger",
  review: "ok",
  panels: "info",
} as const;

/** How often a blocked session's screen is re-read while it waits. */
const SCREEN_REFRESH_MS = 2500;

/** A session whose screen has not been read yet: free text only. */
const NO_PROMPT: SessionPrompt = {
  question: "",
  detail: [],
  options: [],
  multi: false,
  advance: [],
  typeSteps: [],
};

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** A daemon session the Inbox can read and type into. */
const answerable = (row: InboxRow) => !!row.panel.sessionId && !row.panel.ended;
const hostOfRow = (row: InboxRow) => daemonHost(row.workspace.connection);

/** The visible text of every blocked session, re-read while it waits. */
function usePaneScreens(rows: InboxRow[]) {
  const [screens, setScreens] = useState<Record<string, string>>({});
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const read = useCallback(async (row: InboxRow) => {
    const result = await window.bridge?.sessionRead(
      hostOfRow(row),
      row.panel.sessionId!,
    );
    const text: unknown = result?.text;
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
 * waiting on an answer or finished unseen, and the rows modules put in the
 * Inbox (each drawn by its module's `AttentionDetail`) - then what is still
 * working or idle. One queue on the left, the selected item on the right. */
export function InboxPage({
  groups,
  markSeen,
  moduleItems,
  switchWorkspace,
  ws,
  connectionProfiles,
  registry,
  openExtensionTarget,
  cwd,
  connection,
  workspaces,
  daemonSessions,
}: {
  groups: InboxGroup[];
  markSeen(panelId: string): void;
  moduleItems: ModuleEntry[];
  switchWorkspace(id: string): void;
  ws: WorkspaceController;
  connectionProfiles: ConnectionProfile[];
  registry: ExtensionRegistry;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
  cwd: string;
  connection?: string;
  workspaces: Workspace[];
  /** What each daemon host runs, open asks included (`useDaemon`). */
  daemonSessions: SessionsByHost;
}) {
  const [query, setQuery] = useState(""),
    [project, setProject] = useState(""),
    [hostFilter, setHostFilter] = useState(""),
    [filter, setFilter] = useState<Filter>("all"),
    [selectedKey, setSelectedKey] = useState<string | null>(null),
    [notes, setNotes] = useState<Record<string, string>>({}),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [reviewAll, setReviewAll] = useState<string | null>(null),
    [cleanup, setCleanup] = useState<Set<string> | null>(null),
    [idleOpen, setIdleOpen] = useState(false);
  const inFlight = useRef(false);

  const sessionRows = useMemo(
    () =>
      groups
        .filter((group) => group.key !== "shells")
        .flatMap((group) => group.rows)
        .filter(answerable),
    [groups],
  );
  const asksByPanel = useMemo(() => {
    const out: Record<string, OpenAsk[]> = {};
    for (const row of sessionRows) {
      const open = asksOf(daemonSessions, hostOfRow(row), row.panel.sessionId!);
      if (open.length) out[row.panel.id] = open;
    }
    return out;
  }, [sessionRows, daemonSessions]);
  const items = useMemo(
    () => inboxItems(moduleItems, groups, new Set(Object.keys(asksByPanel))),
    [moduleItems, groups, asksByPanel],
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
  const noteOf = (item: Item) => notes[item.key] ?? "";
  const setNote = (item: Item, text: string) =>
    setNotes((old) => ({ ...old, [item.key]: text }));
  /** Types into the session, one step at a time, then re-reads its screen. */
  const send = (item: SessionItem, steps: string[], clearNote = false) =>
    void act(async () => {
      const { row } = item;
      for (const [index, raw] of steps.entries()) {
        if (index > 0) await sleep(150);
        await window.bridge!.sessionInput(
          hostOfRow(row),
          row.panel.sessionId!,
          raw,
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
  const decide = (ask: OpenAsk, decision: "allow" | "deny") =>
    void act(() => {
      const [host, response] = askDecision(ask, decision);
      return window.bridge!.askRespond(host, response);
    });
  const jump = (row: InboxRow) => {
    switchWorkspace(row.workspace.id);
    ws.setSelected(row.panel.id);
    ws.setZoomed(row.panel.id);
  };
  /** A module's "review all": the first click arms it, the second runs it
   * on the review rows on screen. */
  const reviewEverything = (extensionId: string, keys: string[]) => {
    const ui = moduleUis.find((m) => m.extensionId === extensionId);
    if (!ui?.reviewAll) return;
    if (reviewAll !== extensionId) return setReviewAll(extensionId);
    setReviewAll(null);
    void act(() => ui.reviewAll!.run(keys));
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
    // A module's own keys are handled by its detail.
    if (item.source !== "session") return;
    const prompt = prompts[item.row.panel.id];
    if (item.kind === "answer" && prompt && /^[1-9]$/.test(key)) {
      const option = prompt.options[Number(key) - 1];
      if (option) send(item, option.steps);
    } else if (key === "e" && item.kind === "panels")
      markSeen(item.row.panel.id);
  };
  useEffect(() => {
    const listener = (event: KeyboardEvent) => handlers.current(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  const hostName = (item: Item) => groupLabel(item.host, connectionProfiles);
  const sessionKind = (row: InboxRow) =>
    row.panel.agent
      ? agentName(row)
      : row.panel.kind === "terminal"
        ? "terminal"
        : "agent panel";

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

  /** What a permission ask is about: the tool and a short summary of its input. */
  const askLine = (ask: OpenAsk) =>
    [ask.tool || "Tool", summarizeAskInput(ask.input)]
      .filter(Boolean)
      .join(" · ");

  function askButtons(ask: OpenAsk) {
    return (
      <>
        <button
          className="ui-button primary"
          disabled={busy}
          onClick={() => decide(ask, "allow")}
        >
          Allow
        </button>
        <button
          className="ui-button secondary"
          disabled={busy}
          onClick={() => decide(ask, "deny")}
        >
          Deny
        </button>
      </>
    );
  }

  function sessionView(item: SessionItem) {
    const { row } = item;
    const prompt = prompts[row.panel.id];
    const open = asksByPanel[row.panel.id] ?? [];
    const meta = `${item.project} · ${hostName(item)} · ${sessionKind(row)}`;
    const context =
      item.kind === "answer" && open.length
        ? `${askLine(open[0])}${open.length > 1 ? ` · +${open.length - 1} more` : ""}`
        : item.kind === "answer"
          ? prompt?.question ||
            `Waiting for your input in the ${row.panel.kind === "terminal" ? "terminal" : "panel"}`
          : `Finished “${row.panel.title}”`;
    const actions = (
      <>
        {open.length > 0 && askButtons(open[0])}
        {open.length === 0 &&
          item.kind === "answer" &&
          prompt &&
          optionChips(item, prompt)}
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

  /** A module row: title, age and where it comes from. What to do with it
   * is in the detail. */
  function moduleView(item: ModuleItem) {
    return {
      meta: `${item.project} · ${hostName(item)}`,
      context: undefined,
      actions: undefined,
    };
  }

  function itemView(item: Item) {
    if (item.kind === "working" || item.kind === "idle")
      return compactRow(item);
    const { meta, context, actions } =
      item.source === "session" ? sessionView(item) : moduleView(item);
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
    const targets =
      kind === "review"
        ? reviewTargets(rows).filter((target) => target.keys.length > 1)
        : [];
    return (
      <section key={kind} className="inbox-group">
        <div className="inbox-group-head">
          <GroupLabel label={LABELS[kind]} count={rows.length} />
          {targets.map(({ extensionId, keys: targetKeys }) => {
            const label = moduleUis.find((m) => m.extensionId === extensionId)
              ?.reviewAll?.label;
            if (!label) return null;
            return (
              <button
                key={extensionId}
                className="inbox-link"
                disabled={busy}
                onClick={() => reviewEverything(extensionId, targetKeys)}
                onBlur={() => setReviewAll(null)}
              >
                {reviewAll === extensionId
                  ? `Confirm: ${label.toLowerCase()} ${targetKeys.length}`
                  : `${label} ${targetKeys.length}`}
              </button>
            );
          })}
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
    const open = asksByPanel[row.panel.id] ?? [];
    if (open.length > 0) {
      const screen = (screens[row.panel.id] ?? "")
        .split("\n")
        .filter((line) => line.trim())
        .slice(-12)
        .join("\n");
      const note = noteOf(item);
      const reply = replySteps(prompt ?? NO_PROMPT, note);
      return (
        <>
          {head}
          <p className="inbox-session-line">
            {`${agentName(row)} · ${row.panel.title} · ${row.workspace.name}`}
          </p>
          {open.map((ask) => (
            <div key={ask.askId} className="inbox-ask">
              <p className="inbox-prompt-question">{askLine(ask)}</p>
              <div className="inbox-prompt-options">{askButtons(ask)}</div>
            </div>
          ))}
          {screen && <pre className="inbox-prompt-detail">{screen}</pre>}
          <div className="inbox-preview-spacer" />
          <form
            className="inbox-reply"
            onSubmit={(event) => {
              event.preventDefault();
              if (reply) send(item, reply, true);
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
        </>
      );
    }
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
    const ui = moduleUis.find((m) => m.extensionId === item.entry.extensionId);
    if (!ui) return null;
    const Detail = ui.AttentionDetail;
    return (
      <Suspense fallback={null}>
        <Detail item={item.entry.item} />
      </Suspense>
    );
  }

  const errorLine = error && (
    <p className="inbox-error" role="alert">
      {error}
    </p>
  );

  function zero() {
    const running = visible.filter((item) => !needsYou(item));
    return (
      <div className="inbox-zero-page">
        <div className="inbox-zero">
          <div className="inbox-zero-mascot">
            <Character mood="idle" />
            <span className="inbox-zero-shadow" />
          </div>
          <h2>Inbox zero</h2>
          <p>Nothing needs you.</p>
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
            {CHIPS.filter(
              ({ filter: value }) =>
                // A group no module fills is not offered.
                (value !== "decide" && value !== "review") ||
                items.some((item) => item.source === "module"),
            ).map(({ filter: value, label }) => (
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
