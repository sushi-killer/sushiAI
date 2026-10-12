import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Check,
  CircleHelp,
  CircleSlash,
  Eye,
  GitPullRequest,
  MessageSquareReply,
  Newspaper,
  TriangleAlert,
} from "lucide-react";
import { useOrchestratorClient, useOrchestratorHost } from "./hostContext";
import { errorText, formatCost } from "./helpers";
import { hostOf } from "./hosts";
import {
  addEntry,
  belongsToProject,
  detailText,
  displayText,
  entryKey,
  fullDateTime,
  groupByDay,
  rowTarget,
  spentLine,
  spentTodayQuery,
  toggleOpen,
} from "./overseerLogModel";
import type { OverseerEntry } from "./types";
import "./overseer-log.css";

const KINDS: Record<OverseerEntry["kind"], { icon: ReactNode; label: string }> =
  {
    woke: { icon: <Eye size={12} aria-hidden />, label: "Woke" },
    answered: {
      icon: <MessageSquareReply size={12} aria-hidden />,
      label: "Answered a task",
    },
    pr: {
      icon: <GitPullRequest size={12} aria-hidden />,
      label: "Pull request",
    },
    asked_owner: {
      icon: <CircleHelp size={12} aria-hidden />,
      label: "Left a note for you",
    },
    done: { icon: <Check size={12} aria-hidden />, label: "Finished" },
    digest: { icon: <Newspaper size={12} aria-hidden />, label: "Digest" },
    skipped: { icon: <CircleSlash size={12} aria-hidden />, label: "Skipped" },
    failed: { icon: <TriangleAlert size={12} aria-hidden />, label: "Failed" },
  };

function timeLabel(ts: number): string {
  return new Date(ts).toLocaleTimeString("en-US", {
    hour: "numeric",
    minute: "2-digit",
  });
}

/** The overseer's Log: what the orchestrator agent did in this panel's
 * project, newest first, with the project's spend today on top. */
export function OverseerLogView({
  cwd,
  dailyBudgetUsd,
  onOpenTask,
  onOpenChat,
}: {
  cwd: string;
  /** The panel's last read of the setting; the view reads it again itself. */
  dailyBudgetUsd: number;
  /** Opens a task of this project. */
  onOpenTask: (taskId: string) => void;
  /** Opens the chat of this project. */
  onOpenChat: () => void;
}) {
  const client = useOrchestratorClient();
  const host = useOrchestratorHost();
  const [entries, setEntries] = useState<OverseerEntry[] | null>(null);
  const [spent, setSpent] = useState<{ project: number; all: number } | null>(
    null,
  );
  const [budget, setBudget] = useState(dailyBudgetUsd);
  const [error, setError] = useState("");
  const [openKey, setOpenKey] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    // Today's spend and the budget, read again on every overseer event.
    const refresh = () => {
      Promise.all([
        client.costsSummary(spentTodayQuery(cwd)),
        client.costsSummary(spentTodayQuery()),
      ])
        .then(
          ([project, all]) =>
            !cancelled &&
            setSpent({
              project: project.totals.costUsd,
              all: all.totals.costUsd,
            }),
        )
        .catch(() => {});
      client
        .settingsGet()
        .then((s) => !cancelled && setBudget(s.dailyBudgetUsd ?? 0))
        .catch(() => {});
    };
    client
      .overseerLog(cwd, 2000)
      // Entries that arrived while the list loaded stay.
      .then(
        (list) =>
          !cancelled && setEntries((old) => (old ?? []).reduce(addEntry, list)),
      )
      .catch((e) => !cancelled && setError(errorText(e)));
    refresh();
    const off = window.bridge?.onOrchestrator((event) => {
      if (
        event.event !== "overseer" ||
        hostOf(event) !== host ||
        !belongsToProject(event.entry, cwd)
      )
        return;
      setEntries((old) => addEntry(old ?? [], event.entry));
      refresh();
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [client, host, cwd]);

  const days = useMemo(() => groupByDay(entries ?? []), [entries]);

  const open = (entry: OverseerEntry) =>
    rowTarget(entry) === "chat" ? onOpenChat() : onOpenTask(entry.taskId!);
  const opens = (entry: OverseerEntry) =>
    rowTarget(entry) === "chat" || !!entry.taskId;

  return (
    <div className="orch-view-scroll log-view">
      <div className="log-head">
        <h2 className="orch-view-title">Log</h2>
        {spent !== null && (
          <p className="log-spent">
            {spentLine(spent.project, spent.all, budget)}
          </p>
        )}
      </div>
      {error && (
        <p className="log-empty" role="alert">
          {error}
        </p>
      )}
      {entries && entries.length === 0 && !error && (
        <p className="log-empty">
          Nothing yet. When the overseer wakes, answers a task or opens a PR, it
          shows up here.
        </p>
      )}
      {days.map((day) => (
        <section key={day.label} className="log-day" aria-label={day.label}>
          <h3 className="orch-eyebrow">{day.label.toUpperCase()}</h3>
          <ul className="log-list">
            {day.entries.map((entry) => {
              const kind = KINDS[entry.kind] ?? KINDS.woke;
              const key = entryKey(entry);
              const expanded = openKey === key;
              const panelId = `log-detail-${key.replace(/[^\w-]/g, "_")}`;
              return (
                <li key={key}>
                  <button
                    type="button"
                    className="log-row clickable"
                    aria-expanded={expanded}
                    aria-controls={expanded ? panelId : undefined}
                    onClick={() => setOpenKey((cur) => toggleOpen(cur, key))}
                  >
                    <span className="log-time">{timeLabel(entry.ts)}</span>
                    <span
                      className={`log-kind log-kind-${entry.kind}`}
                      role="img"
                      aria-label={kind.label}
                      title={kind.label}
                    >
                      {kind.icon}
                    </span>
                    <span className="log-text">{displayText(entry)}</span>
                  </button>
                  {expanded && (
                    <div className="log-detail" id={panelId}>
                      <p className="log-meta">
                        {[
                          fullDateTime(entry.ts),
                          kind.label,
                          entry.costUsd ? formatCost(entry.costUsd) : "",
                        ]
                          .filter(Boolean)
                          .join(" \u00b7 ")}
                      </p>
                      <pre className="log-full">{detailText(entry)}</pre>
                      {opens(entry) && (
                        <div className="log-actions">
                          <button
                            type="button"
                            className="log-link"
                            onClick={() => open(entry)}
                          >
                            {rowTarget(entry) === "chat"
                              ? "Open chat"
                              : "Open task"}
                          </button>
                        </div>
                      )}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}
