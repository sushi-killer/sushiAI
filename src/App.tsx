import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowUpRight,
  Download,
  Bell,
  Check,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  Columns2,
  FolderOpen,
  Globe,
  LayoutDashboard,
  LayoutGrid,
  Maximize2,
  Minimize2,
  MoreHorizontal,
  PanelLeft,
  Play,
  Plus,
  Search,
  Settings,
  Sparkles,
  TerminalSquare,
  Workflow,
  X,
  Zap,
  Plug,
  FileText,
  RefreshCw,
  Link,
  Unplug,
  Trash2,
} from "lucide-react";
import type {
  Layout,
  Message,
  ModelProfile,
  Panel,
  PanelKind,
  Snapshot,
  System,
  SkillCatalogItem,
  SkillManagementAction,
  ClaudePlugin,
  UpdateState,
  Workspace,
} from "./types";
import {
  contains,
  insert,
  leaf,
  remove,
  resize,
  split,
  swap,
  tidy,
  uid,
} from "./layout";
import { TerminalPanel, disposeTerminal } from "./TerminalPanel";
import { BrowserPanel } from "./BrowserPanel";
import { ChatPanel } from "./ChatPanel";
import { ChatView } from "./ChatView";
import { DEFAULT_TITLES, titleFrom } from "./chat-threads";
import { AgentsView } from "./agents/AgentsView";
import { ConnectionsSettings } from "./ConnectionsSettings";
import { ProjectPanel } from "./ProjectPanel";
import { SessionsDialog } from "./SessionsDialog";
import { LocalSkillsView } from "./LocalSkillsView";
import { ClaudeMcpSettings } from "./ClaudeMcpSettings";
import { ProvidersSettings } from "./ProvidersSettings";

import { UpdateSettings } from "./UpdateSettings";

const STORAGE = "sushiai.v1";
type Routine = { id: string; name: string; command: string };
type Saved = {
  workspaces: Workspace[];
  activeId: string;
  socket: string;
  routines: Routine[];
  fontScale: number;
  mode?: "Agent" | "Code" | "Chat";
  tabMode?: boolean;
  section?: string;
  selected?: string;
  zoomed?: string | null;
  sidebar?: boolean;
};
function restore(): Saved | null {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE) || "null");
    if (!Array.isArray(value?.workspaces) || !value.workspaces.length)
      return null;
    const mode =
      value.mode === "Agent" || value.mode === "Chat" ? value.mode : "Code";
    return {
      ...value,
      mode,
      tabMode: value.tabMode === true,
      section: typeof value.section === "string" ? value.section : "",
      selected: typeof value.selected === "string" ? value.selected : "",
      zoomed: typeof value.zoomed === "string" ? value.zoomed : null,
      sidebar:
        typeof value.sidebar === "boolean" ? value.sidebar : undefined,
      workspaces: value.workspaces.map((w: Workspace) => ({
        ...w,
        connection: w.herdrId ? w.connection || value.socket : undefined,
        panels: w.panels.map((p) => ({
          ...p,
          busy: false,
          started: false,
          // A reply that never arrived leaves an empty bubble; drop it.
          messages: p.messages?.filter((m) => m.role === "user" || m.text),
        })),
      })),
    };
  } catch {
    return null;
  }
}
const saved = restore();
function initialWorkspace(cwd = ""): Workspace {
  const panels: Panel[] = [
    { id: uid(), kind: "agent", title: "Claude Code", agent: "claude" },
    { id: uid(), kind: "terminal", title: "zsh" },
    { id: uid(), kind: "browser", title: "Browser" },
    { id: uid(), kind: "chat", title: "Thread", agent: "claude", messages: [] },
  ];
  return {
    id: uid(),
    name: "sushiai",
    cwd,
    panels,
    layout: split(
      leaf(panels[0].id),
      split(
        leaf(panels[1].id),
        split(leaf(panels[2].id), leaf(panels[3].id), "column", 0.49),
        "column",
        0.28,
      ),
      "row",
      0.53,
    ),
  };
}
const Icon = ({
  kind,
  agent,
  size = 13,
}: {
  kind: PanelKind;
  agent?: string;
  size?: number;
}) =>
  agent && ["claude", "codex", "gemini", "cursor-agent"].includes(agent) ? (
    <img
      className="harness-icon"
      src={`./agents/${agent}.svg`}
      width={size}
      height={size}
      alt={agent}
    />
  ) : kind === "agent" ? (
    <span className="agent-star">✳</span>
  ) : kind === "terminal" ? (
    <TerminalSquare size={size} />
  ) : kind === "browser" ? (
    <Globe size={size} />
  ) : kind === "files" ? (
    <FolderOpen size={size} />
  ) : (
    <Sparkles size={size} />
  );
const agentTitle = (name: string) =>
  ({
    claude: "Claude Code",
    codex: "Codex",
    gemini: "Gemini CLI",
    "cursor-agent": "Cursor Agent",
  })[name] || name;
const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);
const catalogFingerprint = (items: SkillCatalogItem[]) =>
  items
    .map((item) =>
      [
        item.path,
        item.provider,
        item.harness,
        item.source,
        item.availability,
        item.disabledBy,
        item.plugin,
        item.name,
        item.description,
        item.size,
        item.updatedAt,
        item.lastUsedAt,
        item.lastUsedSource,
        item.usageCount,
        item.isRecent,
        item.isUnused,
        item.isStale,
        item.changed,
        item.isDuplicate,
        item.duplicateKind,
        item.duplicateCount,
        [...(item.duplicateWith || [])].sort().join(","),
        JSON.stringify(item.recentUses || []),
      ].join("\u001f"),
    )
    .join("\u001e");
// Chat threads live in the workspace but belong to the Chat tab unless they
// were placed into the Code layout, so Code only lists what it shows.
const codePanels = (w: Workspace) =>
  w.panels.filter((p) => p.kind !== "chat" || contains(w.layout, p.id));

export function App() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>(
    saved?.workspaces || [initialWorkspace()],
  );
  const [activeId, setActiveId] = useState(saved?.activeId || "");
  const [system, setSystem] = useState<System | null>(null);
  const [socket, setSocket] = useState(saved?.socket || "");
  const [connection, setConnection] = useState<
    "connected" | "offline" | "connecting"
  >("connecting");
  const [updates, setUpdates] = useState<UpdateState | null>(null);
  const [modelProfiles, setModelProfiles] = useState<ModelProfile[]>([]);
  const [selectedModelProfileId, setSelectedModelProfileId] = useState("");
  const [settingsTab, setSettingsTab] = useState<
    "general" | "connections" | "providers" | "updates"
  >("general");
  const [connectionError, setConnectionError] = useState("");
  const [mode, setMode] = useState<"Agent" | "Code" | "Chat">(
    saved?.mode || "Code",
  );
  const [agentNotices, setAgentNotices] = useState<
    import("./agents/types").AgentActivity[]
  >([]);
  const [tabMode, setTabMode] = useState(saved?.tabMode || false);
  const [section, setSection] = useState(saved?.section || "");
  const [sidebar, setSidebar] = useState(
    saved?.sidebar ?? window.innerWidth >= 760,
  );
  // Agent and Chat render their lists into this slot of the shared sidebar.
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const visitedTabs = useRef(new Set<string>());
  useEffect(() => {
    const query = window.matchMedia("(max-width: 760px)");
    const change = () => setSidebar(!query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  const [modal, setModal] = useState<
    | "pane"
    | "workspace"
    | "settings"
    | "notifications"
    | "updates"
    | "routine"
    | "sessions"
    | "close"
    | "workspace-actions"
    | null
  >(null);
  const [closing, setClosing] = useState<{
    workspace: Workspace;
    panel?: Panel;
  } | null>(null);
  const [workspaceQuery, setWorkspaceQuery] = useState("");
  const canvasRef = useRef<HTMLElement>(null);
  const [compact, setCompact] = useState(false);
  const [adding, setAdding] = useState(false);
  const [selected, setSelected] = useState(saved?.selected || "");
  const [zoomed, setZoomed] = useState<string | null>(saved?.zoomed || null);
  const [toast, setToast] = useState("");
  const [dragId, setDragId] = useState<string | null>(null);
  const [routines, setRoutines] = useState<Routine[]>(saved?.routines || []);
  const [catalog, setCatalog] = useState<SkillCatalogItem[]>([]);
  const [skillsCatalog, setSkillsCatalog] = useState<SkillCatalogItem[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const skillsLoadedRef = useRef(false);
  const skillsFingerprintRef = useRef("");
  const [search, setSearch] = useState("");
  const [fontScale, setFontScale] = useState(saved?.fontScale || 1);
  const active = workspaces.find((w) => w.id === activeId) || workspaces[0];
  const activeRef = useRef(active);
  activeRef.current = active;
  const connected = connection === "connected";
  const activeEndpoint = active.connection || socket;
  useEffect(() => {
    const panels = codePanels(active);
    if (selected && !panels.some((panel) => panel.id === selected))
      setSelected(panels[0]?.id || "");
    if (zoomed && !panels.some((panel) => panel.id === zoomed))
      setZoomed(null);
  }, [active.id, active.layout, active.panels, selected, zoomed]);
  useEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) =>
      setCompact(
        entry.contentRect.width < 720 || entry.contentRect.height < 460,
      ),
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const notify = useCallback((text: string) => setToast(text), []);
  const loadSkills = useCallback(
    async (force = false) => {
      if (!window.bridge) {
        setSkillsCatalog([]);
        skillsFingerprintRef.current = "";
        return;
      }
      setCatalogLoading(true);
      try {
        const nextCatalog = await window.bridge.catalog("skills", { force });
        const nextFingerprint = catalogFingerprint(nextCatalog);
        if (nextFingerprint !== skillsFingerprintRef.current) {
          setSkillsCatalog(nextCatalog);
          skillsFingerprintRef.current = nextFingerprint;
        }
        skillsLoadedRef.current = true;
      } catch (error) {
        notify(errorText(error));
      } finally {
        setCatalogLoading(false);
      }
    },
    [notify],
  );
  const refreshSkills = useCallback(() => loadSkills(true), [loadSkills]);
  const manageSkill = useCallback(
    async (action: SkillManagementAction, item: SkillCatalogItem) => {
      if (!window.bridge?.skillsManage)
        throw new Error("Desktop skill management is unavailable.");
      const result = await window.bridge.skillsManage(action, item);
      notify(result.message);
      await loadSkills(true);
    },
    [loadSkills, notify],
  );
  useEffect(
    () =>
      window.bridge?.onAgents((event) => {
        if (event.type !== "activity") return;
        const activity =
          event.activity as import("./agents/types").AgentActivity;
        setAgentNotices((old) =>
          [activity, ...old.filter((a) => a.id !== activity.id)].slice(0, 100),
        );
        notify(`${activity.agentName} · ${activity.title}`);
      }),
    [notify],
  );
  useEffect(() => {
    const bridge = window.bridge;
    if (!bridge?.updatesState) return;
    let stopped = false;
    let received = false;
    const unsubscribe = bridge.onUpdates((state) => {
      received = true;
      setUpdates(state);
    });
    bridge
      .updatesState()
      .then((state) => {
        if (!stopped && !received) setUpdates(state);
      })
      .catch((e) => notify(errorText(e)));
    return () => {
      stopped = true;
      unsubscribe();
    };
  }, [notify]);
  const updateWorkspace = useCallback(
    (workspaceId: string, update: (w: Workspace) => Workspace) =>
      setWorkspaces((items) =>
        items.map((w) => (w.id === workspaceId ? update(w) : w)),
      ),
    [],
  );
  const updatePanel = useCallback(
    (panelId: string, patch: Partial<Panel>) =>
      setWorkspaces((items) =>
        items.map((w) => ({
          ...w,
          panels: w.panels.map((p) =>
            p.id === panelId ? { ...p, ...patch } : p,
          ),
        })),
      ),
    [],
  );

  useEffect(() => {
    if (!window.bridge) {
      setConnection("offline");
      setConnectionError(
        "Browser preview. Start the desktop app for terminal and Herdr access.",
      );
      return;
    }
    window.bridge
      .system()
      .then((info) => {
        setSystem(info);
        setSocket((current) => current || info.socketPath);
        setWorkspaces((items) =>
          items.map((w) => ({ ...w, cwd: w.cwd || info.cwd })),
        );
      })
      .catch((error) => notify(errorText(error)));
    return window.bridge.onChat((event) =>
      setWorkspaces((items) =>
        items.map((w) => ({
          ...w,
          panels: w.panels.map((p) => {
            if (p.id !== event.panelId) return p;
            const messages = [...(p.messages || [])];
            const last = messages.at(-1);
            if (event.text && last?.role === "assistant")
              messages[messages.length - 1] = {
                ...last,
                text: last.text + event.text,
              };
            if (
              event.done &&
              last?.role === "assistant" &&
              !messages.at(-1)?.text
            )
              messages.pop();
            // Stamp the answer with the model that produced it, so switching
            // models later still shows what each earlier turn was answered by.
            const final = messages.at(-1);
            if (event.done && final?.role === "assistant" && final.text)
              messages[messages.length - 1] = {
                ...final,
                model: event.model || p.resolvedModel,
              };
            return {
              ...p,
              messages,
              busy: !event.done,
              error: event.error || p.error,
              note: event.done ? undefined : (event.note ?? p.note),
              resolvedModel: event.model || p.resolvedModel,
              usage: event.usage || p.usage,
              updatedAt: event.done ? Date.now() : p.updatedAt,
            };
          }),
        })),
      ),
    );
  }, [notify]);
  useEffect(
    () =>
      window.bridge?.onTerminal((event) => {
        if (event.agent !== undefined)
          setWorkspaces((items) =>
            items.map((w) => ({
              ...w,
              panels: w.panels.map((p) =>
                p.id === event.panelId
                  ? {
                      ...p,
                      agent: event.agent || undefined,
                      title:
                        p.kind === "terminal" &&
                        [
                          "zsh",
                          "Claude Code",
                          "Codex",
                          "Gemini CLI",
                          "Cursor Agent",
                        ].includes(p.title)
                          ? event.agent
                            ? agentTitle(event.agent)
                            : "zsh"
                          : p.title,
                    }
                  : p,
              ),
            })),
          );
      }),
    [],
  );
  useEffect(() => {
    if (toast) {
      const timer = setTimeout(() => setToast(""), 6000);
      return () => clearTimeout(timer);
    }
  }, [toast]);
  useEffect(() => {
    // Commit each layout change before a reload or a background snapshot arrives.
    try {
      localStorage.setItem(
        STORAGE,
        JSON.stringify({
          workspaces,
          activeId: active.id,
          socket,
          routines,
          fontScale,
          mode,
          tabMode,
          section,
          selected,
          zoomed,
          sidebar,
        }),
      );
    } catch {
      notify("Storage is full. Clear older chat history.");
    }
  }, [
    workspaces,
    active.id,
    socket,
    routines,
    fontScale,
    mode,
    tabMode,
    section,
    selected,
    zoomed,
    sidebar,
    notify,
  ]);
  const refreshHerdr = useCallback(
    async (path = socket) => {
      if (!window.bridge || !path) return;
      try {
        const response = await window.bridge.herdr(path, "session.snapshot");
        const snapshot: Snapshot = response.snapshot || response;
        if (
          !Array.isArray(snapshot.workspaces) ||
          !Array.isArray(snapshot.panes)
        )
          throw new Error("Unsupported Herdr snapshot response.");
        setWorkspaces((current) => {
          const local = current.filter(
            (w) => !w.herdrId || w.connection !== path,
          );
          const endpointKey = path.startsWith("ssh:") ? path : "local";
          const remote = snapshot.workspaces.map((w) => {
            const existing = current.find(
              (item) =>
                item.herdrId === w.workspace_id && item.connection === path,
            );
            const remotePanes = snapshot.panes.filter(
              (p) => p.workspace_id === w.workspace_id,
            );
            const panels: Panel[] = remotePanes.map((p) => ({
              id:
                existing?.panels.find((old) => old.herdrId === p.pane_id)?.id ||
                `herdr:${endpointKey}:${p.pane_id}`,
              kind: p.agent ? "agent" : "terminal",
              title:
                p.label ||
                (p.agent
                  ? agentTitle(p.agent)
                  : p.terminal_title_stripped || "zsh"),
              herdrId: p.pane_id,
              agent: p.agent,
              status: p.agent_status,
            }));
            const extras = existing?.panels.filter((p) => !p.herdrId) || [];
            const allPanels = [...panels, ...extras];
            let layout = existing
              ? existing.layout
              : tidy(allPanels.map((p) => p.id));
            if (existing) {
              for (const old of existing.panels)
                if (!allPanels.some((p) => p.id === old.id))
                  layout = remove(layout, old.id);
              for (const panel of panels)
                if (!existing.panels.some((p) => p.id === panel.id))
                  layout = layout
                    ? split(layout, leaf(panel.id), "column")
                    : leaf(panel.id);
            }
            return {
              id: existing?.id || `herdr:${endpointKey}:${w.workspace_id}`,
              connection: path,
              herdrId: w.workspace_id,
              name: w.label,
              cwd:
                remotePanes.find((p) => p.cwd)?.cwd ||
                w.worktree?.checkout_path ||
                system?.home ||
                "",
              panels: allPanels,
              layout,
            };
          });
          return [...local, ...remote];
        });
        if (path === socket) {
          setConnection("connected");
          setConnectionError("");
        }
      } catch (error) {
        if (path === socket) {
          setConnection("offline");
          setConnectionError(errorText(error));
        }
      }
    },
    [socket, system?.home],
  );
  useEffect(() => {
    if (!socket || !system) return;
    let stopped = false,
      timer: ReturnType<typeof setTimeout>;
    async function poll() {
      await refreshHerdr();
      if (!stopped) timer = setTimeout(poll, 4000);
    }
    poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [socket, system, refreshHerdr]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setModal(null);
        setZoomed(null);
      }
      if (!event.metaKey) return;
      if (mode !== "Code" && !section && event.key !== "b") return;
      if (event.key === "k" || event.key === "t") {
        event.preventDefault();
        setModal("pane");
      }
      if (event.key === "b") {
        event.preventDefault();
        setSidebar((value) => !value);
      }
      const currentPanel =
        active.panels.find((p) => p.id === selected) || active.panels[0];
      if (event.key === "Enter" && currentPanel) {
        event.preventDefault();
        setZoomed((value) => (value ? null : currentPanel.id));
      }
      if (event.key === "w" && !modal) {
        event.preventDefault();
        if (currentPanel) closePanel(currentPanel);
      }
      const panels = codePanels(active);
      if (/^[1-9]$/.test(event.key)) {
        const panel = panels[Number(event.key) - 1];
        if (panel) {
          event.preventDefault();
          setSelected(panel.id);
          setZoomed(null);
        }
      }
      if (
        event.shiftKey &&
        ["BracketLeft", "BracketRight"].includes(event.code) &&
        panels.length
      ) {
        event.preventDefault();
        const index = panels.findIndex((p) => p.id === selected);
        setSelected(
          panels[
            (Math.max(0, index) +
              (event.code === "BracketRight" ? 1 : -1) +
              panels.length) %
              panels.length
          ]?.id,
        );
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [selected, active, modal, mode, section]);
  useEffect(() => {
    if (modal === "pane" || modal === "settings")
      window.bridge?.modelProfilesList().then(setModelProfiles);
    if (modal === "pane") setSelectedModelProfileId("");
  }, [modal]);
  useEffect(() => {
    setSearch("");
    if (section === "Skills") {
      if (!skillsLoadedRef.current) void loadSkills();
      return;
    }
    setCatalog([]);
    if (section === "Plugins" && connected)
      window.bridge
        ?.herdr(socket, "plugin.list", {})
        .then((result) =>
          setCatalog(
            (result.plugins || []).map((p: any) => ({
              name: p.name || p.id || p.plugin_id,
              description:
                p.description ||
                (p.enabled ? "Enabled in Herdr" : "Installed in Herdr"),
            })),
          ),
        )
        .catch((error) => notify(errorText(error)));
  }, [section, connected, socket, notify, loadSkills]);

  function switchWorkspace(id: string) {
    const target = workspaces.find((w) => w.id === id);
    if (target?.connection) setSocket(target.connection);
    if (window.innerWidth < 760) setSidebar(false);
    setActiveId(id);
    setSection("");
    setMode("Code");
    setZoomed(null);
  }
  async function addPanel(
    kind: PanelKind,
    agent = "claude",
    filesTarget?: Panel["filesTarget"],
    modelProfileId?: string,
  ) {
    const modelProfile = modelProfiles.find((p) => p.id === modelProfileId);
    const current = activeRef.current;
    if (adding) return;
    setAdding(true);
    try {
      if (current.herdrId && (kind === "terminal" || kind === "agent")) {
        if (!window.bridge) throw new Error("Open the desktop app first.");
        const endpoint = current.connection || socket;
        // Staged before the pane exists: a gateway that can't be reached
        // shouldn't leave an empty pane behind. Herdr panes are just a typed
        // shell command, so the model swap rides on that command line
        // instead of the argv/env injection the direct local launch uses.
        let launchText = agent;
        if (kind === "agent" && agent === "claude" && modelProfileId) {
          if (endpoint.startsWith("ssh:"))
            throw new Error(
              "Custom models aren't supported on remote (SSH) workspaces yet.",
            );
          const settingsPath =
            await window.bridge.modelSettingsStage(modelProfileId);
          launchText = `claude --settings '${settingsPath}'`;
        }
        const result = await window.bridge.herdr(endpoint, "pane.split", {
          workspace_id: current.herdrId,
          target_pane_id: current.panels.find((p) => p.herdrId)?.herdrId,
          direction: "right",
          focus: false,
          cwd: current.cwd,
        });
        const paneId = result.pane?.pane_id || result.pane_id;
        if (kind === "agent" && paneId)
          await window.bridge.herdr(endpoint, "pane.send_input", {
            pane_id: paneId,
            text: launchText,
            keys: ["Enter"],
          });
        await refreshHerdr(endpoint);
        if (paneId)
          setSelected(
            `herdr:${endpoint.startsWith("ssh:") ? endpoint : "local"}:${paneId}`,
          );
      } else {
        const panel: Panel = {
          id: uid(),
          kind,
          title:
            kind === "agent"
              ? modelProfile?.label || agentTitle(agent)
              : kind === "terminal"
                ? "zsh"
                : kind === "browser"
                  ? "Browser"
                  : kind === "files"
                    ? "Files & Git"
                    : "Thread",
          agent: kind === "agent" || kind === "chat" ? agent : undefined,
          started: kind === "agent",
          messages: kind === "chat" ? [] : undefined,
          filesTarget: kind === "files" ? filesTarget : undefined,
          modelProfileId: kind === "agent" ? modelProfileId : undefined,
        };
        updateWorkspace(current.id, (w) => ({
          ...w,
          panels: [...w.panels, panel],
          layout: w.layout
            ? split(w.layout, leaf(panel.id), "row", 0.65)
            : leaf(panel.id),
        }));
        setSelected(panel.id);
      }
      setModal(null);
      setSection("");
      setMode("Code");
      setZoomed(null);
    } catch (error) {
      notify(errorText(error));
    } finally {
      setAdding(false);
    }
  }
  function closePanel(panel: Panel) {
    if (panel.herdrId) {
      setClosing({ workspace: active, panel });
      setModal("close");
      return;
    }
    disposeTerminal(panel.id);
    // Closing an imported panel hides it locally; it never kills a Herdr process.
    if (!panel.herdrId)
      window.bridge
        ?.terminalClose(panel.id)
        .catch((error) => notify(errorText(error)));
    if (panel.busy)
      window.bridge
        ?.cancelChat(panel.id)
        .catch((error) => notify(errorText(error)));
    // A pane is a view, a thread is a conversation: closing the view leaves the
    // Code layout only. Threads are deleted from the Chat tab.
    updateWorkspace(active.id, (w) => ({
      ...w,
      layout: remove(w.layout, panel.id),
      panels:
        panel.herdrId || panel.kind === "chat"
          ? w.panels
          : w.panels.filter((p) => p.id !== panel.id),
    }));
    if (zoomed === panel.id) setZoomed(null);
  }
  function showPanel(panel: Panel) {
    setSection("");
    setMode("Code");
    setSelected(panel.id);
    setZoomed(panel.id);
    updateWorkspace(active.id, (w) => ({
      ...w,
      layout: w.layout || leaf(panel.id),
    }));
  }
  function drop(target: string, edge: string) {
    if (!dragId || dragId === target) return;
    updateWorkspace(active.id, (w) => {
      if (!w.layout) return w;
      if (edge === "center")
        return { ...w, layout: swap(w.layout, dragId, target) };
      const without = remove(w.layout, dragId);
      return {
        ...w,
        layout: without ? insert(without, target, dragId, edge) : leaf(dragId),
      };
    });
    setDragId(null);
  }
  async function sendChat(
    panel: Panel,
    text: string,
    attachments: string[] = [],
  ) {
    if (!window.bridge)
      return notify("Open the desktop app to chat with your agents.");
    const owner =
      workspaces.find((w) => w.panels.some((p) => p.id === panel.id)) || active;
    const messages: Message[] = [
      ...(panel.messages || []),
      {
        id: uid(),
        role: "user",
        text,
        attachments: attachments.length ? attachments : undefined,
      },
    ];
    updatePanel(panel.id, {
      messages: [...messages, { id: uid(), role: "assistant", text: "" }],
      busy: true,
      error: "",
      note: "",
      resolvedModel: "",
      updatedAt: Date.now(),
      title: DEFAULT_TITLES.has(panel.title) ? titleFrom(text) : panel.title,
    });
    try {
      await window.bridge.chat({
        panelId: panel.id,
        cwd: owner.cwd,
        endpoint: owner.connection,
        agent: panel.agent || "claude",
        messages,
        model: panel.model || undefined,
        effort: panel.effort || undefined,
        permission: panel.permission || undefined,
      });
    } catch (error) {
      updatePanel(panel.id, { busy: false, error: errorText(error) });
    }
  }
  // Chat threads live in the project's panel list but stay out of the Code layout.
  function newThread(workspaceId: string): Panel {
    const panel: Panel = {
      id: uid(),
      kind: "chat",
      title: "New thread",
      agent: "claude",
      permission: "acceptEdits",
      messages: [],
      updatedAt: Date.now(),
    };
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      panels: [...w.panels, panel],
    }));
    return panel;
  }
  function deleteThread(workspaceId: string, panel: Panel) {
    if (panel.busy)
      window.bridge
        ?.cancelChat(panel.id)
        .catch((error) => notify(errorText(error)));
    updateWorkspace(workspaceId, (w) => ({
      ...w,
      layout: remove(w.layout, panel.id),
      panels: w.panels.filter((p) => p.id !== panel.id),
    }));
    if (zoomed === panel.id) setZoomed(null);
  }
  async function runRoutine(routine: Routine) {
    if (!window.bridge) return notify("Run routines in the desktop app.");
    const panel: Panel = { id: uid(), kind: "terminal", title: routine.name };
    try {
      await window.bridge.terminalOpen({
        panelId: panel.id,
        cwd: active.cwd,
        endpoint: active.connection,
      });
      await window.bridge.terminalWrite(panel.id, routine.command + "\r");
      updateWorkspace(active.id, (w) => ({
        ...w,
        panels: [...w.panels, panel],
        layout: w.layout
          ? split(w.layout, leaf(panel.id), "row")
          : leaf(panel.id),
      }));
      setSection("");
      setMode("Code");
      setZoomed(panel.id);
    } catch (error) {
      notify(errorText(error));
    }
  }

  async function endSessions(items: { workspace: Workspace; panel: Panel }[]) {
    const closed = new Set<string>(),
      errors: string[] = [];
    for (const { workspace, panel } of items) {
      try {
        if (panel.herdrId)
          await window.bridge!.herdr(
            workspace.connection || socket,
            "pane.close",
            { pane_id: panel.herdrId },
          );
        else await window.bridge?.terminalClose(panel.id);
        if (panel.busy) await window.bridge?.cancelChat(panel.id);
        await window.bridge?.terminalClose(panel.id);
        disposeTerminal(panel.id);
        closed.add(panel.id);
      } catch (error) {
        errors.push(panel.title + ": " + errorText(error));
      }
    }
    setWorkspaces((list) =>
      list.map((w) => ({
        ...w,
        panels: w.panels.filter((p) => !closed.has(p.id) || p.kind === "chat"),
        layout: [...closed].reduce((tree, id) => remove(tree, id), w.layout),
      })),
    );
    if (zoomed && closed.has(zoomed)) setZoomed(null);
    for (const endpoint of new Set(
      items
        .filter((i) => i.panel.herdrId)
        .map((i) => i.workspace.connection || socket),
    ))
      await refreshHerdr(endpoint);
    if (errors.length) notify(errors.join("; "));
  }
  async function endWorkspace(workspace: Workspace) {
    try {
      if (workspace.herdrId)
        await window.bridge!.herdr(
          workspace.connection || socket,
          "workspace.close",
          { workspace_id: workspace.herdrId },
        );
      for (const panel of workspace.panels) {
        await window.bridge?.terminalClose(panel.id);
        disposeTerminal(panel.id);
        if (panel.busy) await window.bridge?.cancelChat(panel.id);
      }
      setWorkspaces((list) => {
        const rest = list.filter((w) => w.id !== workspace.id);
        return rest.length ? rest : [initialWorkspace(system?.cwd || "")];
      });
      setModal(null);
      setZoomed(null);
    } catch (error) {
      notify(errorText(error));
    }
  }
  function openHTML(root: string, file: string) {
    const panel: Panel = {
      id: uid(),
      kind: "browser",
      title: file.split("/").at(-1) || "Preview",
      previewFile: { root, path: file, endpoint: active.connection },
    };
    updateWorkspace(active.id, (w) => ({
      ...w,
      panels: [...w.panels, panel],
      layout: w.layout
        ? split(w.layout, leaf(panel.id), "row", 0.55)
        : leaf(panel.id),
    }));
    setSelected(panel.id);
    setZoomed(panel.id);
  }
  function renderPanel(id: string) {
    const panel = active.panels.find((p) => p.id === id);
    if (!panel) return null;
    return (
      <PanelFrame
        key={panel.id}
        panel={panel}
        selected={selected === id}
        zoomed={zoomed === id}
        dragging={!!dragId}
        onFocus={() => setSelected(id)}
        onDrag={() => setDragId(id)}
        onDragEnd={() => setDragId(null)}
        onDrop={(edge) => drop(id, edge)}
        onClose={() => closePanel(panel)}
        onZoom={() => setZoomed(zoomed ? null : id)}
        onAdd={() => setModal("pane")}
        onRename={(title) => {
          if (panel.herdrId)
            window.bridge
              ?.herdr(activeEndpoint, "pane.rename", {
                pane_id: panel.herdrId,
                label: title,
              })
              .then(() => refreshHerdr(activeEndpoint))
              .catch((error) => notify(errorText(error)));
          else updatePanel(id, { title });
        }}
      >
        {panel.kind === "terminal" || panel.kind === "agent" ? (
          active.cwd || !window.bridge ? (
            <TerminalPanel
              panel={panel}
              cwd={active.cwd}
              socket={activeEndpoint}
              endpoint={active.connection}
              onStart={() => updatePanel(id, { started: true })}
            />
          ) : (
            <div className="loading">Opening workspace…</div>
          )
        ) : panel.kind === "browser" ? (
          <BrowserPanel
            url={panel.url}
            endpoint={active.connection}
            sourceFile={panel.previewFile}
            onNavigate={(url) =>
              updatePanel(id, { url, previewFile: undefined })
            }
          />
        ) : panel.kind === "files" ? (
          <ProjectPanel
            key={`${panel.id}:${panel.filesTarget?.root || active.cwd}:${panel.filesTarget?.path || ""}:${panel.filesTarget?.openToken || ""}`}
            cwd={panel.filesTarget?.root || active.cwd}
            initialFile={panel.filesTarget?.path}
            initialEdit={panel.filesTarget?.edit}
            endpoint={
              panel.filesTarget ? panel.filesTarget.endpoint : active.connection
            }
            onHTML={openHTML}
          />
        ) : (
          <ChatPanel
            panel={panel}
            onSend={(text) => sendChat(panel, text)}
            onCancel={() => window.bridge?.cancelChat(id)}
            onAgent={(agent) => updatePanel(id, { agent })}
          />
        )}
      </PanelFrame>
    );
  }
  const filteredPanels = codePanels(active);
  const compactId =
    filteredPanels.find((p) => p.id === selected)?.id || filteredPanels[0]?.id;
  const useTabs = tabMode || compact || filteredPanels.length > 6;
  if (compactId) visitedTabs.current.add(compactId);
  const visibleLayout = active.layout;
  const totalPanels = workspaces.reduce(
    (sum, w) => sum + codePanels(w).length,
    0,
  );
  const blocked = workspaces.flatMap((w) =>
    w.panels
      .filter((p) => p.status === "blocked")
      .map((p) => ({ workspace: w, panel: p })),
  );

  const profile = (
    <div className="profile">
      <span className="avatar">
        <img src="./sushi.svg" width="22" height="22" alt="" />
      </span>
      <div>
        <strong>sushiAI</strong>
        <span>ON YOUR MAC</span>
      </div>
      <button
        className="icon-button"
        title="Keyboard shortcuts"
        onClick={() =>
          notify(
            "⌘K Add panel · ⌘B Toggle sidebar · ⌘Enter Focus panel · Esc Restore layout. Drag panel headers to rearrange; drag dividers to resize.",
          )
        }
      >
        <CircleHelp size={13} />
      </button>
      <button
        className="icon-button"
        aria-label="Settings"
        onClick={() => setModal("settings")}
      >
        <Settings size={13} />
      </button>
    </div>
  );
  return (
    <div
      className={`app ${compact ? "compact" : ""}`}
      style={{ fontSize: `${13 * fontScale}px` }}
    >
      <header className="titlebar">
        <div className="titlebar-left">
          {!window.bridge && (
            <div className="traffic-lights">
              <i />
              <i />
              <i />
            </div>
          )}
          <div className="brand">
            <span className="brand-symbol">
              <img src="./sushi.svg" width="22" height="22" alt="sushiAI" />
            </span>
            <strong>sushiAI</strong>
          </div>
          <button
            className="icon-button sidebar-toggle"
            aria-label="Toggle sidebar"
            title="Toggle sidebar · ⌘B"
            onClick={() => setSidebar(!sidebar)}
          >
            <PanelLeft size={14} />
          </button>
        </div>
        <div className="mode-switch">
          {(["Agent", "Code", "Chat"] as const).map((item) => (
            <button
              key={item}
              className={mode === item ? "active" : ""}
              onClick={() => {
                setMode(item);
                setSection("");
                setZoomed(null);
              }}
            >
              {item}
            </button>
          ))}
        </div>
        <div className="titlebar-right">
          {(mode === "Code" || !!section) && (
            <>
              <button
                className={`icon-button ${tabMode ? "active" : ""}`}
                title={tabMode ? "Switch to split panels" : "Switch to tabs"}
                aria-pressed={tabMode}
                onClick={() => {
                  setTabMode(!tabMode);
                  setZoomed(null);
                }}
              >
                <Columns2 size={14} />
              </button>
              <button
                className="icon-button"
                title="Files and Git"
                onClick={() => {
                  const panel = active.panels.find((p) => p.kind === "files");
                  panel ? showPanel(panel) : addPanel("files");
                }}
              >
                <FolderOpen size={14} />
              </button>
              <button
                className="tidy"
                onClick={() => {
                  updateWorkspace(active.id, (w) => ({
                    ...w,
                    layout: tidy(codePanels(w).map((p) => p.id)),
                  }));
                  setZoomed(null);
                  setSection("");
                  setMode("Code");
                  setTabMode(false);
                }}
                title="Arrange all panels"
              >
                <LayoutGrid size={12} /> Tidy
              </button>
            </>
          )}
          {updates?.release && (
            <button
              className="update-indicator"
              aria-label="Software update available"
              onClick={() => setModal("updates")}
            >
              <Download size={13} />
              <span>
                {updates.phase === "downloading"
                  ? `${updates.progress}%`
                  : updates.phase === "ready"
                    ? "Update ready"
                    : "Update available"}
              </span>
            </button>
          )}
          <button
            className="icon-button notification-button"
            aria-label="Notifications"
            onClick={() => setModal("notifications")}
          >
            <Bell size={14} />
            {(blocked.length > 0 ||
              updates?.release ||
              agentNotices.length > 0) && <i />}
          </button>
        </div>
      </header>
      <div className="app-body">
        {sidebar && (
          <aside className="sidebar">
            {/* The sidebar is one container; Code fills it with nav, workspaces
                and footer, Agent and Chat load only their list into it. */}
            {mode === "Code" ? (
              <>
                <nav className="primary-nav">
                  {[
                    { label: "Dashboard", icon: LayoutDashboard },
                    { label: "Sessions", icon: TerminalSquare },
                    { label: "Routines", icon: Workflow },
                    { label: "Plugins", icon: Plug },
                    { label: "Skills", icon: Sparkles },
                  ].map(({ label, icon: NavIcon }) => (
                    <button
                      key={label}
                      className={
                        section === label ? "nav-item current" : "nav-item"
                      }
                      onClick={() =>
                        label === "Sessions"
                          ? setModal("sessions")
                          : setSection(section === label ? "" : label)
                      }
                    >
                      <NavIcon size={14} />
                      <span>{label}</span>
                      {label === "Dashboard" && (
                        <span className="count">{workspaces.length}</span>
                      )}
                    </button>
                  ))}
                </nav>
                <div className="workspace-section">
                  <div className="section-label">
                    <span>Workspaces</span>
                    <button
                      className="icon-button"
                      aria-label="New workspace"
                      onClick={() => setModal("workspace")}
                    >
                      <Plus size={14} />
                    </button>
                  </div>
                  <label className="workspace-search">
                    <Search size={12} />
                    <input
                      aria-label="Search workspaces"
                      placeholder="Find workspace…"
                      value={workspaceQuery}
                      onChange={(e) => setWorkspaceQuery(e.target.value)}
                    />
                  </label>
                  <div className="workspace-list">
                    {workspaces
                      .filter((w) =>
                        w.name
                          .toLowerCase()
                          .includes(workspaceQuery.toLowerCase()),
                      )
                      .map((w) => (
                        <div key={w.id} className="workspace-item">
                          <button
                            className="workspace-more"
                            title={`Manage workspace ${w.name}`}
                            onClick={() => {
                              setClosing({ workspace: w });
                              setModal("workspace-actions");
                            }}
                          >
                            <MoreHorizontal size={13} />
                          </button>
                          <button
                            className={`workspace-name ${w.id === active.id && !section ? "active" : ""}`}
                            onClick={() => switchWorkspace(w.id)}
                            title={w.cwd}
                          >
                            {w.id === active.id ? (
                              <ChevronDown size={12} />
                            ) : (
                              <ChevronRight size={12} />
                            )}
                            <span>{w.name}</span>
                            {w.herdrId && (
                              <i
                                className={`status-dot ${connected ? "green" : ""}`}
                                title="Herdr workspace"
                              />
                            )}
                          </button>
                          {w.id === active.id && !section && (
                            <div className="workspace-panels">
                              {codePanels(w).map((p) => (
                                <button
                                  key={p.id}
                                  className={
                                    selected === p.id ? "selected" : ""
                                  }
                                  onClick={() => showPanel(p)}
                                  title={p.title}
                                >
                                  <Icon kind={p.kind} agent={p.agent} />
                                  <span>
                                    {p.kind === "browser" && p.url
                                      ? p.url
                                          .replace(/^https?:\/\//, "")
                                          .replace(/\/$/, "")
                                      : p.title}
                                  </span>
                                  {p.status === "working" && (
                                    <i className="status-dot green pulse" />
                                  )}
                                  {p.status === "blocked" && (
                                    <i className="status-dot yellow" />
                                  )}
                                </button>
                              ))}
                            </div>
                          )}
                        </div>
                      ))}
                  </div>
                </div>
                <footer className="sidebar-footer">
                  <button
                    className="backend-status"
                    onClick={() => setModal("settings")}
                  >
                    <span>Herdr</span>
                    <span className={`status-pill ${connected ? "live" : ""}`}>
                      <i />
                      {connected
                        ? "Connected"
                        : connection === "connecting"
                          ? "Connecting"
                          : "Offline"}
                    </span>
                  </button>
                  <div className="session-count">
                    <span>Panels</span>
                    <span className="count">{totalPanels}</span>
                  </div>
                  {profile}
                </footer>
              </>
            ) : (
              <>
                <div className="sidebar-slot" ref={setSlot} />
                <footer className="sidebar-footer">{profile}</footer>
              </>
            )}
          </aside>
        )}
        <main
          ref={canvasRef}
          className={`workspace-canvas ${dragId ? "is-dragging" : ""} ${useTabs ? "tabbed-canvas" : ""}`}
        >
          {section ? (
            <div className="section-page">
              <div className="page-eyebrow">YOUR WORKSPACE</div>
              <div className="page-heading">
                <div>
                  <h1>{section}</h1>
                  <p>
                    {section === "Dashboard"
                      ? "A little space for everything you’re building."
                      : section === "Routines"
                        ? "Your everyday commands, one click away."
                        : section === "Skills"
                          ? "Skills found on this Mac, grouped by harness and ready for cleanup review."
                          : "Extensions connected to your Herdr session."}
                  </p>
                </div>
                {section === "Routines" && (
                  <button
                    className="primary"
                    onClick={() => setModal("routine")}
                  >
                    <Plus size={14} /> New routine
                  </button>
                )}
              </div>
              {section === "Dashboard" ? (
                <>
                  <div className="stat-grid">
                    <div>
                      <LayersIcon />
                      <strong>{workspaces.length}</strong>
                      <span>Workspaces</span>
                    </div>
                    <div>
                      <TerminalSquare size={19} />
                      <strong>{totalPanels}</strong>
                      <span>Open panels</span>
                    </div>
                    <div>
                      <Sparkles size={19} />
                      <strong>
                        {
                          workspaces
                            .flatMap((w) => w.panels)
                            .filter((p) => p.status === "working").length
                        }
                      </strong>
                      <span>Agents working</span>
                    </div>
                  </div>
                  <h3>Pick up where you left off</h3>
                  <div className="project-grid">
                    {workspaces.map((w) => (
                      <button key={w.id} onClick={() => switchWorkspace(w.id)}>
                        <FolderOpen size={19} />
                        <strong>{w.name}</strong>
                        <p>{w.cwd}</p>
                        <span>
                          {codePanels(w).length} panels{" "}
                          <ArrowUpRight size={13} />
                        </span>
                      </button>
                    ))}
                  </div>
                </>
              ) : section === "Routines" ? (
                routines.length ? (
                  <div className="routine-list">
                    {routines.map((r) => (
                      <div key={r.id}>
                        <Workflow size={17} />
                        <div>
                          <strong>{r.name}</strong>
                          <code>{r.command}</code>
                        </div>
                        <button
                          title="Run routine"
                          onClick={() => runRoutine(r)}
                        >
                          <Play size={15} />
                        </button>
                        <button
                          title="Delete routine"
                          onClick={() =>
                            setRoutines((items) =>
                              items.filter((item) => item.id !== r.id),
                            )
                          }
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    ))}
                  </div>
                ) : (
                  <Empty
                    icon={<Workflow size={28} />}
                    title="Make room for your rituals."
                    text="Save build, test, and development commands for this workspace."
                    action="Create a routine"
                    onAction={() => setModal("routine")}
                  />
                )
              ) : section === "Skills" ? (
                <LocalSkillsView
                  items={skillsCatalog}
                  search={search}
                  loading={catalogLoading}
                  onSearchChange={setSearch}
                  onRefresh={() => void refreshSkills()}
                  onManage={manageSkill}
                  home={system?.home}
                />
              ) : (
                <>
                  <label className="catalog-search">
                    <Search size={15} />
                    <input
                      placeholder={`Search ${section.toLowerCase()}…`}
                      value={search}
                      onChange={(event) => setSearch(event.target.value)}
                    />
                  </label>
                  {catalog.length ? (
                    <div className="catalog-grid">
                      {catalog
                        .filter((item) =>
                          (item.name + item.description)
                            .toLowerCase()
                            .includes(search.toLowerCase()),
                        )
                        .map((item, i) => (
                          <div key={item.name + i}>
                            <FileText size={16} />
                            <strong>{item.name}</strong>
                            <p>{item.description}</p>
                            <small title={item.path}>
                              {item.path || "Herdr extension"}
                            </small>
                          </div>
                        ))}
                    </div>
                  ) : (
                    <Empty
                      icon={<Plug size={27} />}
                      title={
                        section === "Skills"
                          ? "No local skills found"
                          : "No plugins to show"
                      }
                      text={
                        section === "Skills"
                          ? "Skills are read from ~/.codex/skills and ~/.agents/skills."
                          : connected
                            ? "Plugins installed in Herdr will appear here."
                            : "Connect to Herdr in Settings to see its plugins."
                      }
                    />
                  )}
                </>
              )}
            </div>
          ) : mode === "Agent" ? (
            <AgentsView slot={slot} />
          ) : mode === "Chat" ? (
            <ChatView
              workspaces={workspaces}
              active={active}
              slot={slot}
              onSelectWorkspace={(id) => {
                const target = workspaces.find((w) => w.id === id);
                if (target?.connection) setSocket(target.connection);
                setActiveId(id);
              }}
              onNewThread={newThread}
              onSend={sendChat}
              onCancel={(id) =>
                window.bridge
                  ?.cancelChat(id)
                  .catch((error) => notify(errorText(error)))
              }
              onPatch={updatePanel}
              onDelete={deleteThread}
              onToggleSidebar={() => setSidebar(!sidebar)}
            />
          ) : zoomed && active.panels.some((p) => p.id === zoomed) ? (
            renderPanel(zoomed)
          ) : useTabs && compactId ? (
            <div className="adaptive-workspace">
              <div
                className="panel-tabs"
                role="tablist"
                aria-label="Workspace panels"
              >
                {filteredPanels.map((p) => (
                  <div
                    className={`panel-tab ${p.id === compactId ? "active" : ""}`}
                    key={p.id}
                  >
                    <button
                      role="tab"
                      aria-selected={p.id === compactId}
                      tabIndex={p.id === compactId ? 0 : -1}
                      onKeyDown={(event) => {
                        if (
                          ["ArrowLeft", "ArrowRight", "Home", "End"].includes(
                            event.key,
                          )
                        ) {
                          event.preventDefault();
                          const index = filteredPanels.findIndex(
                            (item) => item.id === p.id,
                          );
                          const next =
                            event.key === "Home"
                              ? 0
                              : event.key === "End"
                                ? filteredPanels.length - 1
                                : (index +
                                    (event.key === "ArrowRight" ? 1 : -1) +
                                    filteredPanels.length) %
                                  filteredPanels.length;
                          setSelected(filteredPanels[next].id);
                          const tabs = event.currentTarget
                            .closest('[role="tablist"]')
                            ?.querySelectorAll<HTMLElement>('[role="tab"]');
                          tabs?.[next].focus();
                        }
                      }}
                      onClick={() => setSelected(p.id)}
                    >
                      <Icon kind={p.kind} agent={p.agent} />
                      <span>{p.title}</span>
                    </button>
                    <button
                      className="tab-close"
                      aria-label={`Close tab ${p.title}`}
                      onClick={() => closePanel(p)}
                    >
                      <X size={11} />
                    </button>
                  </div>
                ))}
                <button title="Add panel" onClick={() => setModal("pane")}>
                  <Plus size={13} />
                </button>
              </div>
              <div className="adaptive-panel">
                {filteredPanels
                  .filter((p) => visitedTabs.current.has(p.id))
                  .map((p) => (
                    <div
                      className="tab-panel"
                      key={p.id}
                      hidden={p.id !== compactId}
                    >
                      {renderPanel(p.id)}
                    </div>
                  ))}
              </div>
            </div>
          ) : visibleLayout ? (
            <LayoutView
              layout={visibleLayout}
              renderPanel={renderPanel}
              onResize={(id, ratio) =>
                updateWorkspace(active.id, (w) => ({
                  ...w,
                  layout: w.layout ? resize(w.layout, id, ratio) : null,
                }))
              }
            />
          ) : (
            <Empty
              icon={<Columns2 size={30} />}
              title="Space for your next idea."
              text="Add a panel to get started."
              action="Add panel"
              onAction={() => setModal("pane")}
            />
          )}
        </main>
      </div>
      {toast && (
        <div className="toast" role="status">
          <span>{toast}</span>
          <button aria-label="Dismiss message" onClick={() => setToast("")}>
            <X size={14} />
          </button>
        </div>
      )}
      {modal && (
        <div
          className="modal-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) setModal(null);
          }}
        >
          <div
            className={`modal ${modal === "pane" ? "command-modal" : ""} ${modal === "sessions" ? "sessions-modal" : ""} ${modal === "workspace-actions" ? "workspace-actions-modal claude-controls-modal" : ""}`}
            role="dialog"
            aria-modal="true"
            aria-label={
              modal === "pane"
                ? "Add panel"
                : modal === "workspace"
                  ? "New workspace"
                  : modal === "settings"
                    ? "Settings"
                    : modal === "routine"
                      ? "New routine"
                      : modal === "sessions"
                        ? "Session manager"
                        : modal === "workspace-actions"
                          ? "Workspace controls"
                          : modal === "updates"
                            ? "Software updates"
                            : "Notifications"
            }
          >
            <button
              className="modal-close icon-button"
              aria-label="Close dialog"
              onClick={() => setModal(null)}
            >
              <X size={17} />
            </button>
            {modal === "updates" ? (
              <UpdateSettings state={updates} />
            ) : modal === "sessions" ? (
              <SessionsDialog
                workspaces={workspaces}
                activeId={active.id}
                onCloseSessions={endSessions}
                onShow={(w, p) => {
                  switchWorkspace(w.id);
                  setZoomed(p.id);
                  setModal(null);
                }}
              />
            ) : modal === "close" && closing ? (
              <>
                <div className="dialog-eyebrow">CLOSE SESSION</div>
                <h2>{closing.panel?.title}</h2>
                <p>
                  {closing.workspace.name} · {closing.panel?.herdrId}
                </p>
                <p>
                  Hide this panel to keep its process running, or end the actual
                  session.
                </p>
                <div className="dialog-actions">
                  <button
                    className="secondary"
                    onClick={() => {
                      updateWorkspace(closing.workspace.id, (w) => ({
                        ...w,
                        layout: remove(w.layout, closing.panel!.id),
                      }));
                      window.bridge?.terminalClose(closing.panel!.id);
                      disposeTerminal(closing.panel!.id);
                      setZoomed(null);
                      setModal(null);
                    }}
                  >
                    Hide only
                  </button>
                  <button
                    className="danger"
                    onClick={async () => {
                      await endSessions([
                        { workspace: closing.workspace, panel: closing.panel! },
                      ]);
                      setModal(null);
                    }}
                  >
                    End session
                  </button>
                </div>
              </>
            ) : modal === "workspace-actions" && closing ? (
              <ClaudeMcpSettings
                cwd={closing.workspace.cwd}
                endpoint={closing.workspace.connection}
                remote={
                  closing.workspace.connection?.startsWith("ssh:") || false
                }
                workspaceName={closing.workspace.name}
                sessionCount={closing.workspace.panels.length}
                onRename={async (name) => {
                  if (closing.workspace.herdrId)
                    await window.bridge!.herdr(
                      closing.workspace.connection || socket,
                      "workspace.rename",
                      {
                        workspace_id: closing.workspace.herdrId,
                        label: name,
                      },
                    );
                  updateWorkspace(closing.workspace.id, (w) => ({
                    ...w,
                    name,
                  }));
                  setClosing((value) =>
                    value
                      ? { ...value, workspace: { ...value.workspace, name } }
                      : value,
                  );
                }}
                onCloseWorkspace={() => endWorkspace(closing.workspace)}
              />
            ) : modal === "pane" ? (
              <>
                <div className="dialog-eyebrow">MAKE IT YOUR SPACE</div>
                <h2>Add a panel</h2>
                <p>Everything you need, side by side.</p>
                <div className={`panel-options ${adding ? "is-busy" : ""}`}>
                  {(
                    [
                      {
                        kind: "terminal",
                        title: "Terminal",
                        detail: "A real shell in your project",
                        icon: TerminalSquare,
                      },
                      {
                        kind: "files",
                        title: "Files & Git",
                        detail: "Explore code, images and changes",
                        icon: FolderOpen,
                      },
                      {
                        kind: "browser",
                        title: "Browser",
                        detail: "Your local app or any website",
                        icon: Globe,
                      },
                      {
                        kind: "chat",
                        title: "Thread",
                        detail: "Talk to Claude Code or Codex",
                        icon: Sparkles,
                      },
                    ] as const
                  ).map((item) => (
                    <button key={item.kind} onClick={() => addPanel(item.kind)}>
                      <item.icon size={19} />
                      <div>
                        <strong>{item.title}</strong>
                        <small>{item.detail}</small>
                      </div>
                      <Plus size={15} />
                    </button>
                  ))}
                </div>
                <div className="dialog-eyebrow agent-options-label">
                  CODING AGENTS
                </div>
                <div className="agent-options">
                  {["claude", "codex", "gemini", "cursor-agent"].map(
                    (agent) => (
                      <button
                        key={agent}
                        onClick={() =>
                          addPanel(
                            "agent",
                            agent,
                            undefined,
                            agent === "claude"
                              ? selectedModelProfileId || undefined
                              : undefined,
                          )
                        }
                      >
                        <span
                          className={
                            agent === "claude" ? "agent-star" : "agent-logo"
                          }
                        >
                          {agent === "claude"
                            ? "✳"
                            : agent === "codex"
                              ? "✺"
                              : agent === "gemini"
                                ? "✦"
                                : "⌘"}
                        </span>
                        <span>{agentTitle(agent)}</span>
                        <small>
                          {system?.agents.find((a) => a.name === agent)?.path
                            ? "Installed"
                            : "CLI required"}
                        </small>
                      </button>
                    ),
                  )}
                </div>
                {modelProfiles.length > 0 && (
                  <label className="agent-model-picker">
                    Claude Code · Custom model
                    <select
                      value={selectedModelProfileId}
                      onChange={(event) =>
                        setSelectedModelProfileId(event.target.value)
                      }
                    >
                      <option value="">Automatic (Anthropic)</option>
                      {modelProfiles.map((profile) => (
                        <option key={profile.id} value={profile.id}>
                          {profile.label}
                        </option>
                      ))}
                    </select>
                    <small>Pick a model, then click Claude Code above.</small>
                  </label>
                )}
                <div className="dialog-footer">
                  <span>
                    Launches in <strong>{active.name}</strong>
                  </span>
                  <kbd>esc</kbd>
                </div>
              </>
            ) : modal === "settings" ? (
              <>
                <div className="dialog-eyebrow">PREFERENCES</div>
                <h2>Your workspace, connected.</h2>
                <div className="workspace-control-tabs" role="tablist">
                  {(
                    [
                      ["general", "General"],
                      ["connections", "Connections"],
                      ["providers", "Providers"],
                      ["updates", "Updates"],
                    ] as const
                  ).map(([key, label]) => (
                    <button
                      key={key}
                      className={settingsTab === key ? "selected" : ""}
                      role="tab"
                      aria-selected={settingsTab === key}
                      onClick={() => setSettingsTab(key)}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                {settingsTab === "connections" ? (
                  <ConnectionsSettings
                    endpoint={socket}
                    localSocket={system?.socketPath || ""}
                    onSelect={(value) => {
                      setSocket(value);
                      setConnection("connecting");
                    }}
                    socketForm={
                      <form
                        className="socket-form"
                        onSubmit={(event) => {
                          event.preventDefault();
                          const value = String(
                            new FormData(event.currentTarget).get("socket"),
                          );
                          setSocket(value);
                          setConnection("connecting");
                          refreshHerdr(value);
                        }}
                      >
                        <div className="socket-head">
                          <label htmlFor="settings-socket">Herdr socket</label>
                          <div className="connection-detail">
                            <i
                              className={`status-dot ${connected ? "green" : ""}`}
                            />
                            {connected
                              ? "Connected · workspaces sync automatically"
                              : connectionError || "Connecting…"}
                          </div>
                        </div>
                        <div className="socket-controls">
                          <input
                            id="settings-socket"
                            name="socket"
                            key={socket}
                            defaultValue={
                              socket.startsWith("ssh:")
                                ? system?.socketPath
                                : socket
                            }
                            placeholder="/Users/you/.config/herdr/herdr.sock"
                            required
                          />
                          <button className="primary" type="submit">
                            <RefreshCw size={14} /> Reconnect
                          </button>
                        </div>
                      </form>
                    }
                  />
                ) : settingsTab === "providers" ? (
                  <ProvidersSettings />
                ) : settingsTab === "updates" ? (
                  <UpdateSettings state={updates} />
                ) : (
                  <>
                    <div className="setting-block">
                      <h4>Interface size</h4>
                      <div
                        className="workspace-control-tabs"
                        role="group"
                        aria-label="Interface size"
                      >
                        {(
                          [
                            [0.95, "Compact"],
                            [1, "Default"],
                            [1.1, "Large"],
                          ] as const
                        ).map(([value, label]) => (
                          <button
                            key={label}
                            role="radio"
                            aria-checked={fontScale === value}
                            className={fontScale === value ? "selected" : ""}
                            onClick={() => setFontScale(value)}
                          >
                            {label}
                          </button>
                        ))}
                      </div>
                    </div>
                    <div className="settings-note">
                      <TerminalSquare size={16} />
                      <p>
                        Herdr sessions keep running when you close sushiAI.
                        Local terminals live for the duration of the app.
                      </p>
                    </div>
                    <div className="cli-status">
                      {system?.agents.map((a) => (
                        <div key={a.name}>
                          <span>{agentTitle(a.name)}</span>
                          <span>
                            {a.path ? (
                              <>
                                <Check size={12} /> Installed
                              </>
                            ) : (
                              "Not found"
                            )}
                          </span>
                        </div>
                      ))}
                    </div>
                  </>
                )}
              </>
            ) : modal === "workspace" ? (
              <WorkspaceDialog
                defaultCwd={active.cwd}
                endpoint={socket}
                connected={connected}
                onCreate={async (
                  name,
                  cwd,
                  backend,
                  starter,
                  pluginChanges,
                ) => {
                  try {
                    const pluginEndpoint =
                      backend === "herdr" && socket.startsWith("ssh:")
                        ? socket
                        : undefined;
                    const applyPluginChanges = async () => {
                      if (!pluginChanges.length) return;
                      if (!window.bridge)
                        throw new Error("Open the desktop app first.");
                      for (const plugin of pluginChanges)
                        await window.bridge.claudePluginsToggle({
                          cwd,
                          endpoint: pluginEndpoint,
                          name: plugin.name,
                          disabled: plugin.disabled,
                        });
                    };
                    if (backend === "herdr") {
                      const result = await window.bridge!.herdr(
                        socket,
                        "workspace.create",
                        { label: name, cwd, focus: false },
                      );
                      try {
                        await applyPluginChanges();
                      } catch (error) {
                        await window.bridge
                          ?.herdr(socket, "workspace.close", {
                            workspace_id: result.workspace.workspace_id,
                          })
                          .catch(() => {});
                        throw error;
                      }
                      await refreshHerdr();
                      if (starter !== "shell")
                        await window.bridge!.herdr(socket, "pane.send_input", {
                          pane_id: result.root_pane.pane_id,
                          text: starter,
                          keys: ["Enter"],
                        });
                      await refreshHerdr();
                      switchWorkspace(
                        `herdr:${socket.startsWith("ssh:") ? socket : "local"}:${result.workspace.workspace_id}`,
                      );
                    } else {
                      await applyPluginChanges();
                      const w = initialWorkspace(cwd);
                      w.name = name;
                      const panel: Panel = {
                        id: uid(),
                        kind: starter === "shell" ? "terminal" : "agent",
                        title:
                          starter === "shell" ? "zsh" : agentTitle(starter),
                        agent: starter === "shell" ? undefined : starter,
                        started: starter !== "shell",
                      };
                      w.panels = [panel];
                      w.layout = leaf(panel.id);
                      setWorkspaces((items) => [...items, w]);
                      switchWorkspace(w.id);
                    }
                    setModal(null);
                  } catch (error) {
                    notify(errorText(error));
                  }
                }}
              />
            ) : modal === "routine" ? (
              <>
                <div className="dialog-eyebrow">A LITTLE LESS REPETITION</div>
                <h2>New routine</h2>
                <form
                  onSubmit={(event) => {
                    event.preventDefault();
                    const data = new FormData(event.currentTarget);
                    setRoutines((items) => [
                      ...items,
                      {
                        id: uid(),
                        name: String(data.get("name")),
                        command: String(data.get("command")),
                      },
                    ]);
                    setModal(null);
                  }}
                >
                  <label>
                    Name
                    <input
                      name="name"
                      autoFocus
                      required
                      placeholder="Start development"
                    />
                  </label>
                  <label>
                    Command
                    <input name="command" required placeholder="npm run dev" />
                  </label>
                  <p className="muted">
                    Runs in the selected workspace when you press Play.
                  </p>
                  <button className="primary" type="submit">
                    Save routine
                  </button>
                </form>
              </>
            ) : (
              <>
                <div className="dialog-eyebrow">ACTIVITY</div>
                <h2>Your agents at a glance.</h2>
                {agentNotices.map((activity) => (
                  <div key={activity.id} className="notification-item">
                    <span>✦</span>
                    <div>
                      <strong>
                        {activity.agentName} · {activity.title}
                      </strong>
                      <p>{activity.summary}</p>
                    </div>
                    <button
                      aria-label="Dismiss activity"
                      onClick={() =>
                        setAgentNotices((old) =>
                          old.filter((a) => a.id !== activity.id),
                        )
                      }
                    >
                      ×
                    </button>
                  </div>
                ))}
                {updates?.release && (
                  <button
                    className="notification-item"
                    onClick={() => setModal("updates")}
                  >
                    <Download size={18} />
                    <div>
                      <strong>sushiAI {updates.release.version}</strong>
                      <p>
                        {updates.phase === "ready"
                          ? "Ready to install"
                          : "A new update is available"}
                      </p>
                    </div>
                    <ArrowUpRight size={15} />
                  </button>
                )}
                {blocked.length ? (
                  blocked.map(({ workspace, panel }) => (
                    <button
                      key={panel.id}
                      className="notification-item"
                      onClick={() => {
                        switchWorkspace(workspace.id);
                        setZoomed(panel.id);
                        setModal(null);
                      }}
                    >
                      <i className="status-dot yellow" />
                      <div>
                        <strong>{panel.title}</strong>
                        <p>{workspace.name} · Needs your attention</p>
                      </div>
                      <ArrowUpRight size={15} />
                    </button>
                  ))
                ) : !agentNotices.length ? (
                  <Empty
                    icon={<Check size={26} />}
                    title="All quiet for now."
                    text="Agents waiting for your input will appear here."
                  />
                ) : null}
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function LayersIcon() {
  return <LayoutGrid size={19} />;
}
function Empty({
  icon,
  title,
  text,
  action,
  onAction,
}: {
  icon: React.ReactNode;
  title: string;
  text: string;
  action?: string;
  onAction?(): void;
}) {
  return (
    <div className="empty-state">
      {icon}
      <h2>{title}</h2>
      <p>{text}</p>
      {action && (
        <button className="primary" onClick={onAction}>
          <Plus size={14} />
          {action}
        </button>
      )}
    </div>
  );
}
function WorkspaceDialog({
  defaultCwd,
  endpoint,
  connected,
  onCreate,
}: {
  defaultCwd: string;
  endpoint: string;
  connected: boolean;
  onCreate(
    name: string,
    cwd: string,
    backend: string,
    starter: string,
    pluginChanges: Array<{ name: string; disabled: boolean }>,
  ): Promise<void>;
}) {
  const [cwd, setCwd] = useState(defaultCwd),
    [busy, setBusy] = useState(false),
    [backend, setBackend] = useState(connected ? "herdr" : "local");
  const [plugins, setPlugins] = useState<ClaudePlugin[]>([]);
  const [disabledPlugins, setDisabledPlugins] = useState<string[]>([]);
  const [pluginsLoading, setPluginsLoading] = useState(false);
  const remote = backend === "herdr" && endpoint.startsWith("ssh:");
  const targetEndpoint = remote ? endpoint : undefined;
  useEffect(() => {
    let cancelled = false;
    if (remote)
      window.bridge
        ?.projectInspect(endpoint, { operation: "home" })
        .then((info) => {
          if (!cancelled) setCwd(info.home);
        })
        .catch(() => {});
    else setCwd(defaultCwd);
    return () => {
      cancelled = true;
    };
  }, [remote, endpoint, defaultCwd]);
  useEffect(() => {
    let cancelled = false;
    setPluginsLoading(true);
    setPlugins([]);
    setDisabledPlugins([]);
    const request = window.bridge?.claudePluginsList(cwd, targetEndpoint);
    if (!request) {
      setPluginsLoading(false);
      return () => {
        cancelled = true;
      };
    }
    request
      .then((result) => {
        if (cancelled) return;
        setPlugins(result.plugins);
        setDisabledPlugins(
          result.plugins
            .filter((plugin) => plugin.disabled)
            .map((plugin) => plugin.name),
        );
      })
      .catch(() => {
        if (!cancelled) {
          setPlugins([]);
          setDisabledPlugins([]);
        }
      })
      .finally(() => {
        if (!cancelled) setPluginsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, targetEndpoint]);
  return (
    <>
      <div className="dialog-eyebrow">A PLACE TO BUILD</div>
      <h2>New workspace</h2>
      <p>Keep your agents and project together.</p>
      <form
        onSubmit={async (event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          setBusy(true);
          try {
            await onCreate(
              String(data.get("name")),
              cwd,
              backend,
              String(data.get("starter")),
              plugins.flatMap((plugin) => {
                const disabled = disabledPlugins.includes(plugin.name);
                return disabled === plugin.disabled
                  ? []
                  : [{ name: plugin.name, disabled }];
              }),
            );
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Name
          <input
            name="name"
            autoFocus
            placeholder="my-next-project"
            required
            maxLength={80}
          />
        </label>
        <label>
          Project folder
          <div className="folder-field">
            <input
              value={cwd}
              onChange={(event) => setCwd(event.target.value)}
              required
              placeholder="/Users/you/projects/app"
            />
            <button
              type="button"
              title="Choose project folder"
              disabled={remote}
              onClick={async () => {
                const selected = await window.bridge?.chooseDirectory();
                if (selected) setCwd(selected);
              }}
            >
              <FolderOpen size={16} />
            </button>
          </div>
        </label>
        <label>
          Agent / harness
          <select name="starter">
            <option value="shell">One terminal</option>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
            <option value="gemini">Gemini CLI</option>
          </select>
        </label>
        {plugins.length > 0 ? (
          <label>
            Claude Code plugins to disable
            <select
              className="workspace-plugin-picker"
              multiple
              size={Math.min(5, Math.max(3, plugins.length))}
              value={disabledPlugins}
              onChange={(event) =>
                setDisabledPlugins(
                  Array.from(
                    event.currentTarget.selectedOptions,
                    (option) => option.value,
                  ),
                )
              }
            >
              {plugins.map((plugin) => (
                <option value={plugin.name} key={plugin.name}>
                  {plugin.name} · {plugin.disabled ? "off" : "on"}
                </option>
              ))}
            </select>
            <small className="workspace-plugin-hint">
              Select plugins that should be off in this workspace. Existing
              selections reflect their current state.
            </small>
          </label>
        ) : null}
        <label>
          Session backend
          <select
            value={backend}
            onChange={(event) => setBackend(event.target.value)}
          >
            <option value="herdr" disabled={!connected}>
              Herdr · persistent sessions
            </option>
            <option value="local">Local · built-in PTY</option>
          </select>
        </label>
        <button
          type="submit"
          className="primary"
          disabled={busy || pluginsLoading}
        >
          {busy
            ? "Creating…"
            : pluginsLoading
              ? "Finding plugins…"
              : "Create workspace"}
          <ArrowUpRight size={14} />
        </button>
      </form>
    </>
  );
}
function LayoutView({
  layout,
  renderPanel,
  onResize,
}: {
  layout: Layout;
  renderPanel(id: string): React.ReactNode;
  onResize(id: string, ratio: number): void;
}) {
  const container = useRef<HTMLDivElement>(null);
  if (layout.type === "leaf") return renderPanel(layout.id);
  const axis = layout.axis;
  return (
    <div ref={container} className={`split split-${axis}`}>
      <div
        className="split-child"
        style={{ flexGrow: layout.ratio, flexBasis: 0 }}
      >
        <LayoutView
          layout={layout.a}
          renderPanel={renderPanel}
          onResize={onResize}
        />
      </div>
      <div
        className={`split-handle ${axis}`}
        role="separator"
        aria-orientation={axis === "row" ? "vertical" : "horizontal"}
        aria-label="Resize panels"
        tabIndex={0}
        onKeyDown={(event) => {
          if (
            ["ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown"].includes(
              event.key,
            )
          ) {
            event.preventDefault();
            onResize(
              layout.id,
              Math.max(
                0.15,
                Math.min(
                  0.85,
                  layout.ratio +
                    (event.key === "ArrowLeft" || event.key === "ArrowUp"
                      ? -0.025
                      : 0.025),
                ),
              ),
            );
          }
        }}
        onPointerDown={(event) => {
          event.preventDefault();
          const el = event.currentTarget;
          el.setPointerCapture(event.pointerId);
          const rect = container.current!.getBoundingClientRect();
          document.body.classList.add("resizing");
          const move = (e: PointerEvent) => {
            const value =
              axis === "row"
                ? (e.clientX - rect.left) / rect.width
                : (e.clientY - rect.top) / rect.height;
            onResize(layout.id, Math.max(0.15, Math.min(0.85, value)));
          };
          const end = () => {
            el.removeEventListener("pointermove", move);
            el.removeEventListener("pointerup", end);
            el.removeEventListener("pointercancel", end);
            document.body.classList.remove("resizing");
          };
          el.addEventListener("pointermove", move);
          el.addEventListener("pointerup", end);
          el.addEventListener("pointercancel", end);
        }}
      />
      <div
        className="split-child"
        style={{ flexGrow: 1 - layout.ratio, flexBasis: 0 }}
      >
        <LayoutView
          layout={layout.b}
          renderPanel={renderPanel}
          onResize={onResize}
        />
      </div>
    </div>
  );
}
function PanelFrame({
  panel,
  selected,
  zoomed,
  dragging,
  onFocus,
  onDrag,
  onDragEnd,
  onDrop,
  onClose,
  onZoom,
  onAdd,
  onRename,
  children,
}: {
  panel: Panel;
  selected: boolean;
  zoomed: boolean;
  dragging: boolean;
  onFocus(): void;
  onDrag(): void;
  onDragEnd(): void;
  onDrop(edge: string): void;
  onClose(): void;
  onZoom(): void;
  onAdd(): void;
  onRename(title: string): void;
  children: React.ReactNode;
}) {
  const [edge, setEdge] = useState(""),
    [menu, setMenu] = useState(false),
    [rename, setRename] = useState(false);
  return (
    <section
      className={`panel ${selected ? "focused" : ""} panel-${panel.kind}`}
      data-panel-id={panel.id}
      onMouseDown={onFocus}
      onDragOver={(event) => {
        if (!dragging) return;
        event.preventDefault();
        const r = event.currentTarget.getBoundingClientRect();
        const x = (event.clientX - r.left) / r.width,
          y = (event.clientY - r.top) / r.height;
        setEdge(
          x < 0.22
            ? "left"
            : x > 0.78
              ? "right"
              : y < 0.22
                ? "top"
                : y > 0.78
                  ? "bottom"
                  : "center",
        );
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node))
          setEdge("");
      }}
      onDrop={(event) => {
        event.preventDefault();
        onDrop(edge || "center");
        setEdge("");
      }}
    >
      <header
        className="panel-header"
        draggable={!rename}
        onDragStart={(event) => {
          event.dataTransfer.effectAllowed = "move";
          event.dataTransfer.setData("text/plain", panel.id);
          onDrag();
        }}
        onDragEnd={() => {
          setEdge("");
          onDragEnd();
        }}
        onDoubleClick={onZoom}
      >
        <span
          className={`status-dot ${panel.status === "working" || (panel.kind === "agent" && panel.started) ? "green" : panel.status === "blocked" ? "yellow" : ""}`}
        />
        <Icon kind={panel.kind} agent={panel.agent} size={12} />
        {rename ? (
          <input
            className="panel-rename"
            defaultValue={panel.title}
            autoFocus
            onBlur={(event) => {
              if (event.target.value.trim())
                onRename(event.target.value.trim());
              setRename(false);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
        ) : (
          <span className="panel-title">{panel.title}</span>
        )}
        <div className="panel-actions">
          <button
            aria-label={`Options for ${panel.title}`}
            onClick={() => setMenu(!menu)}
          >
            <MoreHorizontal />
          </button>
          <button aria-label={`Maximize ${panel.title}`} onClick={onZoom}>
            {zoomed ? <Minimize2 /> : <Maximize2 />}
          </button>
          <button aria-label="Add panel" onClick={onAdd}>
            <Plus />
          </button>
          <button aria-label={`Close ${panel.title}`} onClick={onClose}>
            <X />
          </button>
        </div>
        {menu && (
          <div className="panel-menu">
            <button
              onClick={() => {
                setRename(true);
                setMenu(false);
              }}
            >
              Rename panel
            </button>
            <button
              onClick={() => {
                onZoom();
                setMenu(false);
              }}
            >
              {zoomed ? "Restore layout" : "Focus panel"}
            </button>
            <button onClick={onClose}>
              {panel.herdrId ? "Close / end session…" : "Close panel"}
            </button>
          </div>
        )}
      </header>
      <div className="panel-content">{children}</div>
      {dragging && edge && (
        <div className={`drop-zone edge-${edge}`}>
          <span>{edge === "center" ? "Swap panels" : `Place ${edge}`}</span>
        </div>
      )}
    </section>
  );
}
