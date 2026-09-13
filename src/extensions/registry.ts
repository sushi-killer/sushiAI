import type { ExtensionPanel } from "../types.ts";
import type {
  CommandContribution,
  NavigationContribution,
  ExtensionSnapshot,
  SurfaceContribution,
  WorkspaceActionContribution,
} from "./types.ts";

const emptySnapshot = (): ExtensionSnapshot => ({
  schemaVersion: 2,
  version: 0,
  problems: [],
  extensions: [],
  surfaces: [],
  navigation: [],
  actions: [],
  commands: [],
});

function unique<T extends { id: string }>(items: T[], kind: string): T[] {
  const ids = new Set<string>();
  for (const item of items) {
    const key = `${(item as T & { extensionId?: string }).extensionId || "core"}:${item.id}`;
    if (ids.has(key))
      throw new Error(`Duplicate ${kind} contribution: ${key}.`);
    ids.add(key);
  }
  return items;
}

export class ExtensionRegistry {
  private current = emptySnapshot();
  private readonly listeners = new Set<() => void>();

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  snapshot(): ExtensionSnapshot {
    return this.current;
  }

  applySnapshot(snapshot: ExtensionSnapshot): void {
    if (snapshot.schemaVersion !== 2)
      throw new Error("Unsupported extension snapshot schema.");
    const extensionIds = new Set(
      snapshot.extensions.map((extension) => extension.manifest.id),
    );
    const surfaces = unique(snapshot.surfaces, "surface");
    const navigation = unique(snapshot.navigation, "navigation");
    const actions = unique(snapshot.actions, "action");
    const commands = unique(snapshot.commands, "command");
    for (const item of [...surfaces, ...navigation, ...actions, ...commands]) {
      if (!extensionIds.has(item.extensionId))
        throw new Error(
          `Contribution belongs to an unknown extension: ${item.extensionId}.`,
        );
    }
    this.current = deepFreeze({
      ...snapshot,
      extensions: [...snapshot.extensions],
      problems: [...(snapshot.problems || [])],
      surfaces: [...surfaces],
      navigation: [...navigation].sort(compareOrder),
      actions: [...actions].sort(compareOrder),
      commands: [...commands],
    }) as ExtensionSnapshot;
    for (const listener of this.listeners) listener();
  }

  isExtensionActive(extensionId: string): boolean {
    return this.current.extensions.some(
      (extension) =>
        extension.manifest.id === extensionId && extension.status === "active",
    );
  }

  availableSurfaces(): SurfaceContribution[] {
    return this.current.surfaces.filter((surface) =>
      this.isExtensionActive(surface.extensionId),
    );
  }

  availableNavigation(): NavigationContribution[] {
    return this.current.navigation.filter((item) =>
      this.isExtensionActive(item.extensionId),
    );
  }

  availableActions(): WorkspaceActionContribution[] {
    return this.current.actions.filter((item) =>
      this.isExtensionActive(item.extensionId),
    );
  }

  snapshotCommands(): CommandContribution[] {
    return this.current.commands.filter((item) =>
      this.isExtensionActive(item.extensionId),
    );
  }

  resolveSurface(panel: ExtensionPanel): SurfaceContribution | undefined {
    return this.current.surfaces.find(
      (surface) =>
        surface.extensionId === panel.extension.extensionId &&
        surface.id === panel.extension.contributionId,
    );
  }

  createPanel(
    extensionId: string,
    surfaceId: string,
    id: string,
  ): ExtensionPanel {
    const surface = this.availableSurfaces().find(
      (item) => item.extensionId === extensionId && item.id === surfaceId,
    );
    if (!surface)
      throw new Error(
        `Unavailable extension surface: ${extensionId}/${surfaceId}.`,
      );
    if (
      !surface.allowedHosts.some(
        (host) => host === "workspace.pane" || host === "workspace.tab",
      )
    )
      throw new Error(`Surface ${surfaceId} is not a workspace surface.`);
    return {
      id,
      kind: "extension",
      title: surface.title,
      extension: {
        extensionId,
        contributionId: surface.id,
        instanceId: id,
        stateVersion: surface.stateVersion,
      },
    };
  }
}

function compareOrder(
  a: { order: number; extensionId: string; id: string },
  b: typeof a,
) {
  return (
    a.order - b.order ||
    a.extensionId.localeCompare(b.extensionId) ||
    a.id.localeCompare(b.id)
  );
}

function deepFreeze<T>(value: T): T {
  if (!value || typeof value !== "object" || Object.isFrozen(value))
    return value;
  Object.freeze(value);
  for (const child of Object.values(value as Record<string, unknown>))
    deepFreeze(child);
  return value;
}
