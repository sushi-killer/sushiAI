// Where the workspace's project lives on a host the Orchestrator panel
// switches to, and where a task created for another host is shown.
import type { Bridge, Project } from "../types.ts";
import type { TaskTarget } from "./notices.ts";

export type HostProject =
  /** The host has the project's checkout: open it there. */
  | { kind: "open"; repo: string; project: Project }
  /** The host lacks it: offer to clone and install it there. */
  | { kind: "bring"; project: Project }
  /** No project with a Git remote, or the host can't be asked: ask for a
   * repo path. */
  | { kind: "pick" };

/** The workspace's project on `host`. A checkout whose install is stale
 * still opens: the next start reinstalls it. */
export async function projectOnHost({
  bridge,
  endpoint,
  cwd,
  host,
}: {
  bridge: Pick<Bridge, "projectsResolve" | "projectHostCheck">;
  /** The workspace's own host: "ssh:<id>" or nothing for this machine. */
  endpoint?: string;
  cwd: string;
  host: string;
}): Promise<HostProject> {
  try {
    const project = await bridge.projectsResolve({
      endpoint: endpoint || "local",
      cwd,
    });
    if (!project?.git?.url) return { kind: "pick" };
    const readiness = await bridge.projectHostCheck(project.id, host);
    return readiness.checkout.ok && readiness.checkout.path
      ? { kind: "open", repo: readiness.checkout.path, project }
      : { kind: "bring", project };
  } catch {
    return { kind: "pick" };
  }
}

/** A task the composer created on a host other than the panel's: the panel
 * switches there and opens it. Null when it runs on the panel's host. */
export function createdTaskTarget(
  task: { id: string; repo: string },
  { panelHost, runHost }: { panelHost: string; runHost: string },
): TaskTarget | null {
  if (runHost === panelHost) return null;
  return { taskId: task.id, repo: task.repo, host: runHost, focus: "summary" };
}
