import { daemonHost } from "../daemonSessions.ts";
import type { DaemonEvent, Panel, Workspace } from "../types.ts";
import type { ExtensionRegistry } from "./registry.ts";

/** `params` of a daemon `session.open` notification: an agent ran
 * `sushiai open <target> <absolute path>` inside its session. */
export type OpenSignal = {
  host: string;
  sessionId: string;
  extensionId: string;
  surfaceId: string;
  nonce: string;
  arg: string;
};

export type OpenResolution =
  | { kind: "rejected"; reason: string }
  | { kind: "companion"; workspace: Workspace; panel: Panel };

/** The daemon only carries `a-z`, digits and `-` in a target, so an extension
 * id has no dots there: `artifacts/preview` names `builtin.artifacts`. A
 * dotted id is taken as it is. */
export function extensionIdOfTarget(name: string): string {
  return name.includes(".") ? name : `builtin.${name}`;
}

/** A `session.open` event as a signal, or null for any other or malformed
 * event. */
export function parseOpenEvent(event: DaemonEvent): OpenSignal | null {
  if (event.method !== "session.open") return null;
  const { id, target, arg, nonce } = event.params;
  if (
    typeof id !== "string" ||
    typeof target !== "string" ||
    typeof arg !== "string" ||
    typeof nonce !== "string" ||
    !id ||
    !nonce
  )
    return null;
  const slash = target.indexOf("/");
  if (slash < 1 || slash === target.length - 1) return null;
  const surfaceId = target.slice(slash + 1);
  if (surfaceId.includes("/")) return null;
  return {
    host: event.host,
    sessionId: id,
    extensionId: extensionIdOfTarget(target.slice(0, slash)),
    surfaceId,
    nonce,
    arg,
  };
}

/** The pane to open the surface beside: the panel bound to the agent's session
 * on that host. */
export function resolveOpenSignal(
  registry: ExtensionRegistry,
  workspaces: Workspace[],
  signal: OpenSignal,
): OpenResolution {
  const name = `${signal.extensionId}/${signal.surfaceId}`;
  const surface = registry
    .availableSurfaces()
    .find(
      (item) =>
        item.extensionId === signal.extensionId && item.id === signal.surfaceId,
    );
  if (!surface)
    return { kind: "rejected", reason: `Unavailable surface: ${name}.` };
  if (surface.view.kind !== "core")
    return {
      kind: "rejected",
      reason: `Surface ${name} cannot be opened in a pane.`,
    };
  if (!surface.allowedHosts.includes("workspace.pane"))
    return {
      kind: "rejected",
      reason: `Surface ${name} is not a workspace pane.`,
    };
  for (const workspace of workspaces) {
    if (daemonHost(workspace.connection) !== signal.host) continue;
    const panel = workspace.panels.find(
      (item) => item.sessionId === signal.sessionId,
    );
    if (panel) return { kind: "companion", workspace, panel };
  }
  return {
    kind: "rejected",
    reason: `Session ${signal.sessionId} has no panel on ${signal.host}.`,
  };
}

/** How many nonces are remembered: far more than an agent opens in a session. */
const NONCES_KEPT = 256;

/** Remembers a request's nonce. `fresh` is false when it was seen before, so
 * a repeated notification never opens a second pane. `seen` is not mutated. */
export function rememberNonce(
  seen: string[],
  nonce: string,
): { seen: string[]; fresh: boolean } {
  if (seen.includes(nonce)) return { seen, fresh: false };
  return { seen: [...seen, nonce].slice(-NONCES_KEPT), fresh: true };
}

type OpenTarget = { extensionId: string; surfaceId: string };

/** The daemon-event handler behind `useOpenSignals`, without React: a
 * `session.open` for a known surface opens it as the companion of the panel
 * bound to that session. Any other event, a repeated nonce, or a request that
 * cannot be honoured does nothing (the last one with a warning). */
export function createOpenHandler(
  latest: () => { workspaces: Workspace[]; registry: ExtensionRegistry },
  openCompanion: (
    panelId: string,
    target: OpenTarget,
    args: { arg: string },
  ) => void,
  warn: (message: string) => void,
): (event: DaemonEvent) => void {
  let seen: string[] = [];
  return (event) => {
    const signal = parseOpenEvent(event);
    if (!signal) return;
    const memo = rememberNonce(seen, signal.nonce);
    if (!memo.fresh) return;
    seen = memo.seen;
    const { workspaces, registry } = latest();
    const open = resolveOpenSignal(registry, workspaces, signal);
    if (open.kind === "rejected") {
      warn(`sushiai open: ${open.reason}`);
      return;
    }
    openCompanion(
      open.panel.id,
      { extensionId: signal.extensionId, surfaceId: signal.surfaceId },
      { arg: signal.arg },
    );
  };
}
