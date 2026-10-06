import { useMemo } from "react";
import type { AttentionItem } from "../extensions/modules.ts";
import { groupKey } from "../app/workspaceMerge.ts";
import { orchestratorClientFor } from "./client.ts";
import { formatCost } from "./helpers.ts";
import { hostOf } from "./hosts.ts";
import {
  enterAnswer,
  landTasks,
  ownerKind,
  ownerTasks,
  projectName,
  type AnswerChoice,
} from "./ownerAttention.ts";
import type { TaskTarget } from "./notices.ts";
import type { Task } from "./types.ts";
import { currentTasks, useTasks } from "./useTasks.ts";

const searchText = (task: Task) =>
  `${task.title} ${task.repo ?? ""}`.toLowerCase();

function item(
  kind: AttentionItem["kind"],
  task: Task,
  prefix: string,
): AttentionItem {
  return {
    key: `${prefix}:${task.id}`,
    kind,
    title: task.title,
    project: projectName(task.repo),
    host: groupKey(task.host),
    at: task.updatedAt,
    search: searchText(task),
  };
}

/** The Inbox rows for a task list: what waits on an answer, what needs a
 * decision, and finished work that is not landed yet. None while off. */
export function attentionItems(
  tasks: Task[],
  enabled: boolean,
): AttentionItem[] {
  if (!enabled) return [];
  return [
    ...ownerTasks(tasks).map((task) => item(ownerKind(task), task, "task")),
    ...landTasks(tasks).map((task) => item("review", task, "land")),
  ];
}

/** The orchestrator's Inbox rows, kept live from the shared task list. */
export function useOrchestratorAttention(enabled: boolean): AttentionItem[] {
  const tasks = useTasks(enabled);
  return useMemo(() => attentionItems(tasks, enabled), [tasks, enabled]);
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
 * selection moves (see `enterAnswer`), except after a digit or click pick
 * made while this question was shown: that pick is as deliberate as a click
 * on Answer, so the Enter right after it sends it. */
export function inboxEnterAnswer(
  choice: AnswerChoice,
  selectedAt: number,
  pickedAt: number | undefined,
  now: number,
): string {
  const picked = pickedAt != null && pickedAt >= selectedAt;
  return enterAnswer(choice, picked ? -Infinity : selectedAt, now);
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
