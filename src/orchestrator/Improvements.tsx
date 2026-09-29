import { useEffect, useState } from "react";
import { orchestratorClient } from "./client";
import type { FailureRow, Note, Proposal } from "./types";

const TOP_FAILURES = 10;

/** The failures that keep coming back across this repo's tasks, most
 * frequent first; a row opens the task where it was seen last. `refresh`
 * changes whenever the task list does. */
function RecurringFailures({
  cwd,
  refresh,
  onOpenTask,
}: {
  cwd: string;
  refresh: string;
  onOpenTask: (id: string) => void;
}) {
  const [rows, setRows] = useState<FailureRow[]>([]);

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .failuresCatalogue(cwd)
      .then((loaded) => !cancelled && setRows(loaded))
      .catch(() => !cancelled && setRows([]));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh]);

  const top = rows.filter((row) => row.count > 1).slice(0, TOP_FAILURES);
  if (top.length === 0) return null;
  return (
    <div className="orch-failures">
      <span className="dialog-eyebrow">RECURRING FAILURES</span>
      {top.map((row) => (
        <button
          key={row.signature}
          className="orch-failure-row"
          title={row.exampleDetail}
          onClick={() => onOpenTask(row.exampleTaskId)}
        >
          <span className="orch-failure-count">{row.count}×</span>
          <span className="orch-failure-kind">{row.kind}</span>
          {/* The signature has its digits stripped for grouping; show a real one. */}
          <span className="orch-failure-signature">
            {row.exampleDetail.split("\n")[0] || row.signature}
          </span>
          <span className="orch-failure-tasks">
            {row.tasks.length} {row.tasks.length === 1 ? "task" : "tasks"}
          </span>
        </button>
      ))}
    </div>
  );
}

function upsertProposal(rows: Proposal[], proposal: Proposal): Proposal[] {
  const index = rows.findIndex((row) => row.id === proposal.id);
  if (index < 0) return [proposal, ...rows];
  return rows.map((row, i) => (i === index ? proposal : row));
}

/** The daemon's evolution proposals for this repo: what to change, the
 * evidence and the metric that will judge it. A repo-track one is approved
 * (it becomes a task) or rejected; a harness-track one carries the A/B eval
 * command and is marked adopted by hand. `refresh` changes whenever the task
 * list does. */
function EvolutionProposals({
  cwd,
  refresh,
}: {
  cwd: string;
  refresh: string;
}) {
  const [rows, setRows] = useState<Proposal[]>([]);
  const [errors, setErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .evolutionList(cwd)
      .then((loaded) => !cancelled && setRows(loaded))
      .catch(() => !cancelled && setRows([]));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh]);

  useEffect(() => {
    const off = window.bridge?.onOrchestrator((event) => {
      if (event.event === "proposal" && event.proposal.repo === cwd)
        setRows((old) => upsertProposal(old, event.proposal));
    });
    return () => off?.();
  }, [cwd]);

  function act(id: string, run: () => Promise<Proposal>) {
    run()
      .then((proposal) => {
        setRows((old) => upsertProposal(old, proposal));
        setErrors((old) => ({ ...old, [id]: "" }));
      })
      .catch((error: unknown) =>
        setErrors((old) => ({
          ...old,
          [id]: error instanceof Error ? error.message : String(error),
        })),
      );
  }

  if (rows.length === 0) return null;
  return (
    <div className="orch-proposals">
      <span className="dialog-eyebrow">PROPOSALS</span>
      {rows.map((row) => (
        <div
          key={row.id}
          className={`orch-proposal${
            row.status === "revert_suggested" ? " revert" : ""
          }`}
        >
          <div className="orch-proposal-head">
            <span className="orch-proposal-track">{row.track}</span>
            <span className="orch-proposal-form">{row.form}</span>
            <span className={`orch-proposal-status status-${row.status}`}>
              {row.status.replace("_", " ")}
            </span>
          </div>
          <div className="orch-proposal-change">{row.change}</div>
          <div className="orch-proposal-meta">{row.evidence}</div>
          <div className="orch-proposal-meta">Metric: {row.metric}</div>
          {row.reason && (
            <div className="orch-proposal-meta">Reason: {row.reason}</div>
          )}
          {row.track === "harness" && row.evalCommand && (
            <code className="orch-proposal-command">{row.evalCommand}</code>
          )}
          <div className="orch-proposal-actions">
            {row.track === "repo" && row.status === "proposed" && (
              <>
                <button
                  className="orch-proposal-button"
                  onClick={() =>
                    act(row.id, () =>
                      orchestratorClient.evolutionApprove(row.id),
                    )
                  }
                >
                  Approve
                </button>
                <button
                  className="orch-proposal-button"
                  onClick={() =>
                    act(row.id, () =>
                      orchestratorClient.evolutionReject(row.id),
                    )
                  }
                >
                  Reject
                </button>
              </>
            )}
            {row.track === "harness" &&
              row.status !== "adopted" &&
              row.status !== "rejected" && (
                <button
                  className="orch-proposal-button"
                  onClick={() =>
                    act(row.id, () => orchestratorClient.evolutionAdopt(row.id))
                  }
                >
                  Mark adopted
                </button>
              )}
          </div>
          {errors[row.id] && (
            <div className="orch-proposal-error" role="alert">
              {errors[row.id]}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

/** The owner's standing notes for this repo, shown to the planner. Always
 * rendered, because it is where the first note is added. A note saved from an
 * approved proposal is marked. `refresh` changes whenever the task list does. */
function RepoNotes({ cwd, refresh }: { cwd: string; refresh: string }) {
  const [notes, setNotes] = useState<Note[]>([]);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .repoNotesList(cwd)
      .then((loaded) => {
        if (cancelled) return;
        setNotes(loaded);
        setError("");
      })
      .catch((caught: unknown) => {
        if (cancelled) return;
        setError(caught instanceof Error ? caught.message : String(caught));
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh]);

  function fail(caught: unknown) {
    setError(caught instanceof Error ? caught.message : String(caught));
  }

  function add() {
    const text = draft.trim();
    if (!text) return;
    orchestratorClient
      .repoNotesAdd(cwd, text)
      .then((note) => {
        setNotes((old) => [...old, note]);
        setDraft("");
        setError("");
      })
      .catch(fail);
  }

  function remove(id: string) {
    orchestratorClient
      .repoNotesRemove(cwd, id)
      .then(() => {
        setNotes((old) => old.filter((note) => note.id !== id));
        setError("");
      })
      .catch(fail);
  }

  return (
    <div className="orch-notes">
      <span className="dialog-eyebrow">REPO NOTES</span>
      {notes.map((note) => (
        <div key={note.id} className="orch-note">
          <span className="orch-note-text">{note.text}</span>
          {note.source.startsWith("proposal:") && (
            <span className="orch-note-source">from proposal</span>
          )}
          <button
            className="orch-proposal-button"
            aria-label="Remove note"
            onClick={() => remove(note.id)}
          >
            Remove
          </button>
        </div>
      ))}
      <form
        className="orch-note-form"
        onSubmit={(event) => {
          event.preventDefault();
          add();
        }}
      >
        <input
          className="orch-note-input"
          aria-label="New repo note"
          placeholder="Standing guidance for the planner"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
        <button
          className="orch-proposal-button"
          type="submit"
          disabled={!draft.trim()}
        >
          Add
        </button>
      </form>
      {error && (
        <div className="orch-proposal-error" role="alert">
          {error}
        </div>
      )}
    </div>
  );
}

/** The Improvements view: proposals, recurring failures and repo notes. Lane
 * L3 builds the rest of the Figma frame. `refresh` changes whenever the task
 * list does. */
export function ImprovementsView({
  cwd,
  refresh,
  onOpenTask,
}: {
  cwd: string;
  refresh: string;
  onOpenTask: (id: string) => void;
}) {
  return (
    <div className="orch-view-scroll">
      <h2 className="orch-view-title">Improvements</h2>
      <EvolutionProposals cwd={cwd} refresh={refresh} />
      <RecurringFailures cwd={cwd} refresh={refresh} onOpenTask={onOpenTask} />
      <RepoNotes cwd={cwd} refresh={refresh} />
    </div>
  );
}
