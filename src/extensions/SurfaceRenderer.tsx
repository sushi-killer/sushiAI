import { Check, Circle, ListTodo, Plus, Trash2 } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties } from "react";
import { uid } from "../layout.ts";
import {
  bucketOf,
  compareRecords,
  display,
  matches,
  orderBuckets,
  seedForFilter,
  text,
  toneOf,
  truthy,
  type Item,
} from "./records.ts";
import { projectScope } from "./routes.ts";
import type { ExtensionPanel, Workspace } from "../types.ts";
import type {
  DeclarativeDocument,
  DesignTokens,
  RecordField,
  SurfaceContribution,
} from "./types.ts";
import type { ExtensionRegistry } from "./registry.ts";

/** Fields the host fills in rather than the extension storing them. They read
 * like any other field and can never be written. */
const PSEUDO: Record<string, RecordField> = {
  $project: { id: "$project", type: "text", label: "Project" },
};

export function ExtensionSurface({
  panel,
  cwd,
  connection,
  registry,
}: {
  panel: ExtensionPanel;
  cwd: string;
  connection?: string;
  registry: ExtensionRegistry;
}) {
  const surface = registry.resolveSurface(panel);
  if (!surface || !registry.isExtensionActive(panel.extension.extensionId))
    return (
      <UnavailableExtensionSurface extensionId={panel.extension.extensionId} />
    );
  return (
    <ExtensionSurfaceView
      surface={surface}
      cwd={cwd}
      connection={connection}
      instanceId={panel.extension.instanceId}
      frame="pane"
    />
  );
}

/** Where the surface is being shown. The host draws the frame - a page
 * heading, a panel header, or an embedded card - and the surface draws only
 * what goes inside it, so one definition renders correctly in all three. */
export type SurfaceFrame = "page" | "pane" | "section";

export function ExtensionSurfaceView({
  surface,
  cwd,
  connection,
  workspaces = [],
  instanceId = "page",
  frame = "section",
}: {
  surface: SurfaceContribution;
  cwd: string;
  connection?: string;
  /** Only read by an aggregate surface, to name the projects it found. */
  workspaces?: Workspace[];
  instanceId?: string;
  frame?: SurfaceFrame;
}) {
  if (surface.view.kind !== "declarative")
    return <UnavailableExtensionSurface extensionId={surface.extensionId} />;
  // An aggregate reads every project at once, so it has no scope of its own.
  const projects = surface.aggregate
    ? new Map(
        workspaces.map((workspace) => [
          projectScope(workspace.cwd, workspace.connection),
          workspace.name,
        ]),
      )
    : undefined;
  const scope = projects
    ? ""
    : surface.stateScope === "project"
      ? projectScope(cwd, connection)
      : surface.stateScope === "global"
        ? "global"
        : instanceId;
  // A project-scoped surface with no project has nowhere to save; it renders
  // read-only rather than inventing a shared "no project" bucket.
  if (!scope && !projects) return <UnscopedExtensionSurface />;
  return (
    <DeclarativeCollection
      key={scope}
      extensionId={surface.extensionId}
      surfaceId={surface.stateId}
      version={surface.stateVersion}
      scope={scope}
      projects={projects}
      document={surface.view.document}
      tokens={surface.tokens}
      frame={frame}
    />
  );
}

function UnscopedExtensionSurface() {
  return (
    <div className="extension-unavailable">
      <ListTodo size={24} />
      <strong>No project yet</strong>
      <p>
        This surface saves its contents per project. Open a workspace with a
        folder to start using it.
      </p>
    </div>
  );
}

function UnavailableExtensionSurface({ extensionId }: { extensionId: string }) {
  return (
    <div className="extension-unavailable">
      <ListTodo size={24} />
      <strong>Extension unavailable</strong>
      <p>
        <code>{extensionId}</code> is disabled or no longer installed. Close
        this panel or enable the extension again from Extensions.
      </p>
    </div>
  );
}

function DeclarativeCollection({
  extensionId,
  surfaceId,
  version,
  scope,
  projects,
  document,
  tokens,
  frame,
}: {
  extensionId: string;
  surfaceId: string;
  version: number;
  scope: string;
  projects?: Map<string, string>;
  document: DeclarativeDocument;
  tokens?: DesignTokens;
  frame: SurfaceFrame;
}) {
  const [items, setItems] = useState<Item[] | null>(null);
  const [readOnly, setReadOnly] = useState("");
  const [draft, setDraft] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const pending = useRef<Item[] | null>(null);
  // Loading is not editing. Without this, the value just read is written
  // straight back, that write is announced, the announcement is read, and the
  // surface saves itself forever.
  const dirty = useRef(false);

  const edit = (next: (current: Item[]) => Item[]) => {
    if (readOnly) return;
    dirty.current = true;
    setItems((current) => next(current || []));
  };

  // Renaming a workspace renames the heading it groups under.
  const projectNames = projects ? [...projects].join("\u0001") : "";

  useEffect(() => {
    let cancelled = false;
    dirty.current = false;
    if (projects) {
      readAggregate(extensionId, surfaceId, version, projects).then(
        (rows) => {
          if (cancelled) return;
          setReadOnly("");
          setItems(rows);
        },
        (error) => {
          if (cancelled) return;
          setReadOnly(String(error?.message || error));
          setItems([]);
        },
      );
      return () => {
        cancelled = true;
      };
    }
    readState(extensionId, surfaceId, version, scope).then((stored) => {
      if (cancelled) return;
      const clean = sanitize(stored);
      if (stored !== null && clean === null) {
        // Something is on disk that this surface cannot read. Show it rather
        // than quietly replacing it with an empty list.
        setReadOnly(
          "Saved contents could not be read, so this surface is read-only.",
        );
        setItems([]);
        return;
      }
      setReadOnly("");
      setItems(clean ?? (document.seed as Item[]));
    });
    return () => {
      cancelled = true;
    };
    // projects is rebuilt every render; projectNames is what actually changes.
  }, [extensionId, surfaceId, version, scope, document.seed, projectNames]);

  // Another view of the same slice saved: take its version, unless this one
  // has an edit of its own still in flight.
  useEffect(() => {
    if (!window.bridge?.onExtensionState) return;
    return window.bridge.onExtensionState((change) => {
      if (
        change.extensionId !== extensionId ||
        change.surfaceId !== surfaceId ||
        change.version !== version
      )
        return;
      // An aggregate cares about every project, not one scope.
      if (projects) {
        void readAggregate(extensionId, surfaceId, version, projects).then(
          setItems,
        );
        return;
      }
      if (change.scope !== scope || pending.current) return;
      void readState(extensionId, surfaceId, version, scope).then((stored) => {
        if (pending.current) return;
        dirty.current = false;
        setItems(sanitize(stored) ?? []);
      });
    });
  }, [extensionId, surfaceId, version, scope, projectNames]);

  useEffect(() => {
    if (items === null || !dirty.current) return;
    const save = items.slice(0, 1000);
    pending.current = save;
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      pending.current = null;
      void writeState(extensionId, surfaceId, version, scope, save).then(
        (error) => error && setReadOnly(error),
      );
    }, 250);
    // Unmounting must flush, not discard: switching workspace re-keys this
    // component, and a dropped timer would silently lose the last edit.
    return () => {
      clearTimeout(timer.current);
      if (pending.current) {
        const flush = pending.current;
        pending.current = null;
        void writeState(extensionId, surfaceId, version, scope, flush);
      }
    };
  }, [items, extensionId, surfaceId, version, scope]);

  const view = document.views[0];
  const byId = new Map(document.fields.map((field) => [field.id, field]));
  const groups = view.groupable || [];
  const [group, setGroup] = useState(view.defaultGroup || "");
  const [hidden, setHidden] = useState<string[]>([]);

  const canEdit = !readOnly;
  const set = (id: string, key: string, value: unknown) =>
    edit((current) =>
      current.map((candidate) =>
        candidate.id === id ? { ...candidate, [key]: value } : candidate,
      ),
    );

  const addItem = () => {
    const label = draft.trim();
    if (!label || !document.allowAdd) return;
    const seed = seedForFilter(view.filter, view.primary);
    edit((current) => [
      ...current,
      { ...seed, id: uid(), [view.primary]: label },
    ]);
    setDraft("");
  };

  const remove = (id: string) =>
    edit((current) => current.filter((candidate) => candidate.id !== id));

  const all = items || [];
  // A tile counts the slice; the rows below it are what the view and the chips
  // leave. They are meant to differ: a summary that changed every time someone
  // picked a filter could not tell you what you had filtered away.
  const shown = all
    .filter((item) => (view.filter || []).every((rule) => matches(item, rule)))
    .filter((item) => !hidden.some((key) => truthy(item[key])));
  const ordered = view.sort
    ? [...shown].sort(compareRecords(view.sort, byId))
    : shown;

  // A board pivots over one closed set, so every column of that set is drawn
  // whether or not anything is in it. Anything else buckets by the data.
  const columnField = view.columns ? byId.get(view.columns) : undefined;
  const pivot = view.layout === "board" && columnField ? columnField.id : group;
  const rows = ordered.map((item) => ({
    item,
    bucket: pivot ? bucketOf(item, byId.get(pivot), pivot) : "",
  }));
  // Every declared column, empty or not, so the board keeps its shape. "None"
  // is not declared, so it appears only when something is actually in it.
  const buckets = columnField
    ? [
        ...(columnField.options || []).map((option) => option.label),
        ...(rows.some((row) => row.bucket === "None") ? ["None"] : []),
      ]
    : pivot
      ? [...new Set(rows.map((row) => row.bucket))].sort(
          orderBuckets(byId.get(pivot)),
        )
      : [""];
  const count = (bucket: string) =>
    rows.filter((row) => row.bucket === bucket).length;

  return (
    <div
      className={`extension-surface extension-surface-${frame}`}
      {...(tokens?.accent ? { "data-accent": tokens.accent } : {})}
      {...(tokens?.density ? { "data-density": tokens.density } : {})}
      {...(tokens?.radius ? { "data-radius": tokens.radius } : {})}
      {...(tokens?.elevation ? { "data-elevation": tokens.elevation } : {})}
    >
      {frame === "section" && (
        <div className="extension-surface-heading">
          <h2>{document.title}</h2>
          {document.description && <p>{document.description}</p>}
        </div>
      )}
      {view.summary && (
        <div className="extension-summary">
          {view.summary.map((tile) => (
            <div key={tile.label} data-tone={tile.tone}>
              <strong>
                {
                  all.filter((item) => matches(item, { ...tile, op: "eq" }))
                    .length
                }
              </strong>
              <span>{tile.label}</span>
            </div>
          ))}
        </div>
      )}
      {(groups.length > 0 || view.filterable) && (
        <div className="extension-surface-controls">
          {groups.length > 0 && (
            <div
              className="extension-surface-groups"
              role="group"
              aria-label="Group by"
            >
              <button
                className={group ? "" : "selected"}
                aria-pressed={!group}
                onClick={() => setGroup("")}
              >
                All
              </button>
              {groups.map((candidate) => (
                <button
                  key={candidate}
                  className={group === candidate ? "selected" : ""}
                  aria-pressed={group === candidate}
                  onClick={() => setGroup(candidate)}
                >
                  {byId.get(candidate)?.label || "Project"}
                </button>
              ))}
            </div>
          )}
          {view.filterable && (
            <div
              className="extension-surface-groups"
              role="group"
              aria-label="Hide"
            >
              {view.filterable.map((key) => (
                <button
                  key={key}
                  className={hidden.includes(key) ? "selected" : ""}
                  aria-pressed={hidden.includes(key)}
                  onClick={() =>
                    setHidden((current) =>
                      current.includes(key)
                        ? current.filter((entry) => entry !== key)
                        : [...current, key],
                    )
                  }
                >
                  Hide {(byId.get(key)?.label || key).toLowerCase()}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
      <div
        className={`extension-surface-list extension-layout-${view.layout}`}
        style={{ "--meta-columns": view.meta.length } as CSSProperties}
      >
        {buckets.map((bucket) => (
          <section className="extension-group" key={bucket || "all"}>
            {pivot && (
              <h3>
                {bucket || "None"}
                <span className="extension-group-count">{count(bucket)}</span>
              </h3>
            )}
            {rows
              .filter((row) => row.bucket === bucket)
              .map(({ item }) => (
                <div
                  className={`extension-row ${view.toggle && item[view.toggle] ? "done" : ""}`}
                  key={item.id}
                >
                  {view.toggle && document.allowToggle && (
                    <button
                      className="extension-row-toggle"
                      aria-label={`${item[view.toggle] ? "Reopen" : "Complete"} ${text(item[view.primary])}`}
                      aria-pressed={Boolean(item[view.toggle])}
                      disabled={!canEdit}
                      onClick={() =>
                        set(item.id, view.toggle!, !item[view.toggle!])
                      }
                    >
                      {item[view.toggle] ? (
                        <Check size={15} />
                      ) : (
                        <Circle size={15} />
                      )}
                    </button>
                  )}
                  <span className="extension-row-label">
                    {text(item[view.primary])}
                    {view.secondary && text(item[view.secondary]) && (
                      <small>{text(item[view.secondary])}</small>
                    )}
                  </span>
                  {view.meta.map((key) => (
                    <FieldControl
                      key={key}
                      field={byId.get(key) || PSEUDO[key]}
                      value={item[key]}
                      editable={canEdit && (view.editable || []).includes(key)}
                      label={text(item[view.primary])}
                      onChange={(next) => set(item.id, key, next)}
                    />
                  ))}
                  {document.allowRemove && canEdit && (
                    <button
                      className="extension-row-remove"
                      aria-label={`Delete ${text(item[view.primary])}`}
                      onClick={() => remove(item.id)}
                    >
                      <Trash2 size={13} />
                    </button>
                  )}
                </div>
              ))}
          </section>
        ))}
      </div>
      {readOnly && <p className="extension-surface-notice">{readOnly}</p>}
      {document.allowAdd && !readOnly && (
        <div className="extension-surface-compose">
          <input
            aria-label={`New ${document.itemLabel}`}
            placeholder={`Add ${document.itemLabel}…`}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") addItem();
            }}
          />
          <button aria-label={`Add ${document.itemLabel}`} onClick={addItem}>
            <Plus size={15} />
          </button>
        </div>
      )}
    </div>
  );
}

function FieldControl({
  field,
  value,
  editable,
  label,
  onChange,
}: {
  field?: RecordField;
  value: unknown;
  editable: boolean;
  label: string;
  onChange(next: unknown): void;
}) {
  if (!field) return null;
  if (field.type === "select" && editable)
    return (
      <select
        className="extension-field"
        aria-label={`${field.label} for ${label}`}
        value={text(value)}
        onChange={(event) => onChange(event.target.value || undefined)}
      >
        <option value="">{field.label}</option>
        {field.options?.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    );
  if (field.type === "date" && editable)
    return (
      <input
        className="extension-field"
        type="date"
        aria-label={`${field.label} for ${label}`}
        value={text(value)}
        onChange={(event) => onChange(event.target.value || undefined)}
      />
    );
  const shown = display(value, field);
  if (!shown) return null;
  return (
    <em className="extension-chip" data-tone={toneOf(value, field)}>
      {shown}
    </em>
  );
}

const fallbackKey = (
  extensionId: string,
  surfaceId: string,
  version: number,
  scope: string,
) =>
  `sushiai.extension.${extensionId}\u0000${surfaceId}\u0000${version}\u0000${scope}`;

async function readState(
  extensionId: string,
  surfaceId: string,
  version: number,
  scope: string,
): Promise<unknown> {
  if (window.bridge?.extensionsStateRead)
    return window.bridge
      .extensionsStateRead(extensionId, surfaceId, version, scope)
      .catch(() => null);
  try {
    return JSON.parse(
      localStorage.getItem(
        fallbackKey(extensionId, surfaceId, version, scope),
      ) || "null",
    );
  } catch {
    return null;
  }
}

/** Every project's records at once, tagged with the project they came from.
 * Read-only: the manifest validator already refuses add and toggle on an
 * aggregate, so nothing here can write back to the wrong project. */
async function readAggregate(
  extensionId: string,
  surfaceId: string,
  version: number,
  projects: Map<string, string>,
): Promise<Item[]> {
  const read = window.bridge?.extensionsStateAggregate;
  if (!read) return [];
  const slices = await read(extensionId, surfaceId, version);
  return slices.flatMap((slice) =>
    (sanitize(slice.records) || []).map((record) => ({
      ...record,
      id: `${slice.scope}\u0000${record.id}`,
      $project: projects.get(slice.scope) || projectName(slice.scope),
    })),
  );
}

/** A project that is no longer an open workspace still has records; show its
 * folder name rather than hiding them. */
const projectName = (scope: string) =>
  scope.split("\u0000").pop()?.split("/").filter(Boolean).pop() || scope;

async function writeState(
  extensionId: string,
  surfaceId: string,
  version: number,
  scope: string,
  value: Item[],
): Promise<string> {
  if (window.bridge?.extensionsStateWrite)
    return window.bridge
      .extensionsStateWrite(extensionId, surfaceId, version, scope, value)
      .then(
        () => "",
        (error) =>
          // A save that cannot land must say so; silence here reads as "saved".
          String(error?.message || error) || "This surface could not be saved.",
      );
  try {
    localStorage.setItem(
      fallbackKey(extensionId, surfaceId, version, scope),
      JSON.stringify(value),
    );
  } catch {
    // Storage is optional; the surface stays usable without it.
  }
  return "";
}

function sanitize(value: unknown): Item[] | null {
  if (!Array.isArray(value)) return null;
  return value
    .filter(
      (item): item is Item =>
        !!item &&
        typeof item === "object" &&
        !Array.isArray(item) &&
        typeof (item as Item).id === "string",
    )
    .slice(0, 1000);
}
