// Pure logic behind the Orchestrator's host selector: which host an event or
// task belongs to, what a host's preflight rules out, and which repo paths to
// suggest on a host. No React, no bridge.
import type { Workspace } from "../types.ts";
import { planDrafts } from "./helpers.ts";
import { unstartedDraft } from "./planModel.ts";
import type { OrchestratorHost, Preflight, Route, Task } from "./types.ts";

export const LOCAL_HOST = "local";

/** The host a task, event or notice belongs to; local when it names none. */
export function hostOf(item: { host?: string } | null | undefined): string {
  return item?.host || LOCAL_HOST;
}

/** Why a host cannot run a route's harness, or null when it can (or when
 * nothing is known yet - a host that was never probed rules nothing out). */
export function routeProblem(
  preflight: Preflight | null | undefined,
  route: Pick<Route, "harness">,
): string | null {
  const state = preflight?.[route.harness];
  if (!state) return null;
  if (!state.installed) return `${route.harness} is not installed on this host`;
  if (!state.loggedIn) return `${route.harness} is not logged in on this host`;
  return null;
}

/** The routes a host cannot run, id -> reason. */
export function unavailableRoutes(
  preflight: Preflight | null | undefined,
  routes: Route[],
): Record<string, string> {
  const result: Record<string, string> = {};
  for (const route of routes) {
    const problem = routeProblem(preflight, route);
    if (problem) result[route.id] = problem;
  }
  return result;
}

export function hostName(hosts: OrchestratorHost[], id: string): string {
  return hosts.find((host) => host.id === id)?.name ?? id.replace(/^ssh:/, "");
}

/** Repo paths worth offering on a host: those of the workspaces opened on it,
 * then those of its existing tasks - each once, most recent first. */
export function repoSuggestions(
  host: string,
  workspaces: Pick<Workspace, "cwd" | "connection">[],
  tasks: Pick<Task, "repo" | "updatedAt">[],
): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const add = (repo: string | undefined) => {
    if (repo && !seen.has(repo)) {
      seen.add(repo);
      out.push(repo);
    }
  };
  for (const workspace of workspaces)
    if ((workspace.connection || LOCAL_HOST) === host) add(workspace.cwd);
  for (const task of [...tasks].sort((a, b) => b.updatedAt - a.updatedAt))
    add(task.repo);
  return out;
}

export type HostTone = "ok" | "warning" | "danger" | "neutral";

/** The daemon state the panel body last saw for the host it shows. */
export type DaemonReach = "loading" | "ready" | "not-built" | "unavailable";

/** The error a host with an older sushiai answers: it has no `orch`
 * capability (electron/orchestrator.cjs `missingCapability`). */
export function needsSushiaiUpdate(detail: string | undefined): boolean {
  return /^Update sushiai on /.test(detail ?? "");
}

/** The failures the one-button "Install sushiai" setup fixes: no sushiai on
 * the host, or a daemon that is not the bundled version. */
function installable(detail: string): boolean {
  return (
    needsSushiaiUpdate(detail) ||
    /not installed on this host|incompatible|not the one bundled/i.test(detail)
  );
}

/** A remote host is still being brought up (or failed to be): the panel
 * shows the setup card instead of its tasks. A host that answered once in
 * this panel keeps its tasks behind the offline banner while the app
 * reconnects it. */
export function needsSetup(
  host: Pick<OrchestratorHost, "id" | "state">,
  answeredOnce: boolean,
): boolean {
  if (host.id === LOCAL_HOST || answeredOnce) return false;
  return host.state !== "ready";
}

/** The host as the panel shows it: while the main process retries a host
 * that failed on its own, its failure stays up instead of flickering back
 * to "connecting" on every retry. */
export function shownHost<T extends Pick<OrchestratorHost, "state" | "detail">>(
  host: T,
  lastError: string,
): T {
  return lastError && (host.state === "connecting" || host.state === "idle")
    ? { ...host, state: "error", detail: lastError }
    : host;
}

/** The status dot and the second line of the host selector. `daemon` is
 * what the panel body saw for the host it shows (undefined for others). */
export function hostStatus(
  host: Pick<OrchestratorHost, "id" | "state" | "detail" | "enabled">,
  daemon?: DaemonReach,
): { tone: HostTone; detail: string } {
  const local = host.id === LOCAL_HOST;
  const where = local ? "This Mac" : "SSH";
  if (daemon === "not-built")
    return { tone: "warning", detail: `${where} · sushiai not built` };
  if (daemon === "unavailable")
    return { tone: "danger", detail: `${where} · sushiai unreachable` };
  if (local)
    return daemon === "loading"
      ? { tone: "warning", detail: "This Mac · connecting" }
      : { tone: "ok", detail: "This Mac · connected" };
  if (host.state === "connecting")
    return { tone: "warning", detail: "SSH · connecting to sushiai" };
  if (host.state === "error")
    return needsSushiaiUpdate(host.detail)
      ? { tone: "warning", detail: "SSH · sushiai needs an update" }
      : { tone: "danger", detail: "SSH · sushiai unreachable" };
  if (host.state === "ready")
    return { tone: "ok", detail: "SSH · sushiai connected" };
  return {
    tone: "neutral",
    detail: host.enabled ? "SSH · connecting" : "SSH · not set up",
  };
}

export type SetupStepState = "done" | "active" | "failed" | "pending";
export type SetupStep = {
  title: string;
  detail: string;
  state: SetupStepState;
  /** A button that gets past the failed step without the owner touching the
   * host: it installs the bundled sushiai there. */
  action?: { label: string; hint: string };
};

/** What the setup card knows beyond the host record: the states this panel
 * saw it pass through and the SSH address of its Connections profile. */
export type SetupSeen = {
  /** States seen, oldest first; the last is the current one. */
  states: OrchestratorHost["state"][];
  address?: string;
};

/** The three-step setup card: connect over SSH, reach the sushiai daemon on
 * the host (the proxy starts it), find the orchestrator in it. The host's
 * own daemon manager state says which step a failure belongs to. */
export function setupSteps(
  host: Pick<OrchestratorHost, "state" | "detail"> &
    Partial<Pick<OrchestratorHost, "name">>,
  seen: SetupSeen,
): SetupStep[] {
  const detail = host.detail ?? "";
  let current: number;
  let failed = false;
  switch (host.state) {
    case "ready":
      current = 3;
      break;
    case "error":
      failed = true;
      current = needsSushiaiUpdate(detail)
        ? 2
        : installable(detail)
          ? 1
          : seen.states.includes("connecting")
            ? 1
            : 0;
      break;
    default:
      current = seen.states.includes("connecting") ? 1 : 0;
  }
  const state = (index: number): SetupStepState =>
    index < current
      ? "done"
      : index > current
        ? "pending"
        : failed
          ? "failed"
          : "active";
  const failure = (index: number) =>
    failed && current === index ? detail : "";
  const action =
    failed && installable(detail)
      ? {
          label: needsSushiaiUpdate(detail)
            ? "Update sushiai"
            : "Install sushiai",
          hint: `Installs the sushiai that ships with this app on ${host.name ?? "the host"} (no sudo) and restarts its daemon. Running tasks keep going.`,
        }
      : undefined;
  return [
    {
      title: current > 0 ? "Connected over SSH" : "Connecting over SSH",
      detail: failure(0) || seen.address || "over your Connections profile",
      state: state(0),
    },
    {
      title: current > 1 ? "Reached the sushiai daemon" : "Reach the daemon",
      detail: failure(1) || "ssh host sushiai proxy starts it when needed",
      state: state(1),
      ...(current === 1 && action ? { action } : {}),
    },
    {
      title: current > 2 ? "Found the orchestrator" : "Find the orchestrator",
      detail: failure(2) || "the daemon's orch module",
      state: state(2),
      ...(current === 2 && action ? { action } : {}),
    },
  ];
}

export type PreflightItem = {
  name: "git" | "claude" | "codex";
  ok: boolean;
  note: string;
};

/** The preflight strip: git, then each harness CLI with its login state. */
export function preflightItems(preflight: Preflight): PreflightItem[] {
  const harness = (name: "claude" | "codex"): PreflightItem => {
    const state = preflight[name];
    return {
      name,
      ok: state.installed && state.loggedIn,
      note: !state.installed
        ? "not installed"
        : state.loggedIn
          ? "logged in"
          : "not logged in",
    };
  };
  return [
    { name: "git", ok: preflight.git, note: preflight.git ? "" : "missing" },
    harness("claude"),
    harness("codex"),
  ];
}

/** What a host missing a harness means: the orchestrator falls back to an installed CLI
 * for those routes. "codex is not installed on lab; its routes fall back to an
 * installed CLI."; empty when every route can run. */
export function routesFallbackNote(
  preflight: Preflight,
  hostName: string,
): string {
  return (["claude", "codex"] as const)
    .filter((name) => !(preflight[name].installed && preflight[name].loggedIn))
    .map(
      (name) =>
        `${name} is not ${preflight[name].installed ? "logged in" : "installed"} on ${hostName}; its routes fall back to an installed CLI.`,
    )
    .join(" ");
}

/** Tasks a host counts as running in the selector: started work, not the
 * drafts that wait on the Plan. */
export function runningCount(tasks: Task[]): number {
  const drafts = new Set(planDrafts(tasks).filter(unstartedDraft));
  return tasks.filter(
    (task) =>
      !task.archived &&
      !drafts.has(task) &&
      (task.status === "running" ||
        task.status === "drafting" ||
        task.status === "queued"),
  ).length;
}
