import { ArrowUpRight, Check, Download } from "lucide-react";
import { Empty } from "./Empty.tsx";
import type { AgentActivity } from "../agents/types";
import type { Panel, UpdateState, Workspace } from "../types";

export function NotificationsDialog({
  agentNotices,
  setAgentNotices,
  updates,
  blocked,
  openUpdates,
  showBlockedPanel,
}: {
  agentNotices: AgentActivity[];
  setAgentNotices(update: (old: AgentActivity[]) => AgentActivity[]): void;
  updates: UpdateState | null;
  blocked: { workspace: Workspace; panel: Panel }[];
  openUpdates(): void;
  showBlockedPanel(item: { workspace: Workspace; panel: Panel }): void;
}) {
  return (
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
              setAgentNotices((old) => old.filter((a) => a.id !== activity.id))
            }
          >
            ×
          </button>
        </div>
      ))}
      {updates?.release && (
        <button className="notification-item" onClick={openUpdates}>
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
            onClick={() => showBlockedPanel({ workspace, panel })}
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
  );
}
