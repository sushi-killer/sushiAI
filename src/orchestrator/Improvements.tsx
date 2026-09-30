import { useEffect, useRef, useState } from "react";
import { Check, X } from "lucide-react";
import { useOrchestratorClient, useOrchestratorHost } from "./hostContext";
import { hostOf } from "./hosts";
import { errorText } from "./helpers";
import {
  failureNoteDraft,
  failureTasksLine,
  failureTitle,
  failuresHint,
  openProposalCount,
  RECENT_DAYS,
  recurringFailures,
  recurringTotal,
  runSummary,
  sectionLabel,
  seenIn,
  upsertProposal,
} from "./improvementsModel";
import type { FailureRow, Note, Proposal } from "./types";
import { Tag } from "./ui";
import "./improvements.css";

/** The daemon's evolution proposals for this repo: what to change, the
 * evidence and the metric that will judge it. A repo-track one is approved
 * (it becomes a task) or rejected; a harness-track one carries the A/B eval
 * command and is marked adopted by hand. */
function Proposals({ cwd, refresh }: { cwd: string; refresh: string }) {
  const orchestratorClient = useOrchestratorClient();
  const host = useOrchestratorHost();
  const [rows, setRows] = useState<Proposal[]>([]);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [looking, setLooking] = useState(false);
  const [lookNote, setLookNote] = useState("");
  const [lookError, setLookError] = useState("");

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .evolutionList(cwd)
      .then((loaded) => !cancelled && setRows(loaded))
      .catch(() => !cancelled && setRows([]));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh, orchestratorClient]);

  useEffect(() => {
    const off = window.bridge?.onOrchestrator((event) => {
      if (hostOf(event) !== host) return;
      if (event.event === "proposal" && event.proposal.repo === cwd)
        setRows((old) => upsertProposal(old, event.proposal));
    });
    return () => off?.();
  }, [cwd, host]);

  function act(id: string, run: () => Promise<Proposal>) {
    run()
      .then((proposal) => {
        setRows((old) => upsertProposal(old, proposal));
        setErrors((old) => ({ ...old, [id]: "" }));
      })
      .catch((error: unknown) =>
        setErrors((old) => ({ ...old, [id]: errorText(error) })),
      );
  }

  function lookForImprovements() {
    setLooking(true);
    setLookNote("");
    setLookError("");
    orchestratorClient
      .evolutionRun()
      .then((result) => setLookNote(runSummary(result)))
      .catch((error: unknown) => setLookError(errorText(error)))
      .finally(() => setLooking(false));
  }

  return (
    <>
      <span className="orch-eyebrow">
        {sectionLabel("PROPOSALS", openProposalCount(rows))}
      </span>
      <div className="imp-run">
        <button
          type="button"
          className="ui-button secondary"
          disabled={looking}
          onClick={lookForImprovements}
        >
          {looking ? "Looking…" : "Look for improvements"}
        </button>
        {lookNote && <span className="imp-faint">{lookNote}</span>}
        {!lookNote && !lookError && rows.length === 0 && (
          <span className="imp-faint">No proposals yet.</span>
        )}
      </div>
      {lookError && (
        <div className="imp-error" role="alert">
          {lookError}
        </div>
      )}
      {rows.map((row) => {
        const open = row.status === "proposed";
        const seen = seenIn(row);
        return (
          <div
            key={row.id}
            className={`imp-card${row.status === "revert_suggested" ? " revert" : ""}`}
          >
            <div className="imp-card-head">
              <Tag tone={row.track === "repo" ? "info" : "neutral"}>
                {row.track}
              </Tag>
              <span className="imp-card-title">{row.change}</span>
              {!open && (
                <Tag
                  tone={
                    row.status === "adopted" || row.status === "approved"
                      ? "ok"
                      : row.status === "revert_suggested"
                        ? "warning"
                        : "neutral"
                  }
                >
                  {row.status.replace("_", " ")}
                </Tag>
              )}
            </div>
            <div className="imp-line">
              <span className="imp-faint">Metric</span>
              <span className="imp-metric">{row.metric}</span>
            </div>
            <div className="imp-reason">Reason: {row.evidence}</div>
            {row.reason && <div className="imp-reason">{row.reason}</div>}
            {row.track === "harness" && row.evalCommand && (
              <code className="imp-command">
                <span className="imp-faint">$</span>
                <span>{row.evalCommand}</span>
              </code>
            )}
            <div className="imp-actions">
              {row.track === "repo" && open && (
                <>
                  <button
                    type="button"
                    className="ui-button primary"
                    onClick={() =>
                      act(row.id, () =>
                        orchestratorClient.evolutionApprove(row.id),
                      )
                    }
                  >
                    Approve
                  </button>
                  <button
                    type="button"
                    className="ui-button ghost"
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
                  <>
                    <button
                      type="button"
                      className="ui-button secondary"
                      onClick={() =>
                        act(row.id, () =>
                          orchestratorClient.evolutionAdopt(row.id),
                        )
                      }
                    >
                      <Check size={14} />
                      Mark adopted
                    </button>
                    <span className="imp-faint">needs a harness change</span>
                  </>
                )}
              {seen && <span className="imp-faint">{seen}</span>}
            </div>
            {errors[row.id] && (
              <div className="imp-error" role="alert">
                {errors[row.id]}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}

/** The failures that keep coming back across this repo's tasks. */
function RecurringFailures({
  cwd,
  refresh,
  onOpenTask,
  onAddNote,
}: {
  cwd: string;
  refresh: string;
  onOpenTask: (id: string) => void;
  onAddNote: (draft: string) => void;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [rows, setRows] = useState<FailureRow[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    orchestratorClient
      .failuresCatalogue(cwd, RECENT_DAYS)
      .then((loaded) => !cancelled && setRows(loaded))
      .catch(() => !cancelled && setRows([]));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh, orchestratorClient]);

  const top = recurringFailures(rows);
  if (top.length === 0) return null;
  return (
    <>
      <span className="orch-eyebrow">
        {sectionLabel(
          "RECURRING FAILURES",
          recurringTotal(rows),
          failuresHint(recurringTotal(rows)),
        )}
      </span>
      {top.map((row) => {
        const open = expanded === row.signature;
        return (
          <div key={row.signature} className="imp-failure">
            <span className="imp-failure-count">{row.count}×</span>
            <div className="imp-failure-body">
              <span className="imp-failure-title" title={row.exampleDetail}>
                {failureTitle(row)}
              </span>
              <span className="imp-failure-tasks">{failureTasksLine(row)}</span>
            </div>
            <button
              type="button"
              className="ui-button ghost"
              aria-expanded={open}
              onClick={() => setExpanded(open ? null : row.signature)}
            >
              Open tasks
            </button>
            <button
              type="button"
              className="ui-button secondary"
              onClick={() => onAddNote(failureNoteDraft(row))}
            >
              Add a note
            </button>
            {open && (
              <div className="imp-failure-list">
                {row.tasks.map((task) => (
                  <button
                    key={task.id}
                    type="button"
                    className="imp-failure-task"
                    onClick={() => onOpenTask(task.id)}
                  >
                    {task.title}
                  </button>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}

/** The owner's standing notes for this repo, shown to the planner. Always
 * rendered, because it is where the first note is added. */
function RepoNotes({
  cwd,
  refresh,
  draft,
  onDraft,
  inputRef,
}: {
  cwd: string;
  refresh: string;
  draft: string;
  onDraft: (text: string) => void;
  inputRef: React.RefObject<HTMLInputElement | null>;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [notes, setNotes] = useState<Note[]>([]);
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
      .catch((caught: unknown) => !cancelled && setError(errorText(caught)));
    return () => {
      cancelled = true;
    };
  }, [cwd, refresh, orchestratorClient]);

  function add() {
    const text = draft.trim();
    if (!text) return;
    orchestratorClient
      .repoNotesAdd(cwd, text)
      .then((note) => {
        setNotes((old) => [...old, note]);
        onDraft("");
        setError("");
      })
      .catch((caught: unknown) => setError(errorText(caught)));
  }

  function remove(id: string) {
    orchestratorClient
      .repoNotesRemove(cwd, id)
      .then(() => {
        setNotes((old) => old.filter((note) => note.id !== id));
        setError("");
      })
      .catch((caught: unknown) => setError(errorText(caught)));
  }

  return (
    <>
      <span className="orch-eyebrow">
        {sectionLabel(
          "REPO NOTES",
          notes.length,
          "standing guidance for the planner",
        )}
      </span>
      <div className="imp-notes">
        {notes.map((note) => (
          <div key={note.id} className="imp-note">
            <span className="imp-note-text">{note.text}</span>
            {note.source.startsWith("proposal:") && (
              <Tag tone="info">from proposal</Tag>
            )}
            <button
              type="button"
              className="imp-note-remove"
              aria-label="Remove note"
              onClick={() => remove(note.id)}
            >
              <X size={12} />
            </button>
          </div>
        ))}
        <form
          className="imp-note-add"
          onSubmit={(event) => {
            event.preventDefault();
            add();
          }}
        >
          <input
            ref={inputRef}
            className="imp-note-input"
            aria-label="New repo note"
            placeholder="Standing guidance for the planner…"
            value={draft}
            onChange={(event) => onDraft(event.target.value)}
          />
          <button
            type="submit"
            className="ui-button secondary"
            disabled={!draft.trim()}
          >
            Add
          </button>
        </form>
      </div>
      {error && (
        <div className="imp-error" role="alert">
          {error}
        </div>
      )}
    </>
  );
}

/** The Improvements view: proposals, recurring failures and repo notes.
 * `refresh` changes whenever the task list does. */
export function ImprovementsView({
  cwd,
  refresh,
  onOpenTask,
}: {
  cwd: string;
  refresh: string;
  onOpenTask: (id: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const input = useRef<HTMLInputElement | null>(null);

  function addNote(text: string) {
    setDraft(text);
    input.current?.focus();
    input.current?.scrollIntoView({ block: "nearest" });
  }

  return (
    <div className="orch-view-scroll imp-view">
      <div className="imp-head">
        <h2 className="orch-view-title">Improvements</h2>
        <p className="imp-lede">
          Changes to how the orchestrator works — its own proposals, failures
          that keep repeating, and your standing notes.
        </p>
      </div>
      <Proposals cwd={cwd} refresh={refresh} />
      <RecurringFailures
        cwd={cwd}
        refresh={refresh}
        onOpenTask={onOpenTask}
        onAddNote={addNote}
      />
      <RepoNotes
        cwd={cwd}
        refresh={refresh}
        draft={draft}
        onDraft={setDraft}
        inputRef={input}
      />
    </div>
  );
}
