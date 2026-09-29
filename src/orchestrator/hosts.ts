// Pure logic behind the Orchestrator's host selector: which host an event or
// task belongs to, what a host's preflight rules out, and which repo paths to
// suggest on a host. No React, no bridge.
import type { Workspace } from "../types.ts";
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

/** Host-level problems worth a line in the panel: no git, no usable CLI. */
export function preflightProblems(preflight: Preflight | null | undefined) {
  if (!preflight) return [];
  const problems: string[] = [];
  if (!preflight.git) problems.push("git is not installed");
  for (const harness of ["claude", "codex"] as const) {
    const state = preflight[harness];
    if (!state.installed) problems.push(`${harness} is not installed`);
    else if (!state.loggedIn) problems.push(`${harness} is not logged in`);
  }
  return problems;
}

/** Task rows name their host only when more than one host is in use. */
export function hostsInUse(hosts: OrchestratorHost[]): number {
  return hosts.filter((host) => host.enabled).length;
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
