"use strict";

/**
 * Maps an orchd task notice (`orchestratorNotice` in orchestrator.cjs) to the
 * generic notice the Notices API shows, and runs the actions of that notice
 * against the orchestrator service. All task wording and task rules live here.
 */

const MAX_ANSWER_CHARS = 2000;
const MAX_TRACKED = 200;
const SOURCE_ID = "builtin.orchestrator";
// orchd task ids are v4 UUIDs (orchd/src/engine/mod.rs validate_task_id).
const TASK_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const RERUNNABLE = new Set(["failed", "stopped"]);
const LABELS = {
  input: "Needs you",
  done: "Done",
  failed: "Failed",
  stopped: "Stopped",
  landing: "Landing",
};
const KINDS = {
  input: "input",
  done: "done",
  failed: "failed",
  stopped: "failed",
  landing: "info",
};

const hostPrefix = (notice) => (notice.host ? `${notice.host}/` : "");

/** The notice key: a task's open question is one entry; any other notice is
 * one entry per kind, task and body. */
function noticeKey(notice) {
  const at = hostPrefix(notice);
  if (notice.kind === "input") return `input:${at}${notice.taskId}`;
  return `${notice.kind}:${at}${notice.taskId}:${notice.body}`;
}

/** "$0.31", "review PASS", "not landed": the done notice's second line. Null
 * when the task carries no cost. */
function doneMeta(notice) {
  if (typeof notice.costUsd !== "number") return null;
  return [
    `$${notice.costUsd.toFixed(2)}`,
    notice.verdict ? `review ${notice.verdict}` : "",
    notice.landed ? "landed" : "not landed",
  ].filter(Boolean);
}

function actionsOf(notice) {
  const open = (label, emphasis) => ({
    id: "open",
    label,
    ...(emphasis ? { emphasis } : {}),
  });
  switch (notice.kind) {
    case "input":
      return [open("Open task")];
    case "done":
      return [
        ...(notice.canLand
          ? [{ id: "land", label: "Land", emphasis: "primary", icon: "check" }]
          : []),
        open("View diff", "ghost"),
      ];
    case "failed":
    case "stopped":
      return [
        { id: "rerun", label: "Run again", icon: "refresh" },
        open("Open", "ghost"),
      ];
    default:
      return [open("Open")];
  }
}

/** The generic notice for a task notice. */
function toNotice(notice) {
  const out = {
    key: noticeKey(notice),
    kind: KINDS[notice.kind] ?? "info",
    label: LABELS[notice.kind] ?? "Notice",
    title: notice.title,
    body: notice.body,
    actions: actionsOf(notice),
  };
  const repo =
    notice.repoName ||
    String(notice.repo ?? "")
      .split(/[\\/]+/)
      .filter(Boolean)
      .pop();
  if (repo) out.header = repo;
  if (notice.at > 0) out.at = notice.at;
  if (notice.kind === "input") {
    out.reply = true;
    if (notice.options?.length) out.choices = notice.options;
    if (notice.askedBy) out.meta = [notice.title, `asked by ${notice.askedBy}`];
  }
  if (notice.kind === "done") {
    const meta = doneMeta(notice);
    if (meta) out.meta = meta;
  }
  return out;
}

/** Checks a quick answer before it reaches the daemon and returns the text to
 * send. The task must be waiting with a question and belong to a tracked,
 * still-open needs-input notice; the text is trimmed, non-empty and capped. */
function validateAnswer(taskId, text, task, queue = []) {
  if (typeof taskId !== "string" || !taskId) throw new Error("Invalid task.");
  if (!queue.some((item) => item.kind === "input" && item.taskId === taskId))
    throw new Error("No open question for that task.");
  if (
    !task ||
    task.id !== taskId ||
    task.status !== "waiting" ||
    !task.question
  )
    throw new Error("That task is not waiting for an answer.");
  if (typeof text !== "string") throw new Error("Invalid answer.");
  const answer = text.trim();
  if (!answer) throw new Error("Type an answer first.");
  if (answer.length > MAX_ANSWER_CHARS)
    throw new Error("That answer is too long.");
  return answer;
}

/** The failed/stopped notice a Run again click may restart, or null. Only a
 * UUID-shaped task id with such a notice still tracked is accepted. */
function rerunNotice(taskId, queue) {
  if (typeof taskId !== "string" || !TASK_ID.test(taskId)) return null;
  return (
    queue.find((item) => RERUNNABLE.has(item.kind) && item.taskId === taskId) ??
    null
  );
}

/**
 * @param {{
 *   notices: { register(id: string, onAction: Function): void, publish(id: string, notice: object): unknown, retract(id: string, key: string): void },
 *   getService: () => ({ call(method: string, params: object, host?: string): Promise<any> }) | null | undefined,
 *   showWindow: () => void,
 *   send: (channel: string, value: object) => void,
 * }} deps
 */
function createOrchestratorNotices({ notices, getService, showWindow, send }) {
  /** Task notices that are still on offer, by their notice key. */
  const tracked = new Map();

  const service = () => {
    const found = getService();
    if (!found) throw new Error("The orchestrator is not running.");
    return found;
  };
  const gone = () => new Error("That notice is gone.");

  async function onAction(key, actionId, text) {
    const notice = tracked.get(key);
    if (!notice) throw gone();
    switch (actionId) {
      case "reply":
      case "answer": {
        // A remote task is answered on the host it runs on.
        const task = await service().call(
          "task.get",
          { id: notice.taskId },
          notice.host,
        );
        const answer = validateAnswer(notice.taskId, text, task, [notice]);
        await service().call(
          "task.answer",
          { id: notice.taskId, answer },
          notice.host,
        );
        tracked.delete(key);
        return "Answered — the task carries on.";
      }
      case "land":
        if (notice.kind !== "done" || !notice.canLand) throw gone();
        await service().call("task.land", { id: notice.taskId }, notice.host);
        tracked.delete(key);
        return undefined;
      case "rerun": {
        const found = rerunNotice(notice.taskId, [notice]);
        if (!found) throw gone();
        await service().call("task.start", { id: found.taskId }, found.host);
        tracked.delete(key);
        return undefined;
      }
      case "open":
        showWindow();
        send("orchestrator-open", {
          taskId: notice.taskId,
          repo: notice.repo,
          focus: notice.focus,
          ...(notice.host ? { host: notice.host } : {}),
        });
        return undefined;
      default:
        throw new Error("Unknown action.");
    }
  }

  notices.register(SOURCE_ID, onAction);

  return {
    SOURCE_ID,
    /** Show a task notice (needs input, done, failed, ...). */
    publish(notice) {
      const key = noticeKey(notice);
      tracked.delete(key);
      tracked.set(key, notice);
      if (tracked.size > MAX_TRACKED)
        tracked.delete(tracked.keys().next().value);
      try {
        notices.publish(SOURCE_ID, toNotice(notice));
      } catch (error) {
        tracked.delete(key);
        console.error(`[notices] ${error.message}`);
      }
    },
    /** A task moved: its open question is withdrawn once it is not waiting. */
    onTask(task) {
      if (task?.status === "waiting") return;
      for (const [key, notice] of [...tracked])
        if (notice.kind === "input" && notice.taskId === task?.id) {
          tracked.delete(key);
          notices.retract(SOURCE_ID, key);
        }
    },
  };
}

module.exports = {
  SOURCE_ID,
  MAX_ANSWER_CHARS,
  TASK_ID,
  RERUNNABLE,
  noticeKey,
  toNotice,
  validateAnswer,
  rerunNotice,
  createOrchestratorNotices,
};
