import { useEffect, useState } from "react";
import type { ProjectGit } from "../app/useProjectGit";
import {
  isWorktreeCleanupSelected,
  isMergedAndClosedPullRequest,
  worktreeCleanupRequestKey,
  worktreeCleanupTarget,
  type WorktreeDeleteChoice,
  type WorktreeCleanupRequest,
} from "../app/worktreeCleanup";
import type { Panel, Workspace } from "../types";

export function CloseSessionDialog({
  workspace,
  panel,
  workspaces,
  projectGit,
  hidePanel,
  endSessions,
  onClose,
}: {
  workspace: Workspace;
  panel: Panel;
  workspaces: Workspace[];
  projectGit: Record<string, ProjectGit>;
  hidePanel(workspaceId: string, panelId: string): void;
  endSessions(
    items: { workspace: Workspace; panel: Panel }[],
    cleanup?: WorktreeCleanupRequest,
  ): Promise<void>;
  onClose(): void;
}) {
  const cleanupTarget = worktreeCleanupTarget(
    workspace,
    panel,
    workspaces,
    projectGit,
  );
  const checkout = cleanupTarget?.checkout;
  const branch = cleanupTarget?.branch;
  const blocked = cleanupTarget?.blocked ?? false;
  const requestKey = worktreeCleanupRequestKey(
    workspace.connection,
    cleanupTarget,
  );
  const checkingGit = Boolean(workspace.cwd) && !projectGit[workspace.id];
  const [deleteChoice, setDeleteChoice] = useState<WorktreeDeleteChoice>({
    requestKey: "",
    checked: false,
    manual: false,
  });
  const [prResultKey, setPrResultKey] = useState("");
  const canCheckPr = Boolean(requestKey && !blocked && window.bridge);
  const checkingPr = canCheckPr && prResultKey !== requestKey;
  const deleteWorktree = isWorktreeCleanupSelected(deleteChoice, requestKey);

  useEffect(() => {
    if (!checkout || !branch || blocked || !window.bridge) {
      return;
    }
    let current = true;
    window.bridge
      .projectInspect(workspace.connection, {
        operation: "git_pr_status",
        root: checkout,
        branch,
      })
      .then((status) => {
        if (!current) return;
        setPrResultKey(requestKey);
        setDeleteChoice((choice) =>
          choice.requestKey === requestKey && choice.manual
            ? choice
            : {
                requestKey,
                checked: isMergedAndClosedPullRequest(status),
                manual: false,
              },
        );
      })
      .catch(() => {
        if (!current) return;
        setPrResultKey(requestKey);
        setDeleteChoice((choice) =>
          choice.requestKey === requestKey && choice.manual
            ? choice
            : { requestKey, checked: false, manual: false },
        );
      });
    return () => {
      current = false;
    };
  }, [workspace.connection, checkout, branch, blocked, requestKey]);

  async function endSession() {
    if (checkingGit || checkingPr) return;
    const request =
      deleteWorktree && !checkingPr && cleanupTarget && !cleanupTarget.blocked
        ? { ...cleanupTarget, workspace, panel }
        : undefined;
    await endSessions([{ workspace, panel }], request);
    onClose();
  }

  function hideSession() {
    hidePanel(workspace.id, panel.id);
    onClose();
  }

  return (
    <>
      <div className="dialog-eyebrow">CLOSE SESSION</div>
      <h2>{panel.title}</h2>
      <p>
        {workspace.name} · {panel.herdrId}
      </p>
      <p>
        Hide this panel to keep its process running, or end the actual session.
      </p>
      {checkingGit && <p>Checking workspace Git status…</p>}
      {cleanupTarget && (
        <label className="setting-check">
          <input
            type="checkbox"
            checked={deleteWorktree}
            disabled={blocked || checkingPr || !window.bridge}
            onChange={(event) => {
              setDeleteChoice({
                requestKey,
                checked: event.target.checked,
                manual: true,
              });
            }}
          />
          <span>
            <strong>Delete this worktree after ending the session</strong>
            <em>
              {blocked
                ? "Close other panes in this worktree first."
                : checkingPr
                  ? "Checking whether its pull request was merged and closed…"
                  : "The branch stays in Git. Uncommitted files keep the worktree."}
            </em>
          </span>
        </label>
      )}
      <div className="dialog-actions">
        <button className="secondary" onClick={hideSession}>
          Hide only
        </button>
        <button
          className="danger"
          disabled={checkingGit || checkingPr}
          onClick={() => void endSession()}
        >
          {deleteWorktree ? "End session and delete worktree" : "End session"}
        </button>
      </div>
    </>
  );
}
