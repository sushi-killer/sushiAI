import { useState } from "react";
import { Eye, Search, TerminalSquare, Trash2 } from "lucide-react";
import type { Panel, Workspace } from "./types";
export function SessionsDialog({
  workspaces,
  activeId,
  onCloseSessions,
  onShow,
}: {
  workspaces: Workspace[];
  activeId: string;
  onCloseSessions(
    items: { workspace: Workspace; panel: Panel }[],
  ): Promise<void>;
  onShow(workspace: Workspace, panel: Panel): void;
}) {
  const [query, setQuery] = useState(""),
    [scope, setScope] = useState(activeId),
    [filter, setFilter] = useState("all"),
    [checked, setChecked] = useState<string[]>([]),
    [confirming, setConfirming] = useState(false),
    [busy, setBusy] = useState(false);
  const rows = workspaces
    .filter((w) => !scope || w.id === scope)
    .flatMap((workspace) =>
      workspace.panels.map((panel) => ({ workspace, panel })),
    )
    .filter(
      ({ workspace, panel }) =>
        (workspace.name + panel.title)
          .toLowerCase()
          .includes(query.toLowerCase()) &&
        (filter === "all" || filter === "shells"
          ? filter !== "shells" || panel.kind === "terminal"
          : panel.status === filter),
    );
  const selected = rows.filter((row) => checked.includes(row.panel.id));
  return (
    <>
      <div className="dialog-eyebrow">SESSION MANAGER</div>
      <h2>Keep your workspace clear.</h2>
      <p>
        Hide a view or end the actual session. Ending Herdr sessions stops their
        processes.
      </p>
      <div className="session-filters">
        <label className="catalog-search">
          <Search size={14} />
          <input
            aria-label="Search sessions"
            placeholder="Search sessions…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setConfirming(false);
            }}
          />
        </label>
        <select
          aria-label="Session workspace"
          value={scope}
          onChange={(e) => {
            setScope(e.target.value);
            setChecked([]);
            setConfirming(false);
          }}
        >
          <option value="">All workspaces</option>
          {workspaces.map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
        </select>
        <select
          aria-label="Session status"
          value={filter}
          onChange={(e) => {
            setFilter(e.target.value);
            setConfirming(false);
          }}
        >
          <option value="all">All sessions</option>
          <option value="shells">Shells only</option>
          <option value="idle">Idle agents</option>
          <option value="done">Finished agents</option>
          <option value="blocked">Needs input</option>
        </select>
      </div>
      <div className="session-select-all">
        <label>
          <input
            type="checkbox"
            checked={rows.length > 0 && selected.length === rows.length}
            onChange={(e) => {
              setChecked(e.target.checked ? rows.map((r) => r.panel.id) : []);
              setConfirming(false);
            }}
          />{" "}
          Select visible ({rows.length})
        </label>
        <span>{selected.length} selected</span>
      </div>
      <div className="sessions-list">
        {rows.map(({ workspace, panel }) => (
          <div className="session-row" key={panel.id}>
            <input
              aria-label={`Select session ${panel.title} ${panel.id}`}
              type="checkbox"
              checked={checked.includes(panel.id)}
              onChange={(e) => {
                setChecked((ids) =>
                  e.target.checked
                    ? [...ids, panel.id]
                    : ids.filter((id) => id !== panel.id),
                );
                setConfirming(false);
              }}
            />
            <TerminalSquare size={14} />
            <div>
              <strong>{panel.title}</strong>
              <small>
                {workspace.name} · {panel.herdrId || "Local"} ·{" "}
                {panel.status || panel.kind}
              </small>
            </div>
            <button
              title={`Show ${panel.title}`}
              onClick={() => onShow(workspace, panel)}
            >
              <Eye size={14} />
            </button>
          </div>
        ))}
      </div>
      {confirming ? (
        <div className="delete-confirm">
          <p>
            End these {selected.length} selected sessions? Running commands will
            stop. Project files will stay on disk.
          </p>
          <button onClick={() => setConfirming(false)}>Cancel</button>
          <button
            className="danger"
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                await onCloseSessions(selected);
                setChecked([]);
                setConfirming(false);
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? "Closing…" : `End ${selected.length} sessions`}
          </button>
        </div>
      ) : (
        <button
          className="danger session-close-button"
          disabled={!selected.length}
          onClick={() => setConfirming(true)}
        >
          <Trash2 size={13} /> End selected sessions
        </button>
      )}
    </>
  );
}
