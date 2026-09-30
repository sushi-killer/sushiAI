// Pure model behind the Plan view: which drafts sit where, how the backlog
// reorders. No React, no I/O.
import { dependencyTitles, planDrafts } from "./helpers.ts";
import { needsOwner } from "./ownerAttention.ts";
import type { BacklogBucket, Task } from "./types.ts";

export type PlanItem = {
  task: Task;
  /** Drafts the planner split out of this one, in planned order. */
  children: PlanChild[];
  /** Every dependency is done (or there is none). */
  ready: boolean;
  /** Titles this draft comes after. */
  after: string[];
};
export type PlanChild = {
  task: Task;
  /** Titles this draft comes after. */
  after: string[];
  waits: boolean;
};
export type PlanModel = {
  next: PlanItem[];
  later: PlanItem[];
};

export function criteriaLabel(count: number): string {
  return `${count} ${count === 1 ? "criterion" : "criteria"}`;
}

/** "hard · 5 criteria": the meta a draft row carries. */
export function draftMeta(task: Task): string {
  return `${task.tier} · ${criteriaLabel(task.criteria.length)}`;
}

function depsDone(task: Task, tasks: Task[]): boolean {
  return (task.dependsOn ?? []).every(
    (id) => tasks.find((t) => t.id === id)?.status === "done",
  );
}

/** A draft that never ran: in the backlog (orchd only lets an unstarted
 * task in), or queued/stopped with no attempt of any stage yet. A task that
 * is planning or waits for a slot has started and is not a draft. */
export function unstartedDraft(task: Task): boolean {
  return !!task.backlog || task.attempts.length === 0;
}

/** The tasks only the Plan lists, never the rail: unstarted drafts - but not
 * one that needs the owner, such as a task the engine stopped before its
 * first attempt. */
export function planOnly(tasks: Task[]): Set<Task> {
  return new Set(
    planDrafts(tasks).filter(
      (task) => unstartedDraft(task) && !needsOwner(task),
    ),
  );
}

/** Backlog tasks by their order first, then the rest oldest first. */
function byPlanOrder(a: Task, b: Task): number {
  const ao = a.backlog?.order;
  const bo = b.backlog?.order;
  if (ao !== undefined && bo !== undefined && ao !== bo) return ao - bo;
  if (ao !== undefined && bo === undefined) return -1;
  if (ao === undefined && bo !== undefined) return 1;
  return a.createdAt - b.createdAt;
}

/** The Plan's lists. A draft whose planner-split children are drafts too
 * becomes a group. LATER holds the `later` backlog bucket; NEXT holds the
 * `next` bucket and every draft outside the backlog after it. */
export function planModel(tasks: Task[]): PlanModel {
  const drafts = planDrafts(tasks).filter(unstartedDraft);
  const ids = new Set(drafts.map((t) => t.id));
  const top = drafts
    .filter((t) => !t.parent || !ids.has(t.parent))
    .sort(byPlanOrder);
  const items: PlanItem[] = top.map((task) => ({
    task,
    children: drafts
      .filter((t) => t.parent === task.id)
      .sort((a, b) => a.createdAt - b.createdAt)
      .map((child) => ({
        task: child,
        after: dependencyTitles(child, tasks),
        waits: !depsDone(child, tasks),
      })),
    ready: depsDone(task, tasks),
    after: dependencyTitles(task, tasks),
  }));
  return {
    next: items.filter((i) => i.task.backlog?.bucket !== "later"),
    later: items.filter((i) => i.task.backlog?.bucket === "later"),
  };
}

/** The `task.backlog` calls that move `items[index]` one place up (-1) or
 * down (+1) in its bucket: the list renumbered 0..n in its new order down to
 * the moved pair (a draft outside the backlog below them stays out), only
 * the tasks whose order changes. Nothing when it cannot move. */
export function backlogMoves(
  items: PlanItem[],
  index: number,
  step: -1 | 1,
  bucket: BacklogBucket,
): { id: string; order: number }[] {
  const to = index + step;
  if (index < 0 || to < 0 || to >= items.length) return [];
  const order = items.map((i) => i.task);
  [order[index], order[to]] = [order[to], order[index]];
  return order
    .slice(0, Math.max(index, to) + 1)
    .flatMap((task, at) =>
      task.backlog?.bucket === bucket && task.backlog.order === at
        ? []
        : [{ id: task.id, order: at }],
    );
}

/** The draft the autopilot starts next: the first ready NEXT draft in the
 * backlog (the autopilot never starts a draft outside it). */
export function autopilotNext(model: PlanModel): Task | undefined {
  return model.next.find((i) => i.ready && i.task.backlog)?.task;
}

/** Drafts "Start n ready" starts: every NEXT top-level draft that is not
 * waiting on anything. */
export function readyDrafts(model: PlanModel): Task[] {
  return model.next.filter((i) => i.ready).map((i) => i.task);
}
