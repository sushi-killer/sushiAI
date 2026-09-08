import { AgentRecoveryPanel } from "./AgentRecoveryPanel";
import { AgentImage } from "./AgentImage";
import { createPortal } from "react-dom";
import { Plus, RefreshCw } from "lucide-react";
import { AgentFilesPanel } from "./AgentFilesPanel";
import { AgentGitPanel } from "./AgentGitPanel";
import { AgentSkillPicker } from "./AgentSkillPicker";
import { AgentAttachments } from "./AgentAttachments";
import {
  useAttachmentDrafts,
  attachmentIsLoading,
  addAttachmentFiles,
  setAttachmentFiles,
  setAttachmentError,
} from "./attachment-drafts";
import { AgentConversationSettings } from "./AgentConversationSettings";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type {
  AgentIdentity,
  AgentProvider,
  AgentConversation,
  AgentSnapshot,
  AgentTranscriptItem,
  AgentInteraction,
} from "./types";
import { AgentConnectionsPanel } from "./AgentConnectionsPanel";
import { AgentAddonPanel } from "./AgentAddonPanel";
import { AgentActivityPanel } from "./AgentActivityPanel";
import { blank, visible, groupItems, label } from "./transcript";
import "./agents.css";

type Tab = {
  providerId: string;
  agentId: string;
  conversationId: string;
  title: string;
};
const identity = (t: Tab) =>
  JSON.stringify([t.providerId, t.agentId, t.conversationId]);
const storage = "sushiai.agent-tabs.v1";
const focusStorage = "sushiai.agent-focus.v1";
// Which tab was open survives a restart alongside the tab list; unsent text
// stays in memory across mode switches and is never written to storage.
function savedFocus(): { providerId: string; agentId: string; active: string } {
  const empty = { providerId: "", agentId: "", active: "" };
  try {
    const row = JSON.parse(localStorage.getItem(focusStorage) || "{}");
    return row &&
      ["providerId", "agentId", "active"].every(
        (key) => typeof row[key] === "string",
      )
      ? { providerId: row.providerId, agentId: row.agentId, active: row.active }
      : empty;
  } catch {
    return empty;
  }
}
const viewMemory = {
  ...savedFocus(),
  drafts: {} as Record<string, string>,
  // The last known providers and agents survive a remount, so switching tabs
  // shows the roster at once instead of an empty list that fills in later.
  providers: [] as AgentProvider[],
  agents: [] as AgentIdentity[],
};
type ConversationPage = {
  conversations: AgentConversation[];
  nextOffset: number | null;
  searchLimited?: boolean;
};
const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
const display = (v: unknown) =>
  typeof v === "string" ? v : v == null ? "" : JSON.stringify(v, null, 2);
function savedTabs(): Tab[] {
  try {
    const rows = JSON.parse(localStorage.getItem(storage) || "[]");
    return Array.isArray(rows)
      ? rows
          .filter(
            (t) =>
              t &&
              [t.providerId, t.agentId, t.conversationId, t.title].every(
                (v) => typeof v === "string",
              ),
          )
          .slice(0, 30)
      : [];
  } catch {
    return [];
  }
}
export function RichText({ text }: { text: string }) {
  return (
    <Markdown
      remarkPlugins={[remarkGfm]}
      skipHtml
      components={{
        a: ({ href, children }) => (
          <a
            href={href}
            onClick={(e) => {
              e.preventDefault();
              if (href)
                void window.bridge?.agentOpenExternal(href).catch(() => {});
            }}
          >
            {children}
          </a>
        ),
        img: ({ alt }) => (
          <span className="agent-media-label">
            Image: {alt || "attachment"}
          </span>
        ),
      }}
    >
      {text}
    </Markdown>
  );
}
function ToolGroup({ items }: { items: AgentTranscriptItem[] }) {
  const running = items.some((i) => i.status === "running");
  const failed = items.some((i) => i.status === "error");
  const last = items[items.length - 1];
  return (
    <details
      className={`agent-detail agent-tool agent-tool-group ${running ? "running" : failed ? "error" : ""}`}
    >
      <summary>
        <span>{items.length} actions</span>
        {/* The name of the last action only: an item's own text can be a whole
            answer, which does not belong in a one-line header. */}
        <small>{label(last.effect || last.name).slice(0, 60)}</small>
        <span className="agent-tool-status">
          {running ? "Running…" : failed ? "Failed" : "Done"}
        </span>
      </summary>
      {items.map((item) => (
        <TranscriptItem key={item.id} item={item} />
      ))}
    </details>
  );
}
function TranscriptItem({ item }: { item: AgentTranscriptItem }) {
  if (item.kind === "text")
    return (
      <article className={`agent-message ${item.role || "assistant"}`}>
        <RichText text={item.text || ""} />
      </article>
    );
  if (item.kind === "reasoning")
    return (
      <details className="agent-detail">
        <summary>
          Reasoning {item.status === "running" ? "· thinking…" : ""}
        </summary>
        <RichText text={item.text || ""} />
      </details>
    );
  if (item.kind === "tool") {
    const name = label(item.effect || item.name) || "Tool";
    const group = label(item.category);
    return (
      <details className={`agent-detail agent-tool ${item.status || ""}`}>
        <summary>
          <span>{name}</span>
          {/* The group is a second word only when it says something the name does not. */}
          {group && group !== name && item.category !== "other" && (
            <small>{group}</small>
          )}
          <span className="agent-tool-status">
            {item.status === "running"
              ? "Running…"
              : item.status === "error"
                ? "Failed"
                : "Done"}
          </span>
        </summary>
        {!blank(item.input) && (
          <>
            <h4>Input</h4>
            <pre>{display(item.input)}</pre>
          </>
        )}
        {!blank(item.output) && (
          <>
            <h4>Result</h4>
            <pre>{display(item.output)}</pre>
          </>
        )}
      </details>
    );
  }
  return (
    <details className={`agent-detail agent-activity ${item.status || ""}`}>
      <summary>{item.text || label(item.name) || label(item.kind)}</summary>
      {!blank(item.output) && <pre>{display(item.output)}</pre>}
    </details>
  );
}
function Interaction({
  request,
  onRespond,
}: {
  request: AgentInteraction;
  onRespond: (response: Record<string, unknown>) => Promise<void>;
}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const kind = String(request.kind || request.type || "").replace(
    /\.request$/,
    "",
  );
  const payload = (request.input || request) as Record<string, unknown>;
  const submit = async (
    responses: Record<string, unknown> | Record<string, unknown>[],
  ) => {
    setBusy(true);
    setError("");
    try {
      for (const response of Array.isArray(responses) ? responses : [responses])
        await onRespond(response);
      setValue("");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const questions = Array.isArray(payload.questions)
    ? (payload.questions as Record<string, unknown>[])
    : [];
  const supported = ["approval", "clarify", "secret", "sudo"].includes(kind);
  return (
    <section className="agent-interaction">
      <strong>
        {kind === "approval"
          ? "Approval needed"
          : kind === "clarify"
            ? "Your input is needed"
            : kind === "secret" || kind === "sudo"
              ? "Private input required"
              : "Action required"}
      </strong>
      <p>
        {display(
          payload.question ||
            payload.description ||
            payload.prompt ||
            payload.text ||
            payload.command,
        )}
      </p>
      {kind === "approval" ? (
        <div className="agent-actions">
          {["once", "session", "always", "deny"]
            .filter((c) =>
              Array.isArray(payload.choices)
                ? payload.choices.includes(c)
                : c !== "always",
            )
            .map((c) => (
              <button
                disabled={busy}
                key={c}
                onClick={() => void submit({ choice: c })}
              >
                {c === "once"
                  ? "Allow once"
                  : c === "session"
                    ? "Allow for session"
                    : c === "always"
                      ? "Always allow"
                      : "Deny"}
              </button>
            ))}
        </div>
      ) : supported ? (
        <form
          onSubmit={async (e) => {
            e.preventDefault();
            if (questions.length) {
              await submit(
                questions.map((q) => ({
                  answer: answers[String(q.qid)] || "",
                  questionId: String(q.qid),
                })),
              );
            } else
              await submit({
                [kind === "sudo"
                  ? "password"
                  : kind === "secret"
                    ? "value"
                    : "answer"]: value,
              });
          }}
        >
          {questions.length ? (
            questions.map((q) => (
              <label key={String(q.qid)}>
                {display(q.question || q.text)}
                <input
                  disabled={busy}
                  value={answers[String(q.qid)] || ""}
                  onChange={(e) =>
                    setAnswers((a) => ({
                      ...a,
                      [String(q.qid)]: e.target.value,
                    }))
                  }
                  required
                />
                {Array.isArray(q.choices) && (
                  <small>{q.choices.map(display).join(" · ")}</small>
                )}
              </label>
            ))
          ) : (
            <>
              <input
                aria-label="Response"
                type={
                  kind === "sudo" || kind === "secret" ? "password" : "text"
                }
                autoComplete="off"
                value={value}
                onChange={(e) => setValue(e.target.value)}
                required
              />
              {Array.isArray(payload.choices) && (
                <div className="agent-actions">
                  {payload.choices.map((c, i) => (
                    <button
                      type="button"
                      key={i}
                      onClick={() => setValue(display(c))}
                    >
                      {display(c)}
                    </button>
                  ))}
                </div>
              )}
            </>
          )}
          <button disabled={busy} type="submit">
            Send response
          </button>
        </form>
      ) : (
        <p>
          This request needs an integration that is not connected yet. Stop the
          turn to cancel it.
        </p>
      )}
      {error && (
        <p role="alert" className="agent-error">
          {error}
        </p>
      )}
    </section>
  );
}

export function AgentsView({ slot }: { slot: HTMLElement | null }) {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const composerInput = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const closeTools = (event: Event) => {
      const escape = event instanceof KeyboardEvent && event.key === "Escape";
      if (event instanceof KeyboardEvent && !escape) return;
      document
        .querySelectorAll<HTMLDetailsElement>(".composer-tools[open]")
        .forEach((menu) => {
          if (escape || !menu.contains(event.target as Node)) menu.open = false;
        });
      if (escape) setSettingsOpen(false);
    };
    document.addEventListener("pointerdown", closeTools);
    document.addEventListener("keydown", closeTools);
    return () => {
      document.removeEventListener("pointerdown", closeTools);
      document.removeEventListener("keydown", closeTools);
    };
  }, []);
  const [providers, setProvidersState] = useState<AgentProvider[]>(
    viewMemory.providers,
  );
  const setProviders = (rows: AgentProvider[]) => {
    viewMemory.providers = rows;
    setProvidersState(rows);
  };
  const [providerId, setProviderId] = useState(viewMemory.providerId);
  const [agents, setAgentsState] = useState<AgentIdentity[]>(viewMemory.agents);
  const setAgents = (rows: AgentIdentity[]) => {
    viewMemory.agents = rows;
    setAgentsState(rows);
  };
  const [agentId, setAgentId] = useState(viewMemory.agentId);
  const [tabs, setTabs] = useState<Tab[]>(savedTabs);
  const [active, setActive] = useState(viewMemory.active);
  const [snapshots, setSnapshots] = useState<Record<string, AgentSnapshot>>({});
  const [drafts, setDrafts] = useState<Record<string, string>>(
    viewMemory.drafts,
  );
  useLayoutEffect(() => {
    const input = composerInput.current;
    if (input) {
      input.style.height = "0px";
      input.style.height = `${Math.min(160, Math.max(28, input.scrollHeight))}px`;
    }
  }, [active, drafts]);
  const [recents, setRecents] = useState<AgentConversation[]>([]);
  const {
    files: attachments,
    loading: attachmentLoading,
    errors: attachmentErrors,
  } = useAttachmentDrafts();
  const [nextOffset, setNextOffset] = useState<number | null>(null);
  const [searchLimited, setSearchLimited] = useState(false);
  const [historyBusy, setHistoryBusy] = useState(false);
  const historyGeneration = useRef(0);
  const [picker, setPicker] = useState(false);
  const [search, setSearch] = useState("");
  const [archived, setArchived] = useState(false);
  const [addon, setAddon] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [rename, setRename] = useState<string | null>(null);
  const [newAgent, setNewAgent] = useState(false);
  const transcript = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const scrollAnchor = useRef<{
    active: string;
    firstId?: string;
    height: number;
    top: number;
  } | null>(null);
  const generation = useRef(0);
  const tab = tabs.find(
    (t) =>
      identity(t) === active &&
      t.agentId === agentId &&
      t.providerId === providerId,
  );
  const snapshot = tab ? snapshots[active] : undefined;
  const agent = agents.find((a) => a.id === agentId);
  const provider = providers.find((p) => p.id === providerId);
  const call = <T,>(operation: string, input: Record<string, unknown> = {}) => {
    if (!window.bridge)
      return Promise.reject(
        new Error("Open sushiAI Desktop to connect your agents."),
      );
    return window.bridge.agentCall<T>(providerId, operation, {
      agentId,
      ...input,
    });
  };
  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await fn();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const adopt = (p: string, s: AgentSnapshot) => {
    const t = {
      providerId: p,
      agentId: s.agentId,
      conversationId: s.conversationId,
      title: s.title,
    };
    const id = identity(t);
    setTabs((old) =>
      old.some((v) => identity(v) === id)
        ? old.map((v) => (identity(v) === id ? t : v))
        : [...old, t],
    );
    setSnapshots((old) => ({ ...old, [id]: s }));
    setActive(id);
    setAgentId(s.agentId);
    setAddon("");
    setPicker(false);
    following.current = true;
  };
  useEffect(() => {
    if (!window.bridge) return;
    window.bridge
      .agentProviders()
      .then((p) => {
        setProviders(p);
        setProviderId((old) => old || p[0]?.id || "");
      })
      .catch((e) => setError(errorText(e)));
    return window.bridge.onAgents((event) => {
      if (event.type !== "conversation") return;
      const s = event as unknown as AgentSnapshot;
      const id = identity({
        providerId: event.providerId,
        agentId: s.agentId,
        conversationId: s.conversationId,
        title: s.title,
      });
      setSnapshots((old) => ({ ...old, [id]: s }));
      setTabs((old) =>
        old.map((t) => (identity(t) === id ? { ...t, title: s.title } : t)),
      );
    });
  }, []);
  useEffect(() => {
    if (!providerId) return;
    const gen = ++generation.current;
    setBusy(true);
    setError("");
    window.bridge
      ?.agentCall<AgentIdentity[]>(providerId, "agents.list")
      .then((rows) => {
        if (gen !== generation.current) return;
        setAgents(rows);
        const selected = rows.some((a) => a.id === viewMemory.agentId)
          ? viewMemory.agentId
          : rows[0]?.id || "";
        setAgentId(selected);
        const previous = savedTabs().find(
          (t) =>
            identity(t) === viewMemory.active &&
            t.providerId === providerId &&
            t.agentId === selected,
        );
        if (previous)
          return (
            window
              .bridge!.agentCall<AgentSnapshot>(
                providerId,
                "conversations.open",
                {
                  agentId: selected,
                  conversationId: previous.conversationId,
                  title: previous.title,
                },
              )
              .then((snapshot) => {
                if (gen === generation.current) adopt(providerId, snapshot);
              })
              // A conversation the user opened but never wrote in is not stored,
              // so reopening it fails; drop the tab instead of blocking startup.
              .catch(() => {
                if (gen !== generation.current) return;
                setTabs((old) =>
                  old.filter((t) => identity(t) !== identity(previous)),
                );
              })
          );
      })
      .catch((e) => setError(errorText(e)))
      .finally(() => {
        if (gen === generation.current) setBusy(false);
      });
  }, [providerId]);
  useEffect(() => {
    try {
      localStorage.setItem(storage, JSON.stringify(tabs.slice(-30)));
    } catch {
      /* The current window remains usable without storage. */
    }
  }, [tabs]);
  useEffect(() => {
    Object.assign(viewMemory, { providerId, agentId, active, drafts });
    try {
      localStorage.setItem(
        focusStorage,
        JSON.stringify({ providerId, agentId, active }),
      );
    } catch {
      /* The current window remains usable without storage. */
    }
  }, [providerId, agentId, active, drafts]);
  useLayoutEffect(() => {
    const el = transcript.current,
      anchor = scrollAnchor.current;
    if (!el) return;
    if (
      anchor &&
      anchor.active === active &&
      anchor.firstId !== snapshot?.items[0]?.id
    ) {
      el.scrollTop = anchor.top + el.scrollHeight - anchor.height;
      scrollAnchor.current = null;
    } else if (following.current) el.scrollTop = el.scrollHeight;
  }, [snapshot?.items, snapshot?.requests, active]);
  const openAgent = (id: string) =>
    void run(async () => {
      setAgentId(id);
      const s = await call<AgentSnapshot>("conversations.canonical", {
        agentId: id,
      });
      adopt(providerId, s);
    });
  const create = () =>
    void run(async () =>
      adopt(providerId, await call<AgentSnapshot>("conversations.create")),
    );
  const activate = (t: Tab) =>
    void run(async () => {
      setProviderId(t.providerId);
      setAgentId(t.agentId);
      const s = await window.bridge!.agentCall<AgentSnapshot>(
        t.providerId,
        "conversations.open",
        {
          agentId: t.agentId,
          conversationId: t.conversationId,
          title: t.title,
        },
      );
      adopt(t.providerId, s);
    });
  const closeTab = () => {
    if (!tab) return;
    const remaining = tabs.filter((t) => identity(t) !== active);
    setTabs(remaining);
    const next = remaining
      .filter((t) => t.agentId === agentId && t.providerId === providerId)
      .at(-1);
    setActive(next ? identity(next) : "");
    if (next) activate(next);
    setConfirmDelete(false);
    setRename(null);
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!e.metaKey) return;
      if (e.key === "t") {
        e.preventDefault();
        if (agentId && !busy) create();
      }
      if (e.key === "w") {
        e.preventDefault();
        closeTab();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const list = () => {
    setPicker(true);
    setAddon("");
  };
  const loadRecents = async (offset = 0) => {
    const gen = ++historyGeneration.current;
    setHistoryBusy(true);
    try {
      const r = await call<ConversationPage>("conversations.list", {
        archived,
        offset,
        ...(search.trim() ? { search: search.trim() } : {}),
      });
      if (gen !== historyGeneration.current) return;
      setRecents((old) =>
        offset
          ? [
              ...new Map(
                [...old, ...r.conversations].map((row) => [row.id, row]),
              ).values(),
            ]
          : r.conversations,
      );
      setNextOffset(r.nextOffset ?? null);
      setSearchLimited(!!r.searchLimited);
    } catch (e) {
      if (gen === historyGeneration.current) setError(errorText(e));
    } finally {
      if (gen === historyGeneration.current) setHistoryBusy(false);
    }
  };
  useEffect(() => {
    if (!picker) return;
    const timer = setTimeout(() => void loadRecents(), search ? 250 : 0);
    return () => {
      clearTimeout(timer);
      historyGeneration.current++;
    };
  }, [picker, search, archived, providerId, agentId]);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (
      !tab ||
      busy ||
      attachmentIsLoading(active) ||
      attachmentLoading[active] ||
      (!drafts[active]?.trim() && !attachments[active]?.length)
    )
      return;
    const message = drafts[active] || "";
    const files = attachments[active] || [];
    document
      .querySelectorAll<HTMLDetailsElement>(".composer-tools[open]")
      .forEach((menu) => {
        menu.open = false;
      });
    const target = tab;
    void run(async () => {
      await window.bridge!.agentCall(target.providerId, "conversations.send", {
        agentId: target.agentId,
        conversationId: target.conversationId,
        text: message,
        attachments: files,
      });
      setDrafts((old) => ({ ...old, [identity(target)]: "" }));
      setAttachmentFiles(identity(target), []);
    });
  };
  const inProgress =
    snapshot && ["running", "sending", "waiting"].includes(snapshot.status);
  const addAttachments = async (selected: File[]) => {
    const target = active;
    if (
      !selected.length ||
      !tab ||
      !provider?.capabilities.includes("attachments")
    )
      return;
    if (busy || inProgress) {
      setAttachmentError(
        target,
        "Wait for the current operation before attaching files.",
      );
      return;
    }
    await addAttachmentFiles(target, selected);
  };
  return (
    <div className="agents-view">
      {slot &&
        createPortal(
        <div className="agents-roster">
          <div className="section-label">
            <span>Agents</span>
            <span className="section-actions">
              <button
                className="icon-button"
                title="Add agent"
                aria-label="Add agent"
                disabled={busy}
                onClick={() => setNewAgent(true)}
              >
                <Plus size={14} />
              </button>
              <button
                className="icon-button"
                title="Refresh agents"
                aria-label="Refresh agents"
                disabled={busy}
                onClick={() =>
                  void run(async () =>
                    setAgents(await call<AgentIdentity[]>("agents.list")),
                  )
                }
              >
                <RefreshCw size={13} />
              </button>
            </span>
          </div>
          {providers.length > 1 && (
            <select
              aria-label="Agent provider"
              value={providerId}
              onChange={(e) => setProviderId(e.target.value)}
            >
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          )}
          {/* The open agent always leads the list; its colour stays tied to
              its original position so reordering does not recolour it. */}
          {[...agents]
            .sort(
              (a, b) => Number(b.id === agentId) - Number(a.id === agentId),
            )
            .map((a) => (
            <button
              key={a.id}
              className={`agent-roster-row ${a.id === agentId ? "selected" : ""}`}
              aria-busy={busy}
              onClick={() => openAgent(a.id)}
            >
              <span className={`agent-avatar color-${agents.indexOf(a) % 5}`}>
                <span
                  className="agent-face"
                  data-state={a.id === agentId ? snapshot?.status : "idle"}
                  aria-hidden="true"
                >
                  <i />
                  <i />
                </span>
              </span>
              <span>
                <strong>{a.name}</strong>
                <small>{a.model || a.description || provider?.name}</small>
              </span>
            </button>
          ))}
          {!agents.length && (
            <p className="agent-muted">
              {busy ? "Connecting to your agents…" : "No agents connected."}
            </p>
          )}
        </div>,
        slot,
        )}
      <section className="agents-main">
        <header className="agent-header">
          <div>
            <strong>{agent?.name || "Your agents"}</strong>
            <small>
              {agent?.model
                ? `${provider?.name} · ${agent.model}`
                : "Choose an agent to continue"}
            </small>
          </div>
          <span className={`agent-status ${snapshot?.status || "idle"}`}>
            {busy ? "Connecting…" : snapshot?.status || "Ready"}
          </span>
          {snapshot?.status === "error" && (
            <button
              disabled={busy}
              onClick={() =>
                void run(async () => {
                  await call("connection.recover");
                  if (tab)
                    await window.bridge!.agentCall(
                      providerId,
                      "conversations.open",
                      { agentId, conversationId: tab.conversationId },
                    );
                })
              }
            >
              Reconnect
            </button>
          )}
          <button
            onClick={() => {
              setAddon((old) => (old === "activity" ? "" : "activity"));
              setPicker(false);
            }}
            title="Agent activity"
          >
            Activity
          </button>
          <button
            disabled={!agentId || busy}
            onClick={list}
            title="Conversation history"
          >
            History
          </button>
          <button
            aria-label="Agent settings"
            title="Settings"
            aria-expanded={settingsOpen}
            onClick={() => setSettingsOpen(!settingsOpen)}
          >
            ⚙
          </button>
        </header>
        <nav
          style={{ display: settingsOpen ? undefined : "none" }}
          className="agent-addon-nav"
          aria-label="Agent resources"
        >
          {provider?.addons.map((a) => (
            <button
              key={a.id}
              className={addon === a.id ? "active" : ""}
              onClick={() => setAddon((old) => (old === a.id ? "" : a.id))}
            >
              {a.name || String(a.title || a.id)}
            </button>
          ))}
        </nav>
        <div
          className="agent-tabs"
          role="tablist"
          aria-label="Agent conversations"
        >
          {tabs
            .filter((t) => t.providerId === providerId && t.agentId === agentId)
            .map((t) => (
              <div
                className={`agent-tab ${identity(t) === active ? "active" : ""}`}
                key={identity(t)}
              >
                <button
                  role="tab"
                  aria-selected={identity(t) === active}
                  tabIndex={identity(t) === active ? 0 : -1}
                  onKeyDown={(e) => {
                    const visible = tabs.filter(
                      (x) =>
                        x.providerId === providerId && x.agentId === agentId,
                    );
                    const index = visible.findIndex(
                      (x) => identity(x) === identity(t),
                    );
                    const destination =
                      e.key === "ArrowRight"
                        ? (index + 1) % visible.length
                        : e.key === "ArrowLeft"
                          ? (index + visible.length - 1) % visible.length
                          : e.key === "Home"
                            ? 0
                            : e.key === "End"
                              ? visible.length - 1
                              : -1;
                    if (destination < 0) return;
                    e.preventDefault();
                    const buttons = e.currentTarget
                      .closest("[role=tablist]")
                      ?.querySelectorAll<HTMLButtonElement>("[role=tab]");
                    buttons?.[destination]?.focus();
                    activate(visible[destination]);
                  }}
                  onClick={() => activate(t)}
                >
                  {t.title}
                </button>
                <button
                  aria-label={`Close ${t.title} tab`}
                  onClick={() => {
                    if (identity(t) === active) closeTab();
                    else
                      setTabs((old) =>
                        old.filter((x) => identity(x) !== identity(t)),
                      );
                  }}
                >
                  ×
                </button>
              </div>
            ))}
          <button
            aria-label="New conversation"
            disabled={!agentId || busy}
            onClick={create}
          >
            ＋
          </button>
        </div>
        {error && (
          <div className="agent-error" role="alert">
            {error}
            <button onClick={() => setError("")} aria-label="Dismiss error">
              ×
            </button>
          </div>
        )}
        {newAgent ? (
          <section className="agent-addon-panel">
            <header>
              <h2>New agent</h2>
              <button onClick={() => setNewAgent(false)}>Cancel</button>
            </header>
            <form
              className="agent-resource-editor"
              onSubmit={(e) => {
                e.preventDefault();
                const data = new FormData(e.currentTarget);
                void run(async () => {
                  const created = await call<AgentIdentity>("agents.create", {
                    name: data.get("name"),
                    description: data.get("description"),
                    instructions: data.get("instructions"),
                    cloneFrom: agentId || undefined,
                  });
                  setAgents(await call<AgentIdentity[]>("agents.list"));
                  setAgentId(created.id);
                  setNewAgent(false);
                });
              }}
            >
              <label>
                Name
                <input
                  aria-label="New agent name"
                  name="name"
                  placeholder="release-assistant"
                  pattern="[a-zA-Z0-9][a-zA-Z0-9_-]*"
                  required
                />
              </label>
              <label>
                Description
                <input name="description" />
              </label>
              <label>
                Instructions
                <textarea
                  name="instructions"
                  placeholder="Describe this agent's responsibilities."
                />
              </label>
              <p className="agent-muted">
                Uses {agent?.name || "the default agent"}'s model, connections
                and skills. Conversation history is not copied.
              </p>
              <button disabled={busy}>Create agent</button>
            </form>
          </section>
        ) : addon === "activity" ? (
          <AgentActivityPanel
            key={providerId}
            providerId={providerId}
            onClose={() => setAddon("")}
            onOpen={(a) =>
              activate({
                providerId,
                agentId: a.agentId,
                conversationId: a.conversationId,
                title: a.conversationTitle,
              })
            }
          />
        ) : addon === "mcp" && agent && provider ? (
          <AgentConnectionsPanel
            key={`${providerId}/${agentId}`}
            providerId={providerId}
            agentId={agentId}
            onClose={() => setAddon("")}
          />
        ) : addon === "git" && agent && provider ? (
          <AgentGitPanel
            key={`${providerId}/${agentId}`}
            providerId={providerId}
            agentId={agentId}
            onClose={() => setAddon("")}
          />
        ) : addon === "files" && agent && provider ? (
          <AgentFilesPanel
            key={`${providerId}/${agentId}`}
            providerId={providerId}
            agentId={agentId}
            onClose={() => setAddon("")}
          />
        ) : addon === "recovery" && agent && provider ? (
          <AgentRecoveryPanel
            key={`${providerId}/${agentId}`}
            providerId={providerId}
            agentId={agentId}
            onClose={() => setAddon("")}
          />
        ) : addon && agent && provider ? (
          <AgentAddonPanel
            key={`${providerId}/${agentId}/${addon}`}
            providerId={providerId}
            agentId={agentId}
            addonId={addon}
            onOpenConversation={(id, title) =>
              activate({ providerId, agentId, conversationId: id, title })
            }
            onClose={() => setAddon("")}
          />
        ) : picker ? (
          <div className="agent-history">
            <div className="agent-actions">
              <input
                aria-label="Search conversations"
                placeholder="Search conversations…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <label>
                <input
                  type="checkbox"
                  checked={archived}
                  disabled={!!search.trim()}
                  onChange={(e) => setArchived(e.target.checked)}
                />
                Archived
              </label>
              <button onClick={() => setPicker(false)}>Close</button>
            </div>
            {search.trim() && (
              <p className="agent-muted">
                Searches message content and IDs across active and archived
                conversations.
              </p>
            )}
            {historyBusy && <p role="status">Searching conversations…</p>}
            {!historyBusy && !recents.length && (
              <p className="agent-muted">No conversations found.</p>
            )}
            {recents.map((r) => (
              <div className="agent-history-entry" key={r.id}>
                <button
                  className="agent-history-row"
                  onClick={() =>
                    activate({
                      providerId,
                      agentId,
                      conversationId: r.id,
                      title: r.title,
                    })
                  }
                >
                  <strong>{r.title}</strong>
                  <small>
                    {r.messageCount} messages · {r.model}
                    {r.archived ? " · Archived" : ""}
                  </small>
                </button>
                <button
                  disabled={busy}
                  title={r.pinned ? "Unpin conversation" : "Pin conversation"}
                  onClick={() =>
                    void run(async () => {
                      await call("conversations.update", {
                        conversationId: r.id,
                        patch: { pinned: !r.pinned },
                      });
                      await loadRecents();
                    })
                  }
                >
                  {r.pinned ? "Unpin" : "Pin"}
                </button>
                {r.archived && (
                  <button
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        await call("conversations.update", {
                          conversationId: r.id,
                          patch: { archived: false },
                        });
                        await loadRecents();
                      })
                    }
                  >
                    Restore
                  </button>
                )}
              </div>
            ))}
            {nextOffset !== null && (
              <button
                disabled={historyBusy}
                onClick={() => void loadRecents(nextOffset)}
              >
                Load more conversations
              </button>
            )}
            {searchLimited && (
              <p className="agent-muted">
                Showing the first 100 matches. Refine your search to find a
                specific conversation.
              </p>
            )}
          </div>
        ) : (
          <>
            <div
              className="agent-transcript"
              ref={transcript}
              onScroll={() => {
                const el = transcript.current;
                if (el)
                  following.current =
                    el.scrollHeight - el.scrollTop - el.clientHeight < 100;
              }}
            >
              <div className="agent-transcript-content">
                {snapshot?.hasEarlier && (
                  <button
                    disabled={busy}
                    onClick={() =>
                      void run(async () => {
                        const el = transcript.current;
                        if (el) {
                          following.current = false;
                          scrollAnchor.current = {
                            active,
                            firstId: snapshot?.items[0]?.id,
                            height: el.scrollHeight,
                            top: el.scrollTop,
                          };
                        }
                        const s = await call<AgentSnapshot>(
                          "conversations.history",
                          { conversationId: tab?.conversationId },
                        );
                        setSnapshots((old) => ({ ...old, [active]: s }));
                      })
                    }
                  >
                    Load earlier messages
                  </button>
                )}
                {groupItems((snapshot?.items || []).filter(visible)).map(
                  (item) =>
                    Array.isArray(item) ? (
                      <ToolGroup key={item[0].id} items={item} />
                    ) : item.kind === "image" && tab ? (
                      provider?.capabilities.includes("image-preview") ? (
                        <AgentImage
                          key={item.id}
                          providerId={tab.providerId}
                          agentId={tab.agentId}
                          conversationId={tab.conversationId}
                          itemId={item.id}
                          name={item.name || "Attached image"}
                        />
                      ) : (
                        <p key={item.id}>Image: {item.name || "attachment"}</p>
                      )
                    ) : (
                      <TranscriptItem key={item.id} item={item} />
                    ),
                )}
                {!snapshot && (
                  <div className="agent-welcome">
                    <span>🍣</span>
                    <h2>A workspace for your agents.</h2>
                    <p>
                      Open an agent's main conversation or start a new thread.
                    </p>
                    {agentId && (
                      <button
                        disabled={busy}
                        onClick={() => openAgent(agentId)}
                      >
                        Open conversation
                      </button>
                    )}
                  </div>
                )}
                {snapshot?.requests.map((r, i) => (
                  <Interaction
                    key={`${active}/${r.id || i}`}
                    request={r}
                    onRespond={(response) =>
                      call("interactions.respond", {
                        agentId: tab?.agentId,
                        conversationId: tab?.conversationId,
                        requestId: r.id || r.request_id,
                        response,
                      })
                        .then(() => {})
                        .catch((e) => {
                          setError(errorText(e));
                          throw e;
                        })
                    }
                  />
                ))}
              </div>
            </div>
            {tab && (
              <div className="agent-composer-area">
                <div
                  className="agent-conversation-actions"
                  style={{ display: settingsOpen ? undefined : "none" }}
                >
                  <button onClick={() => setRename(tab.title)}>Rename</button>
                  <button
                    disabled={busy || !!inProgress}
                    onClick={() =>
                      void run(async () => {
                        await call("conversations.update", {
                          agentId: tab.agentId,
                          conversationId: tab.conversationId,
                          patch: { archived: true },
                        });
                        closeTab();
                      })
                    }
                  >
                    Archive
                  </button>
                  <button
                    disabled={busy || !!inProgress}
                    onClick={() => setConfirmDelete(true)}
                  >
                    Delete
                  </button>
                </div>
                {settingsOpen &&
                  provider?.capabilities.includes("conversation-settings") && (
                    <AgentConversationSettings
                      key={active}
                      providerId={providerId}
                      agentId={tab.agentId}
                      conversationId={tab.conversationId}
                      disabled={!!inProgress}
                    />
                  )}
                {rename !== null && (
                  <form
                    className="agent-actions"
                    onSubmit={(e) => {
                      e.preventDefault();
                      void run(async () => {
                        await call("conversations.update", {
                          agentId: tab.agentId,
                          conversationId: tab.conversationId,
                          patch: { title: rename },
                        });
                        setRename(null);
                      });
                    }}
                  >
                    <input
                      aria-label="Conversation title"
                      value={rename}
                      onChange={(e) => setRename(e.target.value)}
                      required
                    />
                    <button>Save title</button>
                    <button type="button" onClick={() => setRename(null)}>
                      Cancel
                    </button>
                  </form>
                )}
                {confirmDelete && (
                  <div className="agent-delete-confirm">
                    Delete this conversation and its history?
                    <button
                      disabled={busy}
                      onClick={() =>
                        void run(async () => {
                          await call("conversations.delete", {
                            agentId: tab.agentId,
                            conversationId: tab.conversationId,
                          });
                          closeTab();
                        })
                      }
                    >
                      Delete conversation
                    </button>
                    <button onClick={() => setConfirmDelete(false)}>
                      Cancel
                    </button>
                  </div>
                )}
                <form
                  className="agent-composer"
                  onSubmit={submit}
                  onDragOver={(event) => {
                    if (event.dataTransfer.types.includes("Files")) {
                      event.preventDefault();
                      event.dataTransfer.dropEffect =
                        busy || inProgress ? "none" : "copy";
                    }
                  }}
                  onDrop={(event) => {
                    if (!event.dataTransfer.types.includes("Files")) return;
                    event.preventDefault();
                    void addAttachments(Array.from(event.dataTransfer.files));
                  }}
                  onPaste={(event) => {
                    if (!provider?.capabilities.includes("attachments")) return;
                    const files = Array.from(event.clipboardData.files);
                    if (!files.length) return;
                    event.preventDefault();
                    void addAttachments(files);
                  }}
                >
                  <textarea
                    ref={composerInput}
                    rows={1}
                    aria-label="Message agent"
                    placeholder="Ask anything…"
                    value={drafts[active] || ""}
                    onChange={(e) =>
                      setDrafts((old) => ({ ...old, [active]: e.target.value }))
                    }
                    onKeyDown={(e) => {
                      if (
                        e.key === "Enter" &&
                        !e.shiftKey &&
                        !e.nativeEvent.isComposing
                      ) {
                        e.preventDefault();
                        if (
                          !busy &&
                          !inProgress &&
                          !attachmentLoading[active] &&
                          (drafts[active]?.trim() ||
                            attachments[active]?.length)
                        )
                          submit(e);
                      }
                    }}
                  />
                  <footer>
                    <details className="composer-tools">
                      <summary
                        aria-label="Message tools"
                        title="Attach files or run a skill"
                      >
                        +
                        {attachments[active]?.length ? (
                          <small>{attachments[active].length}</small>
                        ) : null}
                      </summary>
                      <div className="composer-tools-popover">
                        {provider?.capabilities.includes("skill-commands") && (
                          <AgentSkillPicker
                            key={active}
                            providerId={tab.providerId}
                            agentId={tab.agentId}
                            conversationId={tab.conversationId}
                            disabled={
                              busy ||
                              Boolean(inProgress) ||
                              attachmentLoading[active] ||
                              Boolean(attachments[active]?.length)
                            }
                            onRun={async (command, args) => {
                              await window.bridge!.agentCall(
                                tab.providerId,
                                "conversations.runSkill",
                                {
                                  agentId: tab.agentId,
                                  conversationId: tab.conversationId,
                                  command,
                                  arguments: args,
                                },
                              );
                            }}
                          />
                        )}
                        {provider?.capabilities.includes("attachments") && (
                          <AgentAttachments
                            key={active}
                            files={attachments[active] || []}
                            disabled={busy || Boolean(inProgress)}
                            loading={Boolean(attachmentLoading[active])}
                            error={attachmentErrors[active] || ""}
                            onAdd={(files) => void addAttachments(files)}
                            onChange={(files) =>
                              setAttachmentFiles(active, files)
                            }
                          />
                        )}
                      </div>
                    </details>
                    <span>
                      {String(
                        snapshot?.info.model || agent?.model || "Profile model",
                      )}
                    </span>
                    <span className="agent-usage">
                      {typeof snapshot?.usage.total_tokens === "number"
                        ? `${snapshot.usage.total_tokens.toLocaleString()} tokens`
                        : ""}
                    </span>
                    {inProgress ? (
                      <button
                        type="button"
                        onClick={() =>
                          void run(async () => {
                            await call("conversations.interrupt", {
                              agentId: tab.agentId,
                              conversationId: tab.conversationId,
                            });
                          })
                        }
                      >
                        Stop
                      </button>
                    ) : (
                      <button
                        type="submit"
                        aria-label="Send ↑"
                        disabled={
                          busy ||
                          attachmentLoading[active] ||
                          (!drafts[active]?.trim() &&
                            !attachments[active]?.length)
                        }
                      >
                        ↑
                      </button>
                    )}
                  </footer>
                </form>
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}
