import { useEffect, useState } from "react";
import type { ProjectGit } from "../app/useProjectGit";
import {
  isWorktreeCleanupSelected,
  isMergedPullRequest,
  worktreeCleanupRequestKey,
  worktreeCleanupTarget,
  type WorktreeDeleteChoice,
  type WorktreeCleanupRequest,
} from "../app/worktreeCleanup";
import type { Panel, Workspace } from "../types";
import { Tag, Toggle } from "../orchestrator/ui";

/** The pull request's state as a tag at the worktree's name. */
function PrTag({ state }: { state: string }) {
  if (state === "MERGED") return <Tag tone="ok">PR merged</Tag>;
  if (state === "OPEN")
    return (
      <Tag tone="neutral" dot={false}>
        PR open
      </Tag>
    );
  if (state === "CLOSED")
    return (
      <Tag tone="neutral" dot={false}>
        PR closed
      </Tag>
    );
  return null;
}

export function CloseSessionDialog({
  workspace,
  panel,
  workspaces,
  projectGit,
  endSessions,
  onClose,
}: {
  workspace: Workspace;
  panel: Panel;
  workspaces: Workspace[];
  projectGit: Record<string, ProjectGit>;
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
  const [prState, setPrState] = useState("");
  const [ending, setEnding] = useState(false);
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
        setPrState(
          typeof (status as { state?: unknown })?.state === "string"
            ? (status as { state: string }).state
            : "",
        );
        setDeleteChoice((choice) =>
          choice.requestKey === requestKey && choice.manual
            ? choice
            : {
                requestKey,
                checked: isMergedPullRequest(status),
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
    // A second click while the first is closing would close and remove twice.
    if (checkingGit || checkingPr || ending) return;
    setEnding(true);
    const request =
      deleteWorktree && !checkingPr && cleanupTarget && !cleanupTarget.blocked
        ? { ...cleanupTarget, workspace, panel }
        : undefined;
    try {
      await endSessions([{ workspace, panel }], request);
    } finally {
      setEnding(false);
    }
    onClose();
  }

  return (
    <>
      <div className="dialog-eyebrow">CLOSE SESSION</div>
      <h2 className="cs-title">{panel.title}</h2>
      <p className="cs-meta">
        {workspace.name} · {branch || panel.sessionId || workspace.cwd}
      </p>
      <p className="cs-body">
        Closing this panel ends its session and stops the process.
      </p>
      {checkingGit && <p className="cs-hint">Checking workspace Git status…</p>}
      {cleanupTarget && (
        <>
          <label
            className={`cs-worktree ${blocked || checkingPr ? "off" : ""}`}
          >
            <Toggle
              checked={deleteWorktree}
              label={`Delete worktree ${branch}`}
              disabled={blocked || checkingPr || !window.bridge}
              onChange={(checked) =>
                setDeleteChoice({ requestKey, checked, manual: true })
              }
            />
            <span>Delete worktree {branch}</span>
            {checkingPr ? (
              <Tag tone="neutral" dot={false}>
                checking…
              </Tag>
            ) : (
              !blocked && <PrTag state={prState} />
            )}
          </label>
          <p className="cs-hint">
            {blocked
              ? "Close the other panes in this worktree first."
              : checkingPr
                ? "Checking whether its pull request was merged…"
                : "The branch stays in Git. A worktree with uncommitted changes is kept; ignored files like .env go with it."}
          </p>
        </>
      )}
      <div className="cs-actions">
        <button
          className="ui-button secondary"
          disabled={ending}
          onClick={onClose}
        >
          Cancel
        </button>
        <button
          className="ui-button danger"
          disabled={checkingGit || checkingPr || ending}
          onClick={() => void endSession()}
        >
          {ending
            ? "Closing…"
            : deleteWorktree
              ? "Close session and delete worktree"
              : "Close session"}
        </button>
      </div>
    </>
  );
}
