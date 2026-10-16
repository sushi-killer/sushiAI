import type { Harness, RoutinesCatalog } from "./types.ts";

/** The most projects one routine may watch (the daemon refuses more). */
export const MAX_PROJECTS = 20;

type CatalogProjects = Pick<RoutinesCatalog, "projects">;

/** Project ids are stored; an older file may hold a name. Resolve both. */
export function resolveProject(
  catalog: CatalogProjects | null,
  key: string,
): { id: string; name: string } | null {
  const found = catalog?.projects.find(
    (item) => item.id === key || item.name === key,
  );
  return found ? { id: found.id, name: found.name } : null;
}

/** The projects a routine on `hostId` may pick: the ones with a folder there
 * or a remote to clone from, plus the ones already selected. */
export function projectsForHost(
  catalog: RoutinesCatalog | null,
  hostId: string,
  selected: string[] = [],
) {
  return (catalog?.projects ?? []).filter(
    (project) =>
      selected.includes(project.id) ||
      selected.includes(project.name) ||
      project.folders.some((folder) => folder.host === hostId) ||
      project.remote,
  );
}

/** "host", "host · my-app" or "host · 3 projects". */
export function describeWhere(
  where: { projects?: string[] },
  catalog: CatalogProjects | null,
  hostName: string,
): string {
  const projects = where.projects ?? [];
  if (projects.length === 0) return hostName;
  if (projects.length > 1) return `${hostName} · ${projects.length} projects`;
  return `${hostName} · ${resolveProject(catalog, projects[0])?.name ?? projects[0]}`;
}

/** Add the project to the selection, or remove it. The selection holds ids. */
export function toggleProject(
  projects: string[],
  catalog: CatalogProjects | null,
  id: string,
): string[] {
  const same = (key: string) =>
    resolveProject(catalog, key)?.id === id || key === id;
  if (projects.some(same)) return projects.filter((key) => !same(key));
  return projects.length >= MAX_PROJECTS ? projects : [...projects, id];
}

/** The selection that survives a move to another host: a project stays when
 * it has a folder there or a remote to clone from. */
export function keepOnHost(
  projects: string[],
  catalog: RoutinesCatalog | null,
  hostId: string,
): string[] {
  const allowed = projectsForHost(catalog, hostId);
  return projects.filter((key) =>
    allowed.some((project) => project.id === key || project.name === key),
  );
}

/** Why the picked projects do not work with the harness, or "". */
export function whereProblem(harness: Harness, projects: string[]): string {
  return harness === "codex" && projects.length > 1
    ? "More than one project needs Claude"
    : "";
}
