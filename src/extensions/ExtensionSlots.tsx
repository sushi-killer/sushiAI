import {
  Folder,
  LayoutGrid,
  ListChecks,
  ListTodo,
  Plug,
  Plus,
  Sparkles,
  TerminalSquare,
  Workflow,
} from "lucide-react";
import { ExtensionSurfaceView } from "./SurfaceRenderer.tsx";
import { actionsFor, navigationFor, surfacesFor } from "./routes.ts";
import type { ExtensionRegistry } from "./registry.ts";
import type {
  ExtensionIcon as ExtensionIconValue,
  NavigationPlacement,
  SurfaceHost,
  SurfaceContribution,
  WorkspaceActionPlacement,
} from "./types.ts";
import type { Workspace } from "../types.ts";

// The manifest allows only these icon names, so the map is total.
const ICONS = {
  "list-check": ListChecks,
  "list-todo": ListTodo,
  "layout-grid": LayoutGrid,
  plug: Plug,
  sparkles: Sparkles,
  workflow: Workflow,
  terminal: TerminalSquare,
  folder: Folder,
} as const;

/** Named icons come from the bundled set; an extension that needs its own
 * ships path data and the host builds the element around it. */
export function ExtensionIcon({
  icon,
  size = 14,
}: {
  icon?: ExtensionIconValue;
  size?: number;
}) {
  if (icon?.kind === "svg")
    return (
      <svg
        width={size}
        height={size}
        viewBox={icon.viewBox}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.8}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        {icon.paths.map((path, index) => (
          <path
            key={index}
            d={path.d}
            fill={path.fill}
            {...(path.strokeWidth ? { strokeWidth: path.strokeWidth } : {})}
          />
        ))}
      </svg>
    );
  const Glyph =
    (icon?.kind === "named" && ICONS[icon.name as keyof typeof ICONS]) ||
    ListTodo;
  return <Glyph size={size} />;
}

/** Navigation entries an extension contributed to one placement. Renders
 * nothing when no active extension asked for that spot.
 *
 * The sidebar does not use this: there its entries are merged into one ordered
 * list with the built-in sections, so a contributed page is a peer of Skills
 * rather than a group bolted on at one end. */
export function ExtensionNavSlot({
  registry,
  placement,
  className = "extension-nav-item",
  onOpen,
}: {
  registry: ExtensionRegistry;
  placement: NavigationPlacement;
  className?: string;
  onOpen(extensionId: string, targetSurfaceId: string): void;
}) {
  const items = navigationFor(registry, placement);
  if (!items.length) return null;
  return (
    <>
      {items.map((item) => (
        <button
          key={`${item.extensionId}:${item.id}`}
          className={className}
          title={item.label}
          onClick={() => onOpen(item.extensionId, item.targetSurfaceId)}
        >
          <ExtensionIcon icon={item.icon} />
          <span>{item.label}</span>
        </button>
      ))}
    </>
  );
}

/** Entries an extension asked to show in the add-panel list. They render as
 * ordinary panel cards, so a pane an extension provides is picked the same way
 * a terminal is. */
export function ExtensionPanelOptions({
  registry,
  onAdd,
}: {
  registry: ExtensionRegistry;
  onAdd(extensionId: string, targetSurfaceId: string): void;
}) {
  const items = navigationFor(registry, "panel.picker");
  const describe = (extensionId: string, surfaceId: string) =>
    registry
      .availableSurfaces()
      .find(
        (surface) =>
          surface.extensionId === extensionId && surface.id === surfaceId,
      )?.description;
  return (
    <>
      {items.map((item) => (
        <button
          key={`${item.extensionId}:${item.id}`}
          onClick={() => onAdd(item.extensionId, item.targetSurfaceId)}
        >
          <ExtensionIcon icon={item.icon} size={19} />
          <div>
            <strong>{item.label}</strong>
            <small>
              {describe(item.extensionId, item.targetSurfaceId) ||
                "From an extension"}
            </small>
          </div>
          <Plus size={15} />
        </button>
      ))}
    </>
  );
}

/** The commands a surface asked to put on its own page. They land in the page
 * heading, the one place the design code puts page-level controls, so a
 * contributed page cannot invent a control row of its own. */
export function ExtensionPageActions({
  registry,
  surface,
  onRun,
}: {
  registry: ExtensionRegistry;
  surface: SurfaceContribution;
  onRun(extensionId: string, commandId: string): void;
}) {
  if (surface.view.kind !== "declarative") return null;
  const wanted = surface.view.document.views[0]?.actions || [];
  const commands = registry
    .snapshotCommands()
    .filter(
      (command) =>
        command.extensionId === surface.extensionId &&
        wanted.includes(command.id),
    );
  return (
    <>
      {commands.map((command) => (
        <button
          key={command.id}
          className="secondary"
          onClick={() => onRun(surface.extensionId, command.id)}
        >
          {command.title}
        </button>
      ))}
    </>
  );
}

/** Workspace actions an extension contributed to one placement. */
export function ExtensionActionSlot({
  registry,
  placement,
  className = "icon-button extension-action",
  onRun,
}: {
  registry: ExtensionRegistry;
  placement: WorkspaceActionPlacement;
  className?: string;
  onRun(extensionId: string, commandId: string): void;
}) {
  const items = actionsFor(registry, placement);
  if (!items.length) return null;
  return (
    <>
      {items.map((action) => (
        <button
          key={`${action.extensionId}:${action.id}`}
          className={className}
          title={action.label}
          aria-label={action.label}
          onClick={() => onRun(action.extensionId, action.commandId)}
        >
          <ExtensionIcon icon={action.icon} />
        </button>
      ))}
    </>
  );
}

/** Surfaces an extension asked to embed into a host page, rendered through the
 * same host-owned safe renderer the workspace panes use. */
export function ExtensionSectionSlot({
  registry,
  host,
  cwd,
  connection,
  workspaces,
}: {
  registry: ExtensionRegistry;
  host: SurfaceHost;
  cwd: string;
  connection?: string;
  workspaces?: Workspace[];
}) {
  const surfaces = surfacesFor(registry, host);
  if (!surfaces.length) return null;
  return (
    <div className="extension-sections">
      {surfaces.map((surface) => (
        <section
          className="extension-section"
          key={`${surface.extensionId}:${surface.id}`}
        >
          <ExtensionSurfaceView
            surface={surface}
            cwd={cwd}
            connection={connection}
            workspaces={workspaces}
            instanceId={`section.${host}`}
          />
        </section>
      ))}
    </div>
  );
}
