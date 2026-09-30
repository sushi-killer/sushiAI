import { useState, type ReactNode } from "react";
import {
  Archive,
  ChartColumn,
  ChevronDown,
  ChevronRight,
  LayoutList,
  ListTodo,
  MessageSquare,
  Sparkles,
} from "lucide-react";
import type { OrchestratorView } from "../types";
import {
  childrenOf,
  railGroups,
  subtaskState,
  taskMetaLine,
  taskReason,
  taskTone,
} from "./helpers";
import { planOnly } from "./planModel";
import type { Task } from "./types";
import { GroupLabel, TaskRow } from "./ui";

function NavRow({
  icon,
  label,
  selected,
  trailing,
  trailingClass = "",
  onClick,
}: {
  icon: ReactNode;
  label: string;
  selected: boolean;
  trailing?: ReactNode;
  trailingClass?: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`orch-rail-nav${selected ? " selected" : ""}`}
      aria-current={selected || undefined}
      onClick={onClick}
    >
      {icon}
      <span className="orch-rail-nav-label">{label}</span>
      {trailing !== undefined && trailing !== "" && (
        <span className={`orch-rail-nav-trailing ${trailingClass}`}>
          {trailing}
        </span>
      )}
    </button>
  );
}

/** The orchestrator's left rail (Figma "rail"): the host selector, the
 * views, then the tasks grouped by what they need (a parent's subtasks
 * nested under it, EARLIER folded into one row), and the archive pinned to
 * the bottom. Unstarted drafts live on the Plan, not here. */
export function OrchRail({
  view,
  tasks,
  maxAttempts,
  planCount,
  chatNew,
  improvementsCount,
  archivedCount,
  offline = false,
  header,
  bare = false,
  onOpen,
}: {
  view: OrchestratorView;
  /** Every task of the repo, archived included. */
  tasks: Task[];
  maxAttempts?: number;
  planCount: number;
  chatNew: number;
  improvementsCount: number;
  archivedCount: number;
  /** The daemon is down: the last known list stays, dimmed and inert. */
  offline?: boolean;
  /** Pinned above the views: the host selector. */
  header?: ReactNode;
  /** Views only, no task groups: the host has no daemon to list yet. */
  bare?: boolean;
  onOpen: (view: OrchestratorView) => void;
}) {
  const [earlierOpen, setEarlierOpen] = useState(false);
  const drafts = planOnly(tasks);
  const groups = railGroups(tasks.filter((task) => !drafts.has(task)));
  const selectedId = view.kind === "task" ? view.id : undefined;
  const empty =
    groups.needsYou.length +
      groups.running.length +
      groups.landedToday.length +
      groups.earlier.length ===
    0;

  function row(task: Task) {
    const children = childrenOf(tasks, task.id).filter((c) => !c.archived);
    const taskRow = (
      <TaskRow
        key={task.id}
        title={task.title}
        reason={taskReason(task, tasks, maxAttempts)}
        meta={taskMetaLine(task, maxAttempts)}
        tone={taskTone(task)}
        selected={task.id === selectedId}
        onClick={() => onOpen({ kind: "task", id: task.id })}
      />
    );
    if (children.length === 0) return taskRow;
    return (
      <div key={task.id} className="orch-rail-group">
        {taskRow}
        <div className="orch-rail-subtasks">
          {children.map((child) => {
            const state = subtaskState(child, tasks, maxAttempts);
            return (
              <button
                key={child.id}
                type="button"
                className={`orch-rail-sub${child.id === selectedId ? " selected" : ""}${state.label === "waits" ? " waits" : ""}`}
                aria-current={child.id === selectedId || undefined}
                onClick={() => onOpen({ kind: "task", id: child.id })}
              >
                <span className={`ui-dot ui-tone-${state.tone}`} />
                <span className="orch-rail-sub-title">{child.title}</span>
                <span className="orch-rail-sub-state">{state.label}</span>
              </button>
            );
          })}
        </div>
      </div>
    );
  }

  function group(label: string, list: Task[]) {
    if (list.length === 0) return null;
    return (
      <section className="orch-rail-section" aria-label={label}>
        <GroupLabel label={label.toUpperCase()} count={list.length} />
        {list.map(row)}
      </section>
    );
  }

  /** EARLIER stays one row until opened: the rail keeps what needs you and
   * what runs in view, with the archive pinned under it. */
  function earlier(list: Task[]) {
    if (list.length === 0) return null;
    return (
      <section className="orch-rail-section" aria-label="Earlier">
        <button
          type="button"
          className="orch-rail-earlier"
          aria-expanded={earlierOpen}
          onClick={() => setEarlierOpen((open) => !open)}
        >
          <span className="orch-rail-nav-label">Earlier</span>
          <span className="orch-rail-nav-trailing">{list.length}</span>
          {earlierOpen ? (
            <ChevronDown size={12} aria-hidden />
          ) : (
            <ChevronRight size={12} aria-hidden />
          )}
        </button>
        {earlierOpen && list.map(row)}
      </section>
    );
  }

  const is = (kind: OrchestratorView["kind"]) => view.kind === kind;
  return (
    <nav
      className={`orch-rail${offline ? " offline" : ""}`}
      aria-label="Orchestrator"
    >
      {header}
      <div className="orch-rail-scroll" inert={offline || undefined}>
        <NavRow
          icon={<LayoutList size={14} />}
          label="Home"
          selected={is("home")}
          onClick={() => onOpen({ kind: "home" })}
        />
        <NavRow
          icon={<ListTodo size={14} />}
          label="Plan"
          selected={is("plan") || is("brainstorm")}
          trailing={planCount || ""}
          onClick={() => onOpen({ kind: "plan" })}
        />
        <NavRow
          icon={<MessageSquare size={14} />}
          label="Chat"
          selected={is("chat") || is("messages")}
          trailing={chatNew > 0 ? `${chatNew} new` : ""}
          onClick={() => onOpen({ kind: "chat" })}
        />
        <NavRow
          icon={<Sparkles size={14} />}
          label="Improvements"
          selected={is("improvements")}
          trailing={improvementsCount || ""}
          trailingClass="accent"
          onClick={() => onOpen({ kind: "improvements" })}
        />
        <NavRow
          icon={<ChartColumn size={14} />}
          label="Analytics"
          selected={is("analytics")}
          onClick={() => onOpen({ kind: "analytics" })}
        />
        {bare ? null : empty ? (
          <div className="orch-rail-empty">
            <span>No tasks yet</span>
            <span className="orch-rail-empty-hint">
              Tasks you start appear here.
            </span>
          </div>
        ) : (
          <>
            {group("Needs you", groups.needsYou)}
            {group("Running", groups.running)}
            {group("Landed today", groups.landedToday)}
            {earlier(groups.earlier)}
          </>
        )}
      </div>
      {(archivedCount > 0 || is("archive")) && (
        <button
          type="button"
          className={`orch-rail-archive${is("archive") ? " selected" : ""}`}
          aria-current={is("archive") || undefined}
          inert={offline || undefined}
          onClick={() => onOpen({ kind: "archive" })}
        >
          <Archive size={14} />
          <span className="orch-rail-nav-label">Archive</span>
          <span className="orch-rail-nav-trailing">{archivedCount}</span>
        </button>
      )}
    </nav>
  );
}
