// A one-slot mailbox between "open this task" (a toast's Open button, a
// native notification click) and the Orchestrator panel that will show it.
// The panel may not exist yet, and opening it can remount it (added, then
// zoomed), so a request is not used up by the first panel that reads it: it
// stays readable for a moment, and each panel instance applies it once.
import type { TaskTarget } from "./notices.ts";

export const REVEAL_TTL_MS = 3000;

let pending: { target: TaskTarget; at: number } | null = null;
const listeners = new Set<() => void>();

export function publishReveal(target: TaskTarget, now = Date.now()): void {
  pending = { target, at: now };
  for (const listener of [...listeners]) listener();
}

/** The request for `repo`, while it is fresh. Same object until replaced. */
export function pendingReveal(
  repo: string,
  now = Date.now(),
): TaskTarget | null {
  // A remote task's repo is a path on its host, so any panel takes it and
  // switches to that host; a local task goes to the panel of its own repo.
  if (!pending || (!pending.target.host && pending.target.repo !== repo))
    return null;
  if (now - pending.at > REVEAL_TTL_MS) return null;
  return pending.target;
}

export function subscribeReveal(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function resetReveal(): void {
  pending = null;
  listeners.clear();
}
