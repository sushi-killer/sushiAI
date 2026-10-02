import { Check, GitBranch, Monitor, Play, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Icon } from "../../PanelIcon.tsx";
import { Tag } from "../../orchestrator/ui/index.ts";
import { orchestratorClientFor } from "../../orchestrator/client.ts";
import type { Task } from "../../orchestrator/types.ts";
import { worktreeBranchError } from "../../workspace/worktree.ts";
import {
  branchFor,
  nextBranch,
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
  takenBranch,
  onClose,
  onStart,
}: {
  path: string;
  body: string;
  meta: Record<string, string>;
  cwd: string;
  connection?: string;
  orchestratorOn: boolean;
  /** The branch an ended start used; a restart takes the next free name. */
  takenBranch?: string;
  onClose(): void;
  onStart(runner: Runner, branch: string, base: string): void;
}) {
  const sections = planSections(body);
  const [runner, setRunner] = useState<Runner>("claude");
  const [branch, setBranch] = useState(() =>
    takenBranch
      ? nextBranch(takenBranch)
      : branchFor(meta.title || sections.title || path),
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

const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
};

/** The plan's run, shown inline in the tag row: starting, running (Open goes
 * to it), ended or failed (Restart and Retry open the Start task card). */
export function RunStatus({
  record,
  ended,
  starting,
  status,
  failed,
  onOpen,
  onGo,
}: {
  record: StartRecord | undefined;
  /** The agent session the plan started has ended. */
  ended: boolean;
  starting: boolean;
  status: string;
  /** A start that left no record (the agent did not launch). */
  failed: boolean;
  /** Opens the Start task card. */
  onOpen(): void;
  /** Goes to the agent session that runs the plan. */
  onGo?(branch: string): void;
}) {
  const row = (
    tone: string,
    text: string,
    action?: { label: string; run(): void },
  ) => (
    <span className={`pv-run ${tone}`} role="status">
      <span className="pv-run-dot" aria-hidden="true" />
      <span className="pv-run-text" title={text}>
        {text}
      </span>
      {action && (
        <button type="button" className="pv-run-action" onClick={action.run}>
          {action.label}
        </button>
      )}
    </span>
  );
  if (starting) return row("", "Starting…");
  if (failed)
    return row("danger", "The start failed.", { label: "Retry", run: onOpen });
  if (record?.kind === "agent") {
    const agent = AGENT_NAMES[record.agent] || record.agent;
    if (ended)
      return row("", `Ended · ${record.branch}`, {
        label: "Restart",
        run: onOpen,
      });
    return row(
      "ok",
      `${agent} · ${record.branch}`,
      onGo && { label: "Open", run: () => onGo(record.branch) },
    );
  }
  if (record?.kind === "orchestrator") {
    const label = `Task #${record.taskId} · ${status ? taskStatusLabel(status) : "starting"}`;
    if (status === "failed")
      return row("danger", label, { label: "Retry", run: onOpen });
    return row(status === "waiting" ? "warning" : "ok", label);
  }
  return null;
}
