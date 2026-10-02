import type { Panel, Workspace } from "../types.ts";
import type { ExtensionRegistry } from "./registry.ts";

/** The workspace metadata token an agent sets to ask for a surface as the
 * companion half of its own pane. */
export const OPEN_TOKEN = "sushiai_open";
/** Herdr cuts a token value at 80 characters without an error, so the
 * argument travels in its own token, set in the same call. */
export const OPEN_ARG_TOKEN = "sushiai_open_arg";

export type OpenSignal = {
  paneId: string;
  extensionId: string;
  surfaceId: string;
  nonce: string;
  arg: string;
};

export type OpenResolution =
  | { kind: "unavailable"; reason: string }
  | { kind: "companion"; panelId: string };

/** "<paneId> <extensionId>/<surfaceId> <nonce> <arg>": the nonce is one word
 * that changes on every request, so asking for the same file again is still a
 * new value. The argument is everything after the third space, so a path may
 * contain spaces. */
export function parseOpenSignal(value: unknown): OpenSignal | null {
  if (typeof value !== "string") return null;
  const first = value.indexOf(" ");
  if (first < 1) return null;
  const paneId = value.slice(0, first);
  const rest = value.slice(first + 1);
  const second = rest.indexOf(" ");
  const target = second < 0 ? rest : rest.slice(0, second);
  const afterTarget = second < 0 ? "" : rest.slice(second + 1);
  const third = afterTarget.indexOf(" ");
  const nonce = third < 0 ? afterTarget : afterTarget.slice(0, third);
  const arg = third < 0 ? "" : afterTarget.slice(third + 1);
  if (!nonce) return null;
  const slash = target.indexOf("/");
  if (slash < 1 || slash === target.length - 1) return null;
  const extensionId = target.slice(0, slash);
  const surfaceId = target.slice(slash + 1);
  if (surfaceId.includes("/")) return null;
  return { paneId, extensionId, surfaceId, nonce, arg };
}

export function resolveOpenSignal(
  registry: ExtensionRegistry,
  panels: Panel[],
  signal: OpenSignal,
): OpenResolution {
  const agent = panels.find((panel) => panel.herdrId === signal.paneId);
  if (!agent)
    return {
      kind: "unavailable",
      reason: `Pane ${signal.paneId} is not in this workspace.`,
    };
  const surface = registry
    .availableSurfaces()
    .find(
      (item) =>
        item.extensionId === signal.extensionId && item.id === signal.surfaceId,
    );
  const name = `${signal.extensionId}/${signal.surfaceId}`;
  if (!surface)
    return { kind: "unavailable", reason: `Unavailable surface: ${name}.` };
  if (surface.view.kind !== "core")
    return {
      kind: "unavailable",
      reason: `Surface ${name} cannot be opened in a pane.`,
    };
  if (!surface.allowedHosts.includes("workspace.pane"))
    return {
      kind: "unavailable",
      reason: `Surface ${name} is not a workspace pane.`,
    };
  return { kind: "companion", panelId: agent.id };
}

type Fresh = { workspace: Workspace; value: string };

/** The first value seen for a workspace is its baseline and is not acted on,
 * so restarting the app never reopens what was asked for last session. Any
 * later change is returned once. `seen` is not mutated. */
export function detectOpenSignals(
  seen: Record<string, string>,
  workspaces: Workspace[],
): { seen: Record<string, string>; fresh: Fresh[] } {
  const next = { ...seen };
  const fresh: Fresh[] = [];
  for (const workspace of workspaces) {
    if (!workspace.herdrTokens) continue;
    const open = workspace.herdrTokens[OPEN_TOKEN] ?? "";
    const arg = workspace.herdrTokens[OPEN_ARG_TOKEN] ?? "";
    const value = open && arg ? `${open} ${arg}` : open;
    if (workspace.id in seen && seen[workspace.id] !== value && value)
      fresh.push({ workspace, value });
    next[workspace.id] = value;
  }
  return { seen: next, fresh };
}

/** A relative argument is a path from the agent pane's folder. */
export function absoluteArg(arg: string, base: string | undefined): string {
  if (!arg || arg.startsWith("/") || !base?.startsWith("/")) return arg;
  return `${base.replace(/\/+$/, "")}/${arg.replace(/^\.\//, "")}`;
}
