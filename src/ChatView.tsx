import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  ArrowUp,
  Asterisk,
  AudioLines,
  ChevronDown,
  ChevronRight,
  FileText,
  FolderClosed,
  Image as ImageIcon,
  X,
  GitBranch,
  Gauge,
  PanelRight,
  Pencil,
  Pin,
  PinOff,
  Plus,
  Square,
  Trash2,
} from "lucide-react";
import type { ChatModels, Panel, Workspace } from "./types";
import {
  DEFAULT_TITLES,
  contextUsage,
  groupThreads,
  modelName as readableModel,
  relativeTime,
} from "./chat-threads";
import { RichText } from "./agents/AgentsView";
import "./ChatView.css";

// Models come from each CLI at runtime, so a new Codex release shows up here
// without a change to sushiAI. Reasoning levels belong to the chosen model.
const EFFORT_LABELS: Record<string, string> = {
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
  ultra: "Ultra",
};
const effortName = (id: string) => EFFORT_LABELS[id] || id;
const PERMISSIONS = [
  { id: "default", label: "Ask before edits" },
  { id: "acceptEdits", label: "Auto-accept edits" },
  { id: "plan", label: "Plan mode" },
  { id: "bypassPermissions", label: "Bypass permissions" },
];
const SUGGESTIONS = [
  "Explain this project",
  "Review recent changes",
  "Plan the next feature",
];
// Which thread was open survives a restart, the way the Agent tab remembers its
// conversation. Unsent drafts stay in memory and are never written to storage.
const focusStorage = "sushiai.chat-focus.v1";
function savedFocus(): string {
  try {
    const id = localStorage.getItem(focusStorage);
    return typeof id === "string" ? id : "";
  } catch {
    return "";
  }
}
const memory = {
  selected: savedFocus(),
  drafts: {} as Record<string, string>,
  attachments: {} as Record<string, string[]>,
};
function remember(id: string) {
  memory.selected = id;
  try {
    localStorage.setItem(focusStorage, id);
  } catch {
    /* a full store still leaves the thread open for this session */
  }
}
const basename = (p: string) => p.split("/").filter(Boolean).at(-1) || p;
const isImage = (p: string) => /\.(png|jpe?g|gif|webp)$/i.test(p);
const isFolder = (p: string) =>
  p.endsWith("/") || !/\.[^/]+$/.test(basename(p));
function Attachment({
  path,
  onRemove,
}: {
  path: string;
  onRemove?: () => void;
}) {
  const Icon = isImage(path)
    ? ImageIcon
    : isFolder(path)
      ? FolderClosed
      : FileText;
  return (
    <span className="attachment" title={path}>
      <Icon size={12} />
      <span>{basename(path)}</span>
      {onRemove && (
        <button aria-label={`Remove ${basename(path)}`} onClick={onRemove}>
          <X size={11} />
        </button>
      )}
    </span>
  );
}

export function ChatView({
  workspaces,
  active,
  slot,
  onSelectWorkspace,
  onNewThread,
  onSend,
  onCancel,
  onPatch,
  onDelete,
  onToggleSidebar,
}: {
  workspaces: Workspace[];
  active: Workspace;
  slot: HTMLElement | null;
  onSelectWorkspace(workspaceId: string): void;
  onNewThread(workspaceId: string): Panel;
  onSend(panel: Panel, text: string, attachments: string[]): void;
  onCancel(panelId: string): void;
  onPatch(panelId: string, patch: Partial<Panel>): void;
  onDelete(workspaceId: string, panel: Panel): void;
  onToggleSidebar(): void;
}) {
  const [selected, setSelected] = useState(memory.selected);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [branch, setBranch] = useState("");
  const [renaming, setRenaming] = useState("");
  const [, tick] = useState(0);
  const { pinned, groups } = groupThreads(workspaces);
  // Only an explicit choice opens a thread; otherwise the welcome screen shows.
  const current = workspaces
    .flatMap((w) => w.panels)
    .find((p) => p.id === selected && p.kind === "chat");
  const [draft, setDraft] = useState(memory.drafts[current?.id || ""] || "");
  const [attachments, setAttachments] = useState<string[]>(
    memory.attachments[current?.id || ""] || [],
  );
  const [dragging, setDragging] = useState(false);
  const [catalog, setCatalog] = useState<ChatModels>({});
  const textarea = useRef<HTMLTextAreaElement>(null);
  const end = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const timer = setInterval(() => tick((n) => n + 1), 60_000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!active.cwd || !window.bridge) return setBranch("");
    window.bridge
      .projectInspect(active.connection, { operation: "git", root: active.cwd })
      .then((r) => setBranch(r?.branch || ""))
      .catch(() => setBranch(""));
  }, [active.cwd, active.connection]);
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [current?.messages, current?.busy, current?.note]);
  useEffect(() => {
    window.bridge
      ?.chatModels()
      .then(setCatalog)
      .catch(() => setCatalog({}));
  }, []);

  function open(workspaceId: string, panelId: string) {
    memory.drafts[current?.id || ""] = draft;
    memory.attachments[current?.id || ""] = attachments;
    remember(panelId);
    setSelected(panelId);
    setDraft(memory.drafts[panelId] || "");
    setAttachments(memory.attachments[panelId] || []);
    if (workspaceId !== active.id) onSelectWorkspace(workspaceId);
    textarea.current?.focus();
  }
  function create(workspaceId: string) {
    const panel = onNewThread(workspaceId);
    open(workspaceId, panel.id);
    return panel;
  }
  function submit() {
    const text = draft.trim();
    if (!text || current?.busy) return;
    const panel = current || create(active.id);
    onSend(panel, text, attachments);
    setDraft("");
    setAttachments([]);
    memory.drafts[panel.id] = "";
    memory.attachments[panel.id] = [];
    if (textarea.current) textarea.current.style.height = "";
  }
  const attach = (paths: string[]) =>
    setAttachments((old) => [...new Set([...old, ...paths])].slice(0, 20));
  const agent = current?.agent || "claude";
  const provider = catalog[agent];
  const models = provider?.models || [];
  const chosen = models.find((m) => m.id === current?.model);
  const fallbackEffort = chosen?.defaultEffort || provider?.defaultEffort || "";
  const modelLabel =
    chosen?.label ||
    current?.model ||
    (provider?.defaultModel
      ? `Default · ${provider.defaultModel}`
      : "Default model");
  const effortLabel = current?.effort
    ? effortName(current.effort)
    : fallbackEffort
      ? `Default · ${effortName(fallbackEffort)}`
      : "Default effort";
  // A model that names its own levels wins; without one, the CLI's whole range.
  const efforts = chosen?.efforts?.length
    ? chosen.efforts
    : provider?.efforts || [];
  // The CLI's own number wins; the catalogue only fills in before a first turn.
  const contextLimit =
    current?.usage?.context ||
    chosen?.context ||
    provider?.defaultContext ||
    200_000;
  const usage = contextUsage(
    current?.messages,
    contextLimit,
    current?.usage ? current.usage.input + current.usage.output : undefined,
  );
  const modelName = (id: string) =>
    Object.values(catalog)
      .flatMap((p) => p.models)
      .find((m) => m.id === id)?.label || readableModel(id);
  // Naming every answer is noise on a single-model thread; it matters once the
  // thread has been answered by more than one.
  const answeredBy = new Set(
    (current?.messages || [])
      .filter((m) => m.role === "assistant" && m.model)
      .map((m) => m.model),
  );
  const permissionLabel = (
    PERMISSIONS.find((p) => p.id === (current?.permission || "default")) ||
    PERMISSIONS[0]
  ).label;
  const item = ({
    workspace,
    panel,
  }: {
    workspace: Workspace;
    panel: Panel;
  }) => (
    <div
      key={panel.id}
      className={`thread-item ${panel.id === current?.id ? "active" : ""} ${renaming === panel.id ? "renaming" : ""}`}
    >
      {renaming === panel.id ? (
        <input
          className="thread-rename"
          aria-label="Thread name"
          autoFocus
          defaultValue={panel.title}
          onBlur={(event) => {
            const title = event.target.value.trim();
            if (title) onPatch(panel.id, { title });
            setRenaming("");
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
            if (event.key === "Escape") {
              event.currentTarget.value = panel.title;
              event.currentTarget.blur();
            }
          }}
        />
      ) : (
        <button
          className="thread-open"
          onClick={() => open(workspace.id, panel.id)}
          onDoubleClick={() => setRenaming(panel.id)}
          title={panel.title}
        >
          <i className={`thread-dot ${panel.busy ? "busy" : ""}`} />
          <span>{panel.title}</span>
          <time>{relativeTime(panel.updatedAt)}</time>
        </button>
      )}
      <span className="thread-actions">
        <button
          aria-label="Rename thread"
          onClick={() => setRenaming(panel.id)}
        >
          <Pencil size={11} />
        </button>
        <button
          aria-label={panel.pinned ? "Unpin thread" : "Pin thread"}
          onClick={() => onPatch(panel.id, { pinned: !panel.pinned })}
        >
          {panel.pinned ? <PinOff size={11} /> : <Pin size={11} />}
        </button>
        <button
          aria-label="Delete thread"
          onClick={() => {
            if (!window.confirm(`Delete thread “${panel.title}”?`)) return;
            if (panel.id === selected) {
              remember("");
              setSelected("");
            }
            onDelete(workspace.id, panel);
          }}
        >
          <Trash2 size={11} />
        </button>
      </span>
    </div>
  );

  return (
    <div className="chat-view">
      {slot &&
        createPortal(
          <div className="threads">
            <div className="section-label">
              <span>Threads</span>
              {workspaces.length > 1 ? (
                <label
                  className="icon-button threads-add"
                  title="New thread in…"
                >
                  <Plus size={14} />
                  <select
                    aria-label="New thread in project"
                    value=""
                    onChange={(event) =>
                      event.target.value && create(event.target.value)
                    }
                  >
                    <option value="" disabled>
                      New thread in…
                    </option>
                    {workspaces.map((w) => (
                      <option key={w.id} value={w.id}>
                        {w.name}
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <button
                  className="icon-button"
                  aria-label="New thread"
                  title={`New thread in ${active.name}`}
                  onClick={() => create(active.id)}
                >
                  <Plus size={14} />
                </button>
              )}
            </div>
            <div className="threads-list">
              {pinned.length > 0 && (
                <section>
                  <h4>Pinned</h4>
                  {pinned.map(item)}
                </section>
              )}
              {groups.map(({ workspace, threads }) => (
                <section key={workspace.id}>
                  <div className="thread-group">
                    <button
                      className="thread-group-name"
                      aria-expanded={!collapsed.has(workspace.id)}
                      onClick={() =>
                        setCollapsed((set) => {
                          const next = new Set(set);
                          next.has(workspace.id)
                            ? next.delete(workspace.id)
                            : next.add(workspace.id);
                          return next;
                        })
                      }
                    >
                      <span>{workspace.name}</span>
                      {collapsed.has(workspace.id) ? (
                        <ChevronRight size={12} />
                      ) : (
                        <ChevronDown size={12} />
                      )}
                    </button>
                    <button
                      className="icon-button"
                      aria-label={`New thread in ${workspace.name}`}
                      onClick={() => create(workspace.id)}
                    >
                      <Plus size={12} />
                    </button>
                  </div>
                  {!collapsed.has(workspace.id) && threads.map(item)}
                </section>
              ))}
            </div>
          </div>,
          slot,
        )}
      <section className="chat-main">
        <header className="chat-head">
          <FolderClosed size={14} />
          <strong>{active.name}</strong>
          {branch && (
            <span className="branch">
              <GitBranch size={11} /> {branch}
            </span>
          )}
          {current && (
            <span className="chat-head-title" title={current.title}>
              {current.title}
            </span>
          )}
          <span className="chat-head-right">
            <img
              className="harness-icon"
              src={`./agents/${agent}.svg`}
              width={13}
              height={13}
              alt=""
            />
            <span>{modelLabel}</span>
            {usage && (
              <span className="chat-usage" title="Estimated context in use">
                {usage}
              </span>
            )}
            <i className={`thread-dot ${current?.busy ? "busy" : ""}`} />
            <button
              className="icon-button"
              aria-label="Toggle threads"
              title="Toggle threads · ⌘B"
              onClick={onToggleSidebar}
            >
              <PanelRight size={14} />
            </button>
          </span>
        </header>
        <div className="chat-scroll">
          {!current?.messages?.length ? (
            <div className="chat-welcome" key="welcome">
              <Asterisk size={34} strokeWidth={1.2} />
              <h2>What should we work on?</h2>
              <p>Give your agent a task. Keep the conversation here.</p>
              <div className="chat-suggestions">
                {SUGGESTIONS.map((text) => (
                  <button
                    key={text}
                    onClick={() => {
                      setDraft(text);
                      textarea.current?.focus();
                    }}
                  >
                    {text}
                  </button>
                ))}
              </div>
            </div>
          ) : (
            <div className="chat-column" key={current.id}>
              {current.messages.map((message) => (
                <div key={message.id} className={`message ${message.role}`}>
                  {message.role === "assistant" ? (
                    message.text ? (
                      <>
                        {message.model && answeredBy.size > 1 && (
                          <span className="message-model">
                            {modelName(message.model)}
                          </span>
                        )}
                        <RichText text={message.text} />
                      </>
                    ) : null
                  ) : (
                    <>
                      {message.text}
                      {message.attachments?.length ? (
                        <div className="chat-attachments">
                          {message.attachments.map((path) => (
                            <Attachment key={path} path={path} />
                          ))}
                        </div>
                      ) : null}
                    </>
                  )}
                </div>
              ))}
              {/* Codex often answers, then keeps working; progress belongs
                  under the thread, not inside an empty bubble. */}
              {current.busy && (
                <div className="chat-progress">
                  <i />
                  <span className="thinking">
                    {current.note || "Thinking"}
                    <span>…</span>
                  </span>
                </div>
              )}
              {current.error && (
                <div className="chat-error" role="alert">
                  {current.error}
                </div>
              )}
              <div ref={end} />
            </div>
          )}
        </div>
        <div
          className={`chat-composer ${dragging ? "dragging" : ""}`}
          onDragOver={(event) => {
            event.preventDefault();
            setDragging(true);
          }}
          onDragLeave={() => setDragging(false)}
          onDrop={(event) => {
            event.preventDefault();
            setDragging(false);
            const bridge = window.bridge;
            if (!bridge) return;
            attach([...event.dataTransfer.files].map(bridge.pathForFile));
          }}
        >
          {attachments.length > 0 && (
            <div className="chat-attachments">
              {attachments.map((path) => (
                <Attachment
                  key={path}
                  path={path}
                  onRemove={() =>
                    setAttachments((old) => old.filter((p) => p !== path))
                  }
                />
              ))}
            </div>
          )}
          <textarea
            ref={textarea}
            rows={1}
            aria-label="Message your agent"
            placeholder="Ask about your code — add files and folders with + or drop them here"
            value={draft}
            onChange={(event) => {
              setDraft(event.target.value);
              const el = event.target;
              el.style.height = "";
              el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
            }}
            onKeyDown={(event) => {
              if (
                event.key === "Enter" &&
                !event.shiftKey &&
                !event.nativeEvent.isComposing
              ) {
                event.preventDefault();
                submit();
              }
            }}
          />
          <div className="chat-toolbar">
            <button
              className="chip add"
              aria-label="Add files or folders"
              title={
                window.bridge
                  ? "Add files or folders"
                  : "Attachments need the desktop app"
              }
              disabled={!window.bridge}
              onClick={() =>
                window.bridge
                  ?.chooseAttachments()
                  .then(attach)
                  .catch(() => {})
              }
            >
              <Plus size={14} />
            </button>
            <i className="chat-sep" />
            <label className="chip">
              <img
                className="harness-icon"
                src={`./agents/${agent}.svg`}
                width={12}
                height={12}
                alt=""
              />
              <span>{agent === "codex" ? "Codex" : "Claude"}</span>
              <ChevronDown size={11} />
              <select
                aria-label="Agent"
                disabled={current?.busy}
                value={agent}
                onChange={(event) =>
                  onPatch((current || create(active.id)).id, {
                    agent: event.target.value,
                    model: "",
                    effort: "",
                  })
                }
              >
                <option value="claude">Claude</option>
                <option value="codex">Codex</option>
              </select>
            </label>
            <label className="chip">
              <span>{modelLabel}</span>
              <ChevronDown size={11} />
              <select
                aria-label="Model"
                disabled={current?.busy}
                value={current?.model || ""}
                onChange={(event) => {
                  const next = models.find((m) => m.id === event.target.value);
                  onPatch((current || create(active.id)).id, {
                    model: event.target.value,
                    // Keep the level only when the new model offers it.
                    effort:
                      current?.effort && next?.efforts.includes(current.effort)
                        ? current.effort
                        : "",
                  });
                }}
              >
                <option value="">
                  {provider?.defaultModel
                    ? `Default · ${provider.defaultModel}`
                    : "Default model"}
                </option>
                {models.map((m) => (
                  <option key={m.id} value={m.id} title={m.description}>
                    {m.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="chip">
              <Gauge size={12} />
              <span>{effortLabel}</span>
              <ChevronDown size={11} />
              <select
                aria-label="Effort"
                disabled={current?.busy}
                value={current?.effort || ""}
                onChange={(event) =>
                  onPatch((current || create(active.id)).id, {
                    effort: event.target.value,
                  })
                }
              >
                <option value="">
                  {fallbackEffort
                    ? `Default · ${effortName(fallbackEffort)}`
                    : "Default effort"}
                </option>
                {efforts.map((id) => (
                  <option key={id} value={id}>
                    {effortName(id)}
                  </option>
                ))}
              </select>
            </label>
            <i className="chat-sep" />
            <label className="chip">
              <Pencil size={12} />
              <span>{permissionLabel}</span>
              <ChevronDown size={11} />
              <select
                aria-label="Permission mode"
                disabled={current?.busy}
                value={current?.permission || "default"}
                onChange={(event) =>
                  onPatch((current || create(active.id)).id, {
                    permission: event.target.value,
                  })
                }
              >
                {PERMISSIONS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </select>
            </label>
            <span className="chat-toolbar-right">
              {usage && <span className="chat-usage">{usage}</span>}
              <button
                className="voice"
                disabled
                title="Voice input isn't wired up yet"
                aria-label="Voice input"
              >
                <AudioLines size={15} />
              </button>
              {current?.busy ? (
                <button
                  className="send enabled"
                  title="Stop response"
                  aria-label="Stop response"
                  onClick={() => onCancel(current.id)}
                >
                  <Square size={12} />
                </button>
              ) : (
                <button
                  className={`send ${draft.trim() ? "enabled" : ""}`}
                  aria-label="Send message"
                  disabled={!draft.trim()}
                  onClick={submit}
                >
                  <ArrowUp size={16} />
                </button>
              )}
            </span>
          </div>
        </div>
      </section>
    </div>
  );
}
