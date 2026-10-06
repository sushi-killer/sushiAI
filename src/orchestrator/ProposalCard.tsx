import { useState } from "react";
import { ArrowRight } from "lucide-react";
import { useOrchestratorClient } from "./hostContext";
import {
  createLabel,
  initialChecks,
  modeLabel,
  proposalMeta,
  proposalRequest,
  proposalSelection,
  rowState,
} from "./chatModel";
import { errorText } from "./helpers";
import type { ChatMessage, ChatProposal, Task } from "./types";
import { Tag } from "../ui";

/** Orch/ProposalCard: the tasks a Brainstorm or Plan reply proposed, one row
 * each. The owner ticks the rows to keep, then creates them now or parks
 * them in the plan. What became a task, and what was passed on, is kept by
 * orchd on the message, so the card reads the same after a reload. */
export function ProposalCard({
  cwd,
  message,
  proposal,
  tasks,
  disabled,
  onOpenTask,
}: {
  cwd: string;
  message: ChatMessage;
  proposal: ChatProposal;
  tasks: Task[];
  /** A reply is streaming: orchd refuses to create tasks meanwhile. */
  disabled: boolean;
  onOpenTask?: (id: string) => void;
}) {
  const client = useOrchestratorClient();
  const [checked, setChecked] = useState(() => initialChecks(proposal));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const { create } = proposalSelection(proposal, checked);
  const origin = modeLabel(message.mode).toLowerCase();

  async function run(row?: number, backlog = false) {
    const request = proposalRequest(proposal, checked, { row, backlog });
    if (!request || busy || disabled) return;
    setBusy(true);
    setError("");
    try {
      await client.chatCreateProposal(cwd, message.id, request);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }
  const off = busy || disabled;
  return (
    <section className="ochat-proposal" aria-label="Proposed tasks">
      <header className="ochat-proposal-head">
        <span className="ochat-proposal-title">Proposed tasks</span>
        <span className="ui-count">{proposal.tasks.length}</span>
        {origin && (
          <span className="ochat-proposal-from">from this {origin}</span>
        )}
      </header>
      <ul className="ochat-proposal-rows">
        {proposal.tasks.map((task, i) => {
          const state = rowState(task, checked[i]);
          const meta = proposalMeta(task, proposal, tasks);
          const made = task.taskId;
          return (
            <li key={i} className={`ochat-proposal-row ${state}`}>
              <input
                type="checkbox"
                aria-label={`Include ${task.title}`}
                checked={made ? true : checked[i]}
                disabled={Boolean(made) || off}
                onChange={(event) =>
                  setChecked((old) =>
                    old.map((v, j) => (j === i ? event.target.checked : v)),
                  )
                }
              />
              <span className="ochat-proposal-body">
                <span className="ochat-proposal-name" title={task.title}>
                  {task.title}
                </span>
                <span className="ochat-proposal-meta">{meta}</span>
              </span>
              {made ? (
                <>
                  <Tag tone="info" dot={false}>
                    queued
                  </Tag>
                  {onOpenTask && (
                    <button
                      type="button"
                      className="icon-button"
                      aria-label={`Open ${task.title}`}
                      onClick={() => onOpenTask(made)}
                    >
                      <ArrowRight size={12} />
                    </button>
                  )}
                </>
              ) : state === "skipped" ? (
                <span className="ochat-proposal-skipped">skipped</span>
              ) : (
                <button
                  type="button"
                  className="ui-button secondary"
                  aria-label={`Create ${task.title}`}
                  disabled={off}
                  onClick={() => void run(i + 1)}
                >
                  Create
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {error && (
        <p className="ochat-error" role="alert">
          {error}
        </p>
      )}
      <footer className="ochat-proposal-foot">
        <button
          type="button"
          className="ui-button primary"
          disabled={off || create.length === 0}
          onClick={() => void run()}
        >
          {createLabel(create.length)}
        </button>
        <button
          type="button"
          className="ui-button secondary"
          disabled={off || create.length === 0}
          onClick={() => void run(undefined, true)}
        >
          Add to plan
        </button>
        <span className="ochat-proposal-hint">Reply to change the list</span>
      </footer>
    </section>
  );
}
