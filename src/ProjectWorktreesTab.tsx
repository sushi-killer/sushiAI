import { useCallback, useEffect, useState } from "react";
import { RefreshCw, Trash2 } from "lucide-react";
import type {
  ConnectionProfile,
  Project,
  WorktreeList,
  Workspace,
} from "./types";
import { Tag, Toggle } from "./orchestrator/ui";
import { useWorktreeClaims } from "./app/useWorktreeClaims";
import { relativeTime } from "./chat-threads";
import { ProjectPage } from "./ProjectPage";
import { tildePath } from "./projectPrepare";
import {
  hostLabel,
  hostOf,
  removable,
  removalLoss,
  worktreeRows,
  type WorktreeRow,
} from "./projectWorktrees";

const errorText = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);

/** One row's state tag at its branch: what it is, not how it is doing. */
function StateTag({ row }: { row: WorktreeRow }) {
  if (row.worktree.main)
    return (
      <Tag tone="neutral" dot={false}>
        main checkout
      </Tag>
    );
  if (row.worktree.locked)
    return (
      <Tag tone="neutral" dot={false}>
        locked
      </Tag>
    );
  if (row.claim)
    return (
      <span title={row.claim.label}>
        <Tag tone="neutral" dot={false}>
          claimed
        </Tag>
      </span>
    );
  if (row.open.length)
    return (
      <Tag tone="info" dot={false}>
        open
      </Tag>
    );
  return null;
}

function Status({ row }: { row: WorktreeRow }) {
  const { worktree } = row;
  const ahead =
    !worktree.merged && worktree.ahead ? (
      <span className="pd-faint">{worktree.ahead} ahead</span>
    ) : null;
  if (!worktree.exists)
    return (
      <>
        <Tag tone="danger">folder missing</Tag>
        {ahead}
      </>
    );
  if (worktree.changes)
    return (
      <>
        <Tag tone="warning">{worktree.changes} changed</Tag>
        {ahead}
      </>
    );
  if (worktree.merged && !worktree.main)
    return (
      <span title="Squash merges count">
        <Tag tone="ok">merged</Tag>
      </span>
    );
  return ahead ?? <span className="pd-faint">clean</span>;
}

type Confirm = { rows: WorktreeRow[]; deleteBranch: boolean };

/** The removal throws away work: uncommitted changes, or a branch whose
 * commits are not in the base goes with it. */
const loses = ({ rows, deleteBranch }: Confirm) =>
  rows.some(
    ({ worktree }) =>
      (worktree.exists && worktree.changes > 0) ||
      (deleteBranch && !worktree.merged && worktree.ahead > 0),
  );

export function ProjectWorktreesTab({
  project,
  cwd,
  endpoint,
  hosts,
  workspaces,
  onEndWorkspace,
}: {
  project: Project | null;
  cwd: string;
  endpoint?: string;
  hosts: ConnectionProfile[];
  workspaces: Workspace[];
  onEndWorkspace(workspace: Workspace): Promise<void>;
}) {
  const claims = useWorktreeClaims();
  const [lists, setLists] = useState<WorktreeList[] | null>(null);
  const [error, setError] = useState("");
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [busy, setBusy] = useState(false);
  const [home, setHome] = useState("");

  useEffect(() => {
    window.bridge
      ?.system()
      .then((system) => setHome(system.home))
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    if (!window.bridge) return;
    setError("");
    try {
      setLists(
        await window.bridge.worktreesList(project?.id ?? null, endpoint, cwd),
      );
    } catch (reason) {
      setError(errorText(reason));
      setLists([]);
    }
  }, [project?.id, endpoint, cwd]);

  useEffect(() => {
    void load();
  }, [load]);

  const rows = worktreeRows(lists ?? [], workspaces, claims, hosts);
  const failed = (lists ?? []).filter((list) => "error" in list);
  const linked = rows.filter((row) => !row.worktree.main);
  const merged = linked.filter(
    (row) => row.worktree.merged && !row.worktree.changes && removable(row),
  );
  const missing = linked.filter((row) => !row.worktree.exists).length;
  const hostCount = new Set(rows.map((row) => row.host)).size;

  async function remove({ rows: targets, deleteBranch }: Confirm) {
    if (!window.bridge) return;
    setBusy(true);
    setError("");
    try {
      // The dialog's own worktree goes last: ending its workspace closes the
      // dialog, and every other removal must already be done by then.
      const own = (row: WorktreeRow) =>
        row.host === hostOf(endpoint) &&
        (cwd === row.worktree.path || cwd.startsWith(`${row.worktree.path}/`));
      const ordered = [...targets].sort((a, b) => +own(a) - +own(b));
      const removed: WorktreeRow[] = [];
      try {
        for (const row of ordered) {
          await window.bridge.worktreeRemove(
            row.host,
            row.cwd,
            row.worktree.path,
            // Only the changes the owner saw in the confirmation; a checkout
            // that changed since is kept.
            // and the branch and head they saw, so a worktree that moved
            // since (a new branch, a new commit) is kept with its branch.
            {
              deleteBranch,
              discardChanges: row.worktree.changes > 0,
              branch: row.worktree.branch,
              head: row.worktree.head,
            },
          );
          removed.push(row);
        }
      } finally {
        // The folder is gone, so a session still in it has nowhere to run.
        for (const row of removed)
          for (const workspace of row.open) await onEndWorkspace(workspace);
      }
      setConfirm(null);
    } catch (reason) {
      setError(errorText(reason));
      setConfirm(null);
    } finally {
      setBusy(false);
      void load();
    }
  }

  const single = confirm?.rows.length === 1 ? confirm.rows[0] : null;
  return (
    <>
      <ProjectPage
        title="Worktrees"
        subtitle="Checkouts made from this project. Remove finished ones to free disk space."
      >
        <div className="pd-toolbar">
          <span>
            {lists === null
              ? "Reading worktrees…"
              : `${linked.length} ${linked.length === 1 ? "worktree" : "worktrees"} on ${hostCount} ${hostCount === 1 ? "host" : "hosts"} · ${merged.length} merged${missing ? ` · ${missing} folder missing` : ""}`}
          </span>
          <button
            className="ui-button secondary"
            onClick={() => void load()}
            disabled={busy}
          >
            <RefreshCw size={14} aria-hidden />
            Refresh
          </button>
          <button
            className="ui-button secondary"
            disabled={busy || !merged.length}
            onClick={() => setConfirm({ rows: merged, deleteBranch: true })}
          >
            <Trash2 size={14} aria-hidden />
            Remove {merged.length || ""} merged…
          </button>
        </div>
        {error && (
          <p role="alert" className="pd-alert">
            {error}
          </p>
        )}
        {failed.map((list) => (
          <p role="alert" className="pd-alert" key={list.host + list.cwd}>
            {hostLabel(list.host, hosts)}: {"error" in list ? list.error : ""}
          </p>
        ))}
        {lists !== null && !rows.length && !failed.length && !error && (
          <p className="pd-empty">This project has no git checkout yet.</p>
        )}
        {rows.length > 0 && (
          <div className="pd-table pd-wt" role="table" aria-label="Worktrees">
            <div className="pd-row head" role="row">
              <span role="columnheader">Branch</span>
              <span role="columnheader">Host</span>
              <span role="columnheader">Commit</span>
              <span role="columnheader">Status</span>
              <span role="columnheader" className="pd-wt-action">
                Action
              </span>
            </div>
            {rows.map((row) => (
              <div className="pd-row" role="row" key={row.key}>
                <span className="pd-wt-branch" role="cell">
                  <span className="pd-flex">
                    <span className="pd-cell pd-mono pd-wt-name">
                      {row.worktree.branch ||
                        row.worktree.head.slice(0, 7) ||
                        "detached"}
                    </span>
                    <StateTag row={row} />
                  </span>
                  <small
                    className="pd-cell pd-mono pd-faint"
                    title={row.worktree.path}
                  >
                    {row.host === "local"
                      ? tildePath(row.worktree.path, home)
                      : row.worktree.path}
                  </small>
                </span>
                <span className="pd-cell" role="cell">
                  <Tag tone="neutral" dot={false}>
                    {row.hostLabel}
                  </Tag>
                </span>
                <span className="pd-cell pd-muted" role="cell">
                  {relativeTime(row.worktree.committedAt) || "—"}
                </span>
                <span className="pd-flex pd-wt-status" role="cell">
                  <Status row={row} />
                </span>
                <span className="pd-wt-action" role="cell">
                  {!row.worktree.main && (
                    <button
                      className="ui-button secondary icon"
                      aria-label={`Remove ${row.worktree.branch || row.worktree.path}`}
                      title={
                        removable(row)
                          ? "Remove…"
                          : row.worktree.locked
                            ? "Locked with git worktree lock"
                            : `"${row.claim?.label}" still uses it`
                      }
                      disabled={busy || !removable(row)}
                      onClick={() =>
                        setConfirm({
                          rows: [row],
                          deleteBranch: row.worktree.merged,
                        })
                      }
                    >
                      <Trash2 size={12} aria-hidden />
                    </button>
                  )}
                </span>
              </div>
            ))}
          </div>
        )}
        <p className="pd-note">
          Removing deletes the folder and stops its sessions. Unmerged branches
          are kept unless you choose otherwise.
        </p>
      </ProjectPage>
      {confirm && (
        <div className="pd-confirm">
          <div
            className="pd-confirm-box"
            role="alertdialog"
            aria-label="Remove worktree"
          >
            <div className="dialog-eyebrow">WORKTREE</div>
            <h2>
              {single
                ? `Remove ${single.worktree.branch || "this worktree"}?`
                : `Remove ${confirm.rows.length} merged worktrees?`}
            </h2>
            <p>
              {single
                ? removalLoss(single)
                : `Their folders are deleted and their branches with them: ${confirm.rows.map((row) => row.worktree.branch).join(", ")}.`}
            </p>
            {single?.worktree.branch && (
              <label className="pd-wt-branch-toggle">
                <Toggle
                  checked={confirm.deleteBranch}
                  label={`Also delete branch ${single.worktree.branch}`}
                  onChange={(deleteBranch) =>
                    setConfirm({ ...confirm, deleteBranch })
                  }
                />
                Also delete branch {single.worktree.branch}
              </label>
            )}
            <div className="pd-confirm-actions">
              <button
                className="ui-button secondary"
                disabled={busy}
                onClick={() => setConfirm(null)}
              >
                Cancel
              </button>
              <button
                className={`ui-button ${loses(confirm) ? "danger" : "primary"}`}
                disabled={busy}
                onClick={() => void remove(confirm)}
              >
                {busy
                  ? "Removing…"
                  : single
                    ? "Remove worktree"
                    : "Remove worktrees"}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
