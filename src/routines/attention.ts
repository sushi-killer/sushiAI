// Inbox rows of Routines. A row exists only for what needs the owner: a run
// that needs them, a failed run, a routine the daemon paused. The daemon marks
// those events `inbox: true`; this file never decides that on its own.
import type { AttentionItem } from "../extensions/modules.ts";
import { routineState } from "./model.ts";
import type { RoutineEntry, RoutinesEvent } from "./types.ts";

export const attentionKey = (host: string, routineId: string, tag: string) =>
  `routines:${host}:${routineId}:${tag}`;

/** The row one event asks for, or null. A finished run of status noop never
 * gets one, whatever the flag says. */
export function attentionFromEvent(
  event: RoutinesEvent,
  host: string,
  at = Date.now(),
): AttentionItem | null {
  if (event.inbox !== true) return null;
  const name = event.name || event.routineId;
  if (event.kind === "paused")
    return {
      key: attentionKey(host, event.routineId, "paused"),
      kind: "decide",
      title: `${name} was paused`,
      project: name,
      host,
      at,
      search: `${name} paused`.toLowerCase(),
      meta: `paused after ${event.failures ?? 3} failed runs${
        event.reason ? ` · ${event.reason}` : ""
      }`,
      actions: [{ id: "dismiss", label: "Dismiss" }],
    };
  if (event.kind !== "run_finished") return null;
  if (event.status !== "needs_you" && event.status !== "failed") return null;
  const needs = event.status === "needs_you";
  return {
    key: attentionKey(host, event.routineId, `run-${event.run ?? 0}`),
    kind: needs ? "answer" : "decide",
    title: event.title || (needs ? `${name} needs you` : `${name} failed`),
    project: name,
    host,
    at,
    search: `${name} ${event.title ?? ""}`.toLowerCase(),
    meta: needs
      ? "needs you"
      : `failed${event.reason ? ` · ${event.reason}` : ""}`,
    actions: [{ id: "dismiss", label: "Dismiss" }],
  };
}

/** The rows a fresh list implies, so a restart does not lose them: the last
 * real run needs the owner or failed, or the routine is paused. */
export function attentionFromEntries(entries: RoutineEntry[]): AttentionItem[] {
  const items: AttentionItem[] = [];
  for (const entry of entries) {
    const state = routineState(entry);
    const { routine, lastRun } = entry;
    const item =
      state === "paused"
        ? attentionFromEvent(
            {
              kind: "paused",
              routineId: routine.id,
              name: routine.name,
              failures: entry.consecutiveFailures,
              reason: lastRun?.reason,
              inbox: true,
            },
            entry.host,
            lastRun?.endedAt ?? lastRun?.startedAt,
          )
        : (state === "needs_you" || state === "failed") && lastRun
          ? attentionFromEvent(
              {
                kind: "run_finished",
                routineId: routine.id,
                name: routine.name,
                run: lastRun.n,
                status: lastRun.status,
                title: lastRun.title,
                reason: lastRun.reason,
                inbox: true,
              },
              entry.host,
              lastRun.endedAt ?? lastRun.startedAt,
            )
          : null;
    if (item) items.push(item);
  }
  return items;
}
