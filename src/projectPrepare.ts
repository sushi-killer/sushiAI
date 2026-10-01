import type { ProjectHostReadiness, ProjectPrepareStep } from "./types";

const timesKey = (project: string, host: string) =>
  `sushiai.project.prepare-times:${project}:${host}`;

/** Remembers what each step of a successful prepare took, so the next plan
 * can say how long to expect. */
export function rememberPrepareTimes(
  project: string,
  host: string,
  steps: ProjectPrepareStep[] | undefined,
): void {
  try {
    window.localStorage.setItem(
      timesKey(project, host),
      JSON.stringify(steps ?? []),
    );
  } catch {
    // Private mode: no estimates, nothing else lost.
  }
}

/** The steps of the last successful prepare on a host, or none. */
export function readPrepareTimes(
  project: string,
  host: string,
): ProjectPrepareStep[] | undefined {
  try {
    const raw = window.localStorage.getItem(timesKey(project, host));
    const steps = raw ? (JSON.parse(raw) as ProjectPrepareStep[]) : undefined;
    return Array.isArray(steps) ? steps : undefined;
  } catch {
    return undefined;
  }
}

/** `~/sushiai/app` for a checkout in the standard place, else its path. */
export function checkoutPath(matrix: ProjectHostReadiness): string {
  const { path, nonStandard } = matrix.checkout;
  return nonStandard ? path : `~/sushiai/${path.split("/").pop()}`;
}

/** The folder name a project gets under ~/sushiai. The main process makes the
 * same name for the folder it clones into (`projectSlug` in project-hosts). */
export function projectSlug(name: string): string {
  return (
    name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "project"
  );
}

/** `acme/app` from any spelling of a git remote. */
export function repoSlug(url: string): string {
  return url
    .trim()
    .replace(/\.git$/i, "")
    .replace(/\/+$/, "")
    .split(/[/:]/)
    .filter(Boolean)
    .slice(-2)
    .join("/");
}

/** `~/…` for a path under the home folder. */
export function tildePath(path: string, home?: string): string {
  return home && (path === home || path.startsWith(`${home}/`))
    ? `~${path.slice(home.length)}`
    : path;
}

/** Where a project lives on This Mac: the folder it was opened from when that
 * folder is on this Mac, else its own folder under ~/sushiai. A folder chosen
 * on another host is never a path on this Mac. */
export function localTarget(input: {
  reuseFolder: boolean;
  cwd: string;
  home: string;
  name: string;
}): string {
  return input.reuseFolder
    ? input.cwd
    : `${input.home}/sushiai/${projectSlug(input.name)}`;
}
