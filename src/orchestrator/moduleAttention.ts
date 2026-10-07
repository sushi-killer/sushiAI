import { useMemo, useSyncExternalStore } from "react";
import type { AttentionAction, AttentionItem } from "../extensions/modules.ts";
import { groupKey } from "../app/workspaceMerge.ts";
import { orchestratorClientFor } from "./client.ts";
import {
  formatCost,
  latestImplementAttempt,
  reviewOf,
  taskReason,
} from "./helpers.ts";
import {
  enterReply,
  shownPick,
  togglePick,
  type ReplyChoice,
} from "../lib/reply.ts";
import { hostOf } from "./hosts.ts";
import {
  landTasks,
  ownerKind,
  ownerTasks,
  projectName,
} from "./ownerAttention.ts";
import type { TaskTarget } from "./notices.ts";
import { cleanTitle } from "./taskTitle.ts";
import { ownerTarget } from "./ownerAttention.ts";
import type { Task } from "./types.ts";
import { currentTasks, useTasks } from "./useTasks.ts";

const searchText = (task: Task) =>
  `${task.title} ${task.repo ?? ""}`.toLowerCase();

/** The choice picked for each answer row, shared by the row chips and the
 * Inbox detail: a pick waits for Answer or Enter. */
let picks: Readonly<Record<string, string>> = {};
const pickListeners = new Set<() => void>();
export function setPick(key: string, option: string | undefined): void {
  const next = { ...picks };
  if (option === undefined) delete next[key];
  else next[key] = option;
  picks = next;
  for (const listener of [...pickListeners]) listener();
}
export function usePicks(): Readonly<Record<string, string>> {
  return useSyncExternalStore(
    (listener) => {
      pickListeners.add(listener);
      return () => void pickListeners.delete(listener);
    },
    () => picks,
  );
}

/** The row's second line: the question, what finished, or why it stopped. */
function rowMeta(
  kind: AttentionItem["kind"],
  task: Task,
  tasks: Task[],
): string {
  if (kind === "answer") return (task.question?.text ?? "").split("\n")[0];
  if (kind === "review") {
    const count = latestImplementAttempt(task)?.changedFiles.length;
    const review = reviewOf(task);
    return [
      review ? `review ${review.verdict}` : "done",
      count ? `${count} file${count === 1 ? "" : "s"}` : "",
      formatCost(task.costUsd),
      `not landed · → ${task.baseRef}`,
    ]
      .filter(Boolean)
      .join(" · ");
  }
  return `${taskReason(task, tasks)} · ${formatCost(task.costUsd)}`;
}

/** The row's chips. An answer choice picks (Answer or Enter in the detail
 * sends it); the keys are the Inbox's old L, R and E. */
function rowActions(
  kind: AttentionItem["kind"],
  task: Task,
  pick: string | undefined,
): AttentionAction[] {
  if (kind === "answer") {
    const options = task.question?.options ?? [];
    const shown = shownPick({ pick, preselected: options[0] ?? "", note: "" });
    return options.map((label, index) => ({
      id: `answer:${index}`,
      label,
      selected: shown === label,
    }));
  }
  if (kind === "decide")
    return [
      ...(task.status !== "landing"
        ? [{ id: "run", label: "Run again", key: "r" }]
        : []),
      { id: "note", label: "Run with a note" },
      { id: "archive", label: "Archive", key: "e" },
    ];
  return [
    { id: "land", label: "Land", key: "l", primary: true },
    { id: "fix", label: "Needed a fix" },
    { id: "clean", label: "Clean" },
  ];
}

function item(
  kind: AttentionItem["kind"],
  task: Task,
  prefix: string,
  tasks: Task[],
  picked: Readonly<Record<string, string>>,
): AttentionItem {
  return {
    key: `${prefix}:${task.id}`,
    kind,
    title: cleanTitle(task.title),
    project: projectName(task.repo),
    host: groupKey(task.host),
    at: task.updatedAt,
    search: searchText(task),
    meta: rowMeta(kind, task, tasks),
    actions: rowActions(kind, task, picked[`${prefix}:${task.id}`]),
  };
}

/** The Inbox rows for a task list: what waits on an answer, what needs a
 * decision, and finished work that is not landed yet. None while off. */
export function attentionItems(
  tasks: Task[],
  enabled: boolean,
  picked: Readonly<Record<string, string>> = {},
): AttentionItem[] {
  if (!enabled) return [];
  return [
    ...ownerTasks(tasks).map((task) =>
      item(ownerKind(task), task, "task", tasks, picked),
    ),
    ...landTasks(tasks).map((task) =>
      item("review", task, "land", tasks, picked),
    ),
  ];
}

/** The orchestrator's Inbox rows, kept live from the shared task list. */
export function useOrchestratorAttention(enabled: boolean): AttentionItem[] {
  const tasks = useTasks(enabled);
  const picked = usePicks();
  return useMemo(
    () => attentionItems(tasks, enabled, picked),
    [tasks, enabled, picked],
  );
}

/** Runs one row action through the daemon of the task's host. The row's
 * "Needed a fix" takes a note; without one it opens the task to write it. */
export async function actOnTask(
  key: string,
  actionId: string,
  text?: string,
): Promise<void> {
  const id = key.slice(key.indexOf(":") + 1);
  const task = currentTasks().find((t) => t.id === id);
  if (!task) return;
  const client = orchestratorClientFor(hostOf(task));
  const [verb, index] = actionId.split(":");
  switch (verb) {
    case "answer": {
      const options = task.question?.options ?? [];
      const option = options[Number(index)];
      if (option)
        setPick(
          key,
          togglePick(
            { pick: picks[key], preselected: options[0] ?? "", note: "" },
            option,
          ),
        );
      return;
    }
    case "run":
      if (task.status !== "landing") await client.taskStart(task.id);
      return;
    case "note":
      openTask(ownerTarget(task));
      return;
    case "archive":
      await client.taskArchive(task.id);
      return;
    case "land":
      await client.taskLand(task.id);
      return;
    case "fix":
      if (task.leadTouch?.touched === true) await client.taskLeadTouch(task.id);
      else if (text?.trim())
        await client.taskLeadTouch(task.id, true, text.trim());
      else openTask(ownerTarget(task));
      return;
    case "clean":
      await client.taskLeadTouch(
        task.id,
        task.leadTouch?.touched === false ? undefined : false,
      );
      return;
  }
}

/** "Land all": lands each review row on screen, one after the other, each
 * through the daemon of the host it runs on. */
export const reviewAllTasks = {
  label: "Land",
  async run(keys: string[]): Promise<void> {
    for (const key of keys) {
      const id = key.slice(key.indexOf(":") + 1);
      const task = currentTasks().find((t) => t.id === id);
      if (task) await orchestratorClientFor(hostOf(task)).taskLand(task.id);
    }
  },
};

/** What Enter sends in the Inbox. Enter stays unarmed for a moment after the
 * selection moves (see `enterReply`), except after a digit or click pick
 * made while this question was shown: that pick is as deliberate as a click
 * on Answer, so the Enter right after it sends it. */
export function inboxEnterAnswer(
  choice: ReplyChoice,
  selectedAt: number,
  pickedAt: number | undefined,
  now: number,
): string {
  const picked = pickedAt != null && pickedAt >= selectedAt;
  return enterReply(choice, picked ? -Infinity : selectedAt, now);
}

/** The detail's branch box after the branch and +/−: "3 files · attempt 1/4
 * · $0.04", each part only when there is something to say. */
export function diffFacts(
  files: number | undefined,
  attempts: number,
  maxAttempts: number | undefined,
  costUsd: number | undefined,
): string {
  return [
    files ? `${files} file${files === 1 ? "" : "s"}` : "",
    attempts > 0
      ? `attempt ${attempts}${maxAttempts ? `/${maxAttempts}` : ""}`
      : "",
    costUsd && costUsd > 0 ? formatCost(costUsd) : "",
  ]
    .filter(Boolean)
    .join(" · ");
}

/** Where "Open task" in the Inbox detail goes. The module's shell hook sets
 * it; until then opening does nothing. */
let opener: ((target: TaskTarget) => void) | null = null;
export function setTaskOpener(open: ((target: TaskTarget) => void) | null) {
  opener = open;
}
export function openTask(target: TaskTarget): void {
  opener?.(target);
}
