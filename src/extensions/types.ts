import type { ExtensionPanel } from "../types.ts";

export const EXTENSION_API_VERSION = 1;

export type ExtensionSource =
  | { kind: "builtin" }
  | { kind: "local"; path: string }
  | { kind: "npm"; package: string; version: string; integrity?: string }
  | {
      kind: "git";
      url: string;
      requestedRef: string;
      resolvedCommit: string;
    };

export type ExtensionIcon =
  | { kind: "named"; name: string }
  | {
      kind: "svg";
      viewBox: string;
      paths: {
        d: string;
        fill: "none" | "currentColor";
        strokeWidth?: number;
      }[];
    };

export type SurfaceHost =
  | "app.page"
  | "dashboard.section"
  | "sessions.section"
  | "skills.section"
  | "workspace.pane"
  | "workspace.tab"
  | "settings.section";

export type NavigationPlacement =
  | "mode.primary"
  | "sidebar.primary"
  | "dashboard.navigation"
  | "sessions.navigation"
  | "skills.navigation"
  | "panel.picker";

export type WorkspaceActionPlacement =
  | "workspace.toolbar.start"
  | "workspace.toolbar.before-tidy"
  | "workspace.toolbar.after-tidy"
  | "workspace.toolbar.end"
  | "workspace.folder.actions";

export type DesignTokens = {
  accent?: "sage" | "blue" | "amber" | "violet";
  density?: "comfortable" | "compact";
  radius?: "sm" | "md";
  elevation?: "flat" | "raised";
};

export type FieldType = "text" | "boolean" | "date" | "select";

/** What a value means, so the app can colour it. Never a colour itself. */
export type Tone = "neutral" | "info" | "warning" | "danger" | "muted" | "ok";

export type RecordField = {
  id: string;
  type: FieldType;
  label: string;
  options?: { value: string; label: string; tone: Tone }[];
};

export type RecordView = {
  kind: "records";
  layout: "list" | "board" | "table";
  primary: string;
  secondary?: string;
  toggle?: string;
  meta: string[];
  /** Of the shown fields, the ones this view may also change. */
  editable?: string[];
  /** Always applied, invisible to the reader. */
  filter?: RecordFilter[];
  /** Offered as a chip the host draws and remembers. */
  filterable?: string[];
  sort?: { field: string; dir: "asc" | "desc" }[];
  summary?: { label: string; field: string; value: unknown; tone: Tone }[];
  actions?: string[];
  /** A select field whose options are the board's columns, empty ones kept. */
  columns?: string;
  groupable?: string[];
  defaultGroup?: string;
};

export type DeclarativeDocument = {
  title: string;
  description?: string;
  itemLabel: string;
  allowAdd: boolean;
  allowToggle: boolean;
  allowRemove: boolean;
  fields: RecordField[];
  views: RecordView[];
  /** Rows a fresh slice starts with, from a v1 `collection` document. */
  seed: Record<string, unknown>[];
};

export type ViewDescriptor =
  | { kind: "core"; viewId: string }
  | {
      kind: "declarative";
      schemaVersion: 2;
      document: DeclarativeDocument;
    };

export type SurfaceContribution = {
  id: string;
  extensionId: string;
  title: string;
  description?: string;
  icon?: ExtensionIcon;
  allowedHosts: SurfaceHost[];
  defaultHost: SurfaceHost;
  instancePolicy: "singleton" | "multiple";
  /** The slice this surface reads and writes - its own id unless it borrows
   * another surface's, which is how one list gets two views. */
  stateId: string;
  stateVersion: number;
  /** Which slice of state an instance of this surface reads and writes. */
  stateScope: "instance" | "project" | "global";
  /** Reads every project's slice of this surface at once, read-only. */
  aggregate?: boolean;
  tokens?: DesignTokens;
  view: ViewDescriptor;
};

export type RecordFilter = {
  field: string;
  op: "eq" | "ne";
  value: unknown;
};

export type NavigationContribution = {
  id: string;
  extensionId: string;
  targetSurfaceId: string;
  allowedPlacements: NavigationPlacement[];
  defaultPlacement: NavigationPlacement;
  label: string;
  icon: ExtensionIcon;
  order: number;
};

export type WorkspaceActionContribution = {
  id: string;
  extensionId: string;
  commandId: string;
  allowedPlacements: WorkspaceActionPlacement[];
  defaultPlacement: WorkspaceActionPlacement;
  label: string;
  icon: ExtensionIcon;
  order: number;
};

export type CommandContribution = {
  id: string;
  extensionId: string;
  title: string;
  surfaceId: string;
};

export type ExtensionManifest = {
  id: string;
  name: string;
  version: string;
  apiVersion: number;
  source: ExtensionSource;
  scope: "app";
  description?: string;
  contributions: {
    surfaces: SurfaceContribution[];
    navigation: NavigationContribution[];
    actions: WorkspaceActionContribution[];
    commands: CommandContribution[];
  };
};

export type ExtensionStatus = "active" | "disabled";

export type ExtensionRecord = {
  manifest: ExtensionManifest;
  status: ExtensionStatus;
  error?: string;
};

export type ExtensionProblem = {
  folder: string;
  path: string;
  error: string;
};

export type ExtensionSnapshot = {
  schemaVersion: 2;
  version: number;
  diagnostic?: string;
  /** Absent outside the desktop app, where there is no folder to read. */
  localDir?: string;
  /** Folders that could not be loaded, one entry each. */
  problems: ExtensionProblem[];
  extensions: ExtensionRecord[];
  surfaces: SurfaceContribution[];
  navigation: NavigationContribution[];
  actions: WorkspaceActionContribution[];
  commands: CommandContribution[];
};

export type ExtensionPanelProps = {
  panel: ExtensionPanel;
  cwd: string;
};
