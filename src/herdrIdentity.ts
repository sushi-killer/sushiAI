import type { Layout, Workspace } from "./types";
import type { Saved } from "./workspaceState.ts";

function endpointPrefix(endpoint: string): string {
  return `herdr:v2:${encodeURIComponent(endpoint)}:`;
}

export function herdrWorkspaceKey(endpoint: string, id: string): string {
  return `${endpointPrefix(endpoint)}${encodeURIComponent(id)}`;
}

export function closedProjectKey(
  endpoint: string | undefined,
  cwd: string,
): string {
  return `closed:${endpoint || "local"}:${cwd}`;
}

type Reference = { id: string; owner: number };
type References = Map<string, Reference[]>;

function addReference(
  refs: References,
  old: string,
  id: string,
  owner: number,
) {
  const entries = refs.get(old) || [];
  entries.push({ id, owner });
  refs.set(old, entries);
}

function resolveReference(
  refs: References,
  id: string,
  owner?: number,
): string {
  const entries = refs.get(id);
  return (
    entries?.find((entry) => entry.owner === owner)?.id ||
    entries?.[0]?.id ||
    id
  );
}

function mapLayout(layout: Layout, map: (id: string) => string): Layout;
function mapLayout(
  layout: Layout | null,
  map: (id: string) => string,
): Layout | null;
function mapLayout(
  layout: Layout | null,
  map: (id: string) => string,
): Layout | null {
  if (!layout) return null;
  const mapped = new Map<Layout, Layout>();
  const stack: [Layout, boolean][] = [[layout, false]];
  while (stack.length) {
    const [node, visited] = stack.pop()!;
    if (node.type === "leaf") {
      const id = map(node.id);
      mapped.set(node, id === node.id ? node : { ...node, id });
    } else if (!visited) {
      stack.push([node, true], [node.b, false], [node.a, false]);
    } else {
      const a = mapped.get(node.a)!;
      const b = mapped.get(node.b)!;
      mapped.set(node, a === node.a && b === node.b ? node : { ...node, a, b });
    }
  }
  return mapped.get(layout)!;
}

function migrateMergedLayout(
  layout: Layout,
  refs: References,
  activeOwner?: number,
): Layout {
  const counts = new Map<string, number>();
  const owners = new Set<number>();
  mapLayout(layout, (id) => {
    counts.set(id, (counts.get(id) || 0) + 1);
    const matches = refs.get(id);
    if (matches?.length === 1) owners.add(matches[0].owner);
    return id;
  });
  const used = new Map<string, number>();
  return mapLayout(layout, (id) => {
    const entries = refs.get(id);
    if (!entries?.length) return id;
    const scoped = entries.filter((entry) => owners.has(entry.owner));
    const matches = scoped.length ? scoped : entries;
    const index = used.get(id) || 0;
    used.set(id, index + 1);
    if ((counts.get(id) || 0) >= matches.length)
      return matches[index % matches.length].id;
    return (
      matches.find((entry) => entry.owner === activeOwner)?.id || matches[0].id
    );
  });
}

/** The endpoint comes from the owning workspace, never from an old local ID.
 * Ambiguous global selection uses the saved endpoint; layouts use their owner. */
export function migrateHerdrIdentities(saved: Saved): Saved {
  const workspaceRefs: References = new Map();
  const panelRefs: References = new Map();
  let changed = false;
  const workspaces = saved.workspaces.map((workspace, owner): Workspace => {
    const endpoint = workspace.connection || saved.socket;
    const id =
      workspace.herdrId &&
      endpoint &&
      !workspace.id.startsWith(endpointPrefix(endpoint))
        ? herdrWorkspaceKey(endpoint, workspace.herdrId)
        : workspace.id;
    addReference(workspaceRefs, workspace.id, id, owner);
    const ownPanels: References = new Map();
    const panels = workspace.panels.map((panel) => {
      const nextId =
        panel.herdrId &&
        endpoint &&
        !panel.id.startsWith(endpointPrefix(endpoint))
          ? herdrWorkspaceKey(endpoint, panel.herdrId)
          : panel.id;
      addReference(panelRefs, panel.id, nextId, owner);
      addReference(ownPanels, panel.id, nextId, owner);
      if (nextId === panel.id) return panel;
      changed = true;
      return { ...panel, id: nextId };
    });
    const layout = mapLayout(workspace.layout, (old) =>
      resolveReference(ownPanels, old, owner),
    );
    if (
      id === workspace.id &&
      panels.every((panel, index) => panel === workspace.panels[index]) &&
      layout === workspace.layout
    )
      return workspace;
    changed = true;
    return { ...workspace, id, panels, layout };
  });
  const closedProjects = saved.closedProjects?.map((project) => {
    const id = closedProjectKey(project.endpoint, project.cwd);
    addReference(workspaceRefs, project.id, id, -1);
    if (id === project.id) return project;
    changed = true;
    return { ...project, id };
  });
  if (!changed) return saved;

  const activeMatches = workspaceRefs.get(saved.activeId) || [];
  const activeOwner = (
    activeMatches.find(
      (match) => saved.workspaces[match.owner]?.connection === saved.socket,
    ) || activeMatches[0]
  )?.owner;
  const activeId = resolveReference(workspaceRefs, saved.activeId, activeOwner);
  const views = Object.fromEntries(
    Object.entries(saved.views || {}).flatMap(([key, view]) => {
      const owners = workspaceRefs.get(key);
      return owners?.length
        ? owners.map((owner) => [
            owner.id,
            {
              ...view,
              zoomed:
                view.zoomed === null
                  ? null
                  : resolveReference(panelRefs, view.zoomed, owner.owner),
            },
          ])
        : [
            [
              key,
              {
                ...view,
                zoomed:
                  view.zoomed === null
                    ? null
                    : resolveReference(panelRefs, view.zoomed, activeOwner),
              },
            ],
          ];
    }),
  );
  const mergedLayouts = Object.fromEntries(
    Object.entries(saved.mergedLayouts || {}).flatMap(([key, layout]) => {
      const owners = workspaceRefs.get(key);
      return owners?.length
        ? owners.map((owner) => [
            owner.id,
            mapLayout(layout, (id) =>
              resolveReference(panelRefs, id, owner.owner),
            ),
          ])
        : [[key, migrateMergedLayout(layout, panelRefs, activeOwner)]];
    }),
  );
  return {
    ...saved,
    workspaces,
    closedProjects,
    activeId,
    selected:
      saved.selected === undefined
        ? undefined
        : resolveReference(panelRefs, saved.selected, activeOwner),
    zoomed:
      saved.zoomed == null
        ? saved.zoomed
        : resolveReference(panelRefs, saved.zoomed, activeOwner),
    chatFocus:
      saved.chatFocus === undefined
        ? undefined
        : resolveReference(panelRefs, saved.chatFocus, activeOwner),
    views,
    mergedLayouts,
  };
}
