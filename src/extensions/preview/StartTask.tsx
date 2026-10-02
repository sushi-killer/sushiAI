import { Check, GitBranch, Monitor, Play, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Icon } from "../../PanelIcon.tsx";
import { Tag } from "../../orchestrator/ui/index.ts";
import { orchestratorClientFor } from "../../orchestrator/client.ts";
import type { Task } from "../../orchestrator/types.ts";
import { worktreeBranchError } from "../../workspace/worktree.ts";
import {
  branchFor,
  goalPrompt,
  isVerified,
  planSections,
  taskStatusLabel,
  type StartRecord,
} from "./artifact.ts";

export type Runner = "claude" | "codex" | "orchestrator";

const RUNNERS: { id: Runner; label: string; hint: string }[] = [
  { id: "claude", label: "Claude Code", hint: "/goal" },
  { id: "codex", label: "Codex", hint: "/goal" },
  { id: "orchestrator", label: "Orchestrator", hint: "plan → review → land" },
];

export const hostOfEndpoint = (connection?: string) =>
  connection?.startsWith("ssh:") ? connection : "local";

export function VerifiedTag({ meta }: { meta: Record<string, string> }) {
  return isVerified(meta) ? (
    <Tag tone="ok">Verified</Tag>
  ) : (
    <Tag tone="warning">Not verified</Tag>
  );
}

/** The window over the plan: what will run, who runs it, where, and
 * the exact first line of the prompt. */
export function StartTaskCard({
  path,
  body,
  meta,
  cwd,
  connection,
  orchestratorOn,
  onClose,
  onStart,
}: {
  path: string;
  body: string;
  meta: Record<string, string>;
  cwd: string;
  connection?: string;
  orchestratorOn: boolean;
  onClose(): void;
  onStart(runner: Runner, branch: string, base: string): void;
}) {
  const sections = planSections(body);
  const [runner, setRunner] = useState<Runner>("claude");
  const [branch, setBranch] = useState(() =>
    branchFor(meta.title || sections.title || path),
  );
  const [base, setBase] = useState("main");
  useEffect(() => {
    let disposed = false;
    window.bridge
      ?.projectInspect(connection, { operation: "git", root: cwd })
      .then((git) => {
        if (!disposed && git?.branch) setBase(String(git.branch));
      })
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, [connection, cwd]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const agent = runner !== "orchestrator";
  const error = agent ? worktreeBranchError(branch) : "";
  const first = (agent ? goalPrompt(body, path) : body.trim() || sections.title)
    .split("\n")[0]
    .slice(0, 240);
  const runners = RUNNERS.filter(
    (item) => orchestratorOn || item.id !== "orchestrator",
  );
  const host = connection?.startsWith("ssh:") ? connection.slice(4) : "Local";
  return (
    <div
      className="pv-start"
      role="dialog"
      aria-label="Start task from this plan"
    >
      <div className="pv-start-head">
        <Play size={14} aria-hidden />
        <strong>Start task from this plan</strong>
        <VerifiedTag meta={meta} />
        <button
          type="button"
          className="ui-button icon"
          aria-label="Close"
          onClick={onClose}
        >
          <X size={14} aria-hidden />
        </button>
      </div>
      {!isVerified(meta) && (
        <p className="pv-start-note">
          The agent has not checked this plan for contradictions.
        </p>
      )}
      {sections.goal && (
        <div className="pv-start-block">
          <span className="pv-eyebrow">Goal</span>
          <p>{sections.goal}</p>
        </div>
      )}
      {sections.doneWhen.length > 0 && (
        <div className="pv-start-block">
          <span className="pv-eyebrow">
            Done when · {sections.doneWhen.length}
          </span>
          <ul>
            {sections.doneWhen.map((item) => (
              <li key={item}>
                <Check size={11} aria-hidden />
                <span>
                  {item
                    .split("`")
                    .map((part, index) =>
                      index % 2 ? <code key={index}>{part}</code> : part,
                    )}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="pv-start-block pv-start-block-who">
        <span className="pv-who">Who runs it</span>
        <div className="pv-tiles" role="radiogroup" aria-label="Who runs it">
          {runners.map((item) => (
            <button
              key={item.id}
              type="button"
              role="radio"
              aria-checked={runner === item.id}
              className="pv-tile"
              onClick={() => setRunner(item.id)}
            >
              <span className="pv-tile-name">
                <Icon
                  kind={item.id === "orchestrator" ? "orchestrator" : "agent"}
                  agent={item.id === "orchestrator" ? undefined : item.id}
                />
                {item.label}
              </span>
              <small>{item.hint}</small>
            </button>
          ))}
        </div>
      </div>
      <div className="pv-where">
        <Monitor size={13} aria-hidden />
        <span>{host}</span>
        {agent && (
          <>
            <span className="pv-where-sep">·</span>
            <GitBranch size={13} aria-hidden />
            <span>new worktree</span>
            <input
              className="pv-branch"
              aria-label="Worktree branch"
              aria-invalid={!!error}
              value={branch}
              spellCheck={false}
              onChange={(event) => setBranch(event.target.value)}
            />
            <span>from {base}</span>
          </>
        )}
      </div>
      {error && (
        <div className="pd-alert" role="alert">
          {error}
        </div>
      )}
      <code className="pv-first">{first}</code>
      <div className="pv-start-foot">
        <span>
          {agent
            ? `Opens a new ${runner === "claude" ? "Claude Code" : "Codex"} pane in the worktree`
            : "Creates a task; the orchestrator plans, reviews and lands it"}
        </span>
        <button type="button" className="ui-button secondary" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="ui-button primary"
          disabled={!!error}
          onClick={() => onStart(runner, branch, base)}
        >
          <Play size={12} aria-hidden />
          Start
        </button>
      </div>
    </div>
  );
}

/** A task in one of these states no longer changes, so polling stops. */
const FINAL = new Set(["done", "failed"]);

/** The latest status of the orchestrator task a plan started, polled every
 * 5 s until it is done, failed or archived. Empty until the first answer. */
export function useTaskStatus(record: StartRecord | undefined): string {
  const [status, setStatus] = useState("");
  const taskId = record?.kind === "orchestrator" ? record.taskId : "";
  const host = record?.kind === "orchestrator" ? record.host : "local";
  useEffect(() => {
    setStatus("");
    if (!taskId) return;
    let disposed = false;
    const poll = () =>
      orchestratorClientFor(host)
        .taskGet(taskId)
        .then((task: Task) => {
          if (disposed) return;
          setStatus(task.status);
          if (FINAL.has(task.status) || task.archived)
            window.clearInterval(timer);
        })
        .catch(() => {});
    const timer = window.setInterval(poll, 5000);
    void poll();
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [taskId, host]);
  return status;
}

/** The header control and its states: the Start task icon, then a compact
 * chip for Starting…, the running task or session, Failed · Retry. */
export function StartButton({
  record,
  starting,
  status,
  failed,
  onOpen,
}: {
  record: StartRecord | undefined;
  starting: boolean;
  /** A start that left no record (the agent did not launch). */
  failed: boolean;
  status: string;
  onOpen(): void;
}) {
  if (starting)
    return (
      <button type="button" className="pv-start-state pv-start-btn" disabled>
        Starting…
      </button>
    );
  if (record?.kind === "agent")
    return (
      <span className="pv-start-state" title={`Running in ${record.branch}`}>
        Running in {record.branch}
      </span>
    );
  if (record?.kind === "orchestrator") {
    if (status === "failed")
      return (
        <button
          type="button"
          className="pv-start-state pv-start-btn failed"
          title={`Task #${record.taskId} failed`}
          onClick={onOpen}
        >
          Failed · Retry
        </button>
      );
    return (
      <span
        className={`pv-start-state ${status === "waiting" ? "waiting" : ""}`}
        title={`Task #${record.taskId}`}
      >
        #{record.taskId} · {status ? taskStatusLabel(status) : "starting"}
      </span>
    );
  }
  if (failed)
    return (
      <button
        type="button"
        className="pv-start-state pv-start-btn failed"
        title="Starting the task failed"
        onClick={onOpen}
      >
        Failed · Retry
      </button>
    );
  return (
    <button
      type="button"
      className="pv-icon pv-start-icon pv-start-btn"
      aria-label="Start task"
      title="Start task"
      onClick={onOpen}
    >
      <Play aria-hidden />
    </button>
  );
}
