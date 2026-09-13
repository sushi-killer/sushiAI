const path = require("node:path");
const ID = /^[a-z0-9][a-z0-9._-]*$/;
const VERSION = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const COMMIT = /^[0-9a-f]{7,64}$/i;
/** Where a surface may be shown. Only two entries are consulted at runtime:
 * "workspace.pane" makes a surface openable as a pane, and "app.page" makes it
 * reachable as a page. The rest declare intent - a surface is mounted by its
 * defaultHost, and nothing yet lets a user move it. */
const HOSTS = new Set([
  "app.page",
  "dashboard.section",
  "sessions.section",
  "skills.section",
  "workspace.pane",
  "workspace.tab",
  "settings.section",
]);
/** Where an entry may appear. Only defaultPlacement is read; the list is the
 * author saying which spots they designed for. */
const PLACEMENTS = new Set([
  "mode.primary",
  "sidebar.primary",
  "dashboard.navigation",
  "sessions.navigation",
  "skills.navigation",
  // An entry in the add-panel list, beside Terminal and Thread. The extension
  // asks for the spot; it is not granted by owning a workspace.pane surface.
  "panel.picker",
]);
// Where a surface's state belongs. The extension decides: a task list is
// per project, a scratchpad is per pane, a preference is one per machine.
const INSTANCE_POLICIES = new Set(["singleton", "multiple"]);
const STATE_SCOPES = new Set(["instance", "project", "global"]);
// Named spots rather than numbers: core can add or move its own buttons
// without renumbering a contract extensions depend on.
const ACTION_PLACEMENTS = new Set([
  "workspace.toolbar.start",
  "workspace.toolbar.before-tidy",
  "workspace.toolbar.after-tidy",
  "workspace.toolbar.end",
  "workspace.folder.actions",
]);
const ICONS = new Set([
  "list-check",
  "list-todo",
  "layout-grid",
  "plug",
  "sparkles",
  "workflow",
  "terminal",
  "folder",
]);

// Caps, not opinions: an unbounded title breaks the panel header and the
// picker card long before it looks like an attack.
const LIMITS = {
  name: 80,
  title: 80,
  label: 40,
  description: 300,
  default: 200,
};

function requiredString(value, field) {
  if (typeof value !== "string" || !value.trim())
    throw new Error(`Extension manifest field ${field} is required.`);
  const result = value.trim();
  const cap = LIMITS[field.split(".").pop()] ?? LIMITS.default;
  if (result.length > cap)
    throw new Error(`Extension manifest field ${field} is longer than ${cap}.`);
  return result;
}

const order = (value) =>
  Number.isInteger(value) ? Math.min(Math.max(value, -1000), 1000) : 0;

const PATH_DATA = /^[MmLlHhVvCcSsQqTtAaZz0-9eE,.\s+-]+$/;
const MAX_PATHS = 12;
const MAX_PATH_CHARS = 4096;

/** An icon is either one of the bundled names or geometry the extension ships
 * itself. Only path data survives: the renderer builds the element, so nothing
 * here can carry script, styles or a remote reference. */
function icon(value, field) {
  if (typeof value === "string") {
    if (!ICONS.has(value))
      throw new Error(
        `${field}: ${JSON.stringify(value)} is not a bundled icon. Use one of: ${[...ICONS].join(", ")}, or supply {"kind":"svg","viewBox":…,"paths":[…]}.`,
      );
    return { kind: "named", name: value };
  }
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(`Extension ${field} must be a name or an svg object.`);
  if (value.kind !== "svg") throw new Error(`Unsupported extension ${field}.`);
  const parts = requiredString(value.viewBox, `${field}.viewBox`)
    .trim()
    .split(/\s+/);
  if (parts.length !== 4 || parts.some((part) => !Number.isFinite(+part)))
    throw new Error(`Extension ${field}.viewBox must be four numbers.`);
  const source = Array.isArray(value.paths) ? value.paths : [];
  if (!source.length || source.length > MAX_PATHS)
    throw new Error(`Extension ${field} must declare 1-${MAX_PATHS} paths.`);
  return {
    kind: "svg",
    viewBox: parts.join(" "),
    paths: source.map((entry) => {
      // Path data has its own, larger cap; the generic one is for prose.
      const raw = typeof entry === "string" ? entry : entry?.d;
      if (typeof raw !== "string" || !raw.trim())
        throw new Error(`Extension manifest field ${field}.paths is required.`);
      const d = raw.trim();
      if (d.length > MAX_PATH_CHARS || !PATH_DATA.test(d))
        throw new Error(`Extension ${field} accepts plain path data only.`);
      const width = Number(entry?.strokeWidth);
      return {
        d,
        fill: entry?.fill === "currentColor" ? "currentColor" : "none",
        ...(Number.isFinite(width) && width > 0 && width <= 4
          ? { strokeWidth: width }
          : {}),
      };
    }),
  };
}

function id(value, field) {
  const result = requiredString(value, field);
  if (!ID.test(result)) throw new Error(`Invalid extension ${field}.`);
  return result;
}

function list(value, field, allowed) {
  if (!Array.isArray(value) || !value.length)
    throw new Error(
      `${field} must be a non-empty list. Allowed: ${[...allowed].join(", ")}.`,
    );
  const bad = value.find(
    (item) => typeof item !== "string" || !allowed.has(item),
  );
  if (bad !== undefined)
    throw new Error(
      `${field}: ${JSON.stringify(bad)} is not allowed. Use one of: ${[...allowed].join(", ")}.`,
    );
  return [...new Set(value)];
}

/** One of a closed set, or a message naming what was allowed. Silent coercion
 * is worse than a refusal here: "Project" quietly storing per pane loses work
 * in a way nobody can see. */
function choice(value, field, allowed, fallback) {
  if (value === undefined) return fallback;
  if (typeof value !== "string" || !allowed.has(value))
    throw new Error(
      `${field}: ${JSON.stringify(value)} is not allowed. Use one of: ${[...allowed].join(", ")}.`,
    );
  return value;
}

function validateExtensionSource(source) {
  if (!source || typeof source !== "object")
    throw new Error("Extension source must be an object.");
  const kind = requiredString(source.kind, "source.kind");
  if (kind === "builtin") return { kind };
  if (kind === "local") {
    // The path is stamped by the scanner, never read from the manifest on
    // disk: a folder must not be able to claim where it came from.
    const location = requiredString(source.path, "source.path");
    if (!path.isAbsolute(location))
      throw new Error("Local extension path must be absolute.");
    return { kind, path: location };
  }
  if (kind === "npm") {
    const packageName = requiredString(source.package, "source.package");
    const version = requiredString(source.version, "source.version");
    if (!VERSION.test(version))
      throw new Error("npm extension version must be exact semver.");
    return {
      kind,
      package: packageName,
      version,
      ...(typeof source.integrity === "string"
        ? { integrity: source.integrity }
        : {}),
    };
  }
  if (kind === "git") {
    const url = requiredString(source.url, "source.url");
    const requestedRef = requiredString(
      source.requestedRef,
      "source.requestedRef",
    );
    const resolvedCommit = requiredString(
      source.resolvedCommit,
      "source.resolvedCommit",
    );
    if (!COMMIT.test(resolvedCommit))
      throw new Error("Git extension must pin a resolved commit.");
    if (/^(main|master|develop|HEAD)$/i.test(requestedRef))
      throw new Error(
        "Git extension refs must use a tag or commit, not a branch.",
      );
    return { kind, url, requestedRef, resolvedCommit };
  }
  throw new Error(`Unsupported extension source: ${kind}.`);
}

const FIELD_TYPES = new Set(["text", "boolean", "date", "select"]);
const LAYOUTS = new Set(["list", "board", "table"]);
// What a value means, never what colour it is. The app owns the palette.
const TONES = new Set(["neutral", "info", "warning", "danger", "muted", "ok"]);
const FILTER_OPS = new Set(["eq", "ne"]);
const SORT_DIRS = new Set(["asc", "desc"]);
const MAX_SUMMARY = 6;
const MAX_ACTIONS = 3;
const MAX_FIELDS = 24;
const MAX_OPTIONS = 32;
// The renderer draws views[0] and nothing else. Accepting four and showing one
// is a promise the app does not keep, so the contract stops making it.
const MAX_VIEWS = 1;
// Group buckets the host computes rather than the extension storing them.
// Readable in "meta" and "groupable" only: "sort" and "filterable" compare
// stored values, and a computed bucket has none.
const PSEUDO_FIELDS = new Set(["$project"]);

function validateField(input, at) {
  const fieldId = id(input?.id, `${at}.id`);
  if (fieldId.startsWith("$"))
    throw new Error(`${at}.id may not start with "$"; those names are ours.`);
  const type = choice(input.type, `${at}.type`, FIELD_TYPES, "text");
  const field = {
    id: fieldId,
    type,
    label: requiredString(input.label ?? fieldId, `${at}.label`),
  };
  if (type === "select") {
    const options = Array.isArray(input.options) ? input.options : [];
    if (!options.length || options.length > MAX_OPTIONS)
      throw new Error(`${at}.options must list 1-${MAX_OPTIONS} choices.`);
    field.options = options.map((option, index) => ({
      value: id(option?.value, `${at}.options[${index}].value`),
      label: requiredString(
        option?.label ?? option?.value,
        `${at}.options[${index}].label`,
      ),
      // A closed set of meanings, not a colour: the app decides what "blocked"
      // looks like, so every extension reads the same in the same theme.
      tone: choice(
        option?.tone,
        `${at}.options[${index}].tone`,
        TONES,
        "neutral",
      ),
    }));
  }
  return field;
}

function validateViews(input, fields, at, aggregate) {
  const known = new Map(fields.map((field) => [field.id, field]));
  const views = Array.isArray(input) ? input : [];
  if (!views.length || views.length > MAX_VIEWS)
    throw new Error(`${at} must declare 1-${MAX_VIEWS} views.`);
  const field = (value, where, types) => {
    const found = known.get(id(value, where));
    if (!found)
      throw new Error(
        `${where}: ${JSON.stringify(value)} is not a declared field. Declared: ${[...known.keys()].join(", ")}.`,
      );
    if (types && !types.includes(found.type))
      throw new Error(
        `${where}: ${found.id} is a ${found.type} field; this needs one of ${types.join(", ")}.`,
      );
    return found.id;
  };
  return views.map((view, index) => {
    const where = `${at}[${index}]`;
    const layout = choice(view?.layout, `${where}.layout`, LAYOUTS, "list");
    /** A pseudo-field is host-computed, so it can be read but never written. */
    const readable = (value, spot, types) => {
      if (!PSEUDO_FIELDS.has(value)) return field(value, spot, types);
      if (!aggregate)
        throw new Error(
          `${spot}: ${value} is only available on an aggregate surface.`,
        );
      return value;
    };
    const groupable = (
      Array.isArray(view?.groupable) ? view.groupable : []
    ).map((value, groupIndex) =>
      readable(value, `${where}.groupable[${groupIndex}]`, [
        "select",
        "boolean",
        "date",
      ]),
    );
    const meta = (Array.isArray(view?.meta) ? view.meta : []).map(
      (value, metaIndex) => readable(value, `${where}.meta[${metaIndex}]`),
    );
    // Shown is not the same as editable: an overview displays status without
    // being able to set it, and a pane sets it. Nothing is editable unless the
    // view says so, and only fields it already shows.
    const editable = (Array.isArray(view?.editable) ? view.editable : []).map(
      (value, editIndex) => {
        const spot = `${where}.editable[${editIndex}]`;
        // Only these two have an editor. A text or boolean field named here
        // used to render as a plain chip that quietly ignored the click.
        const target = field(value, spot, ["select", "date"]);
        if (!meta.includes(target))
          throw new Error(
            `${spot}: ${target} is not in this view's "meta", so there is nowhere to edit it.`,
          );
        return target;
      },
    );
    const filter = (Array.isArray(view?.filter) ? view.filter : []).map(
      (rule, ruleIndex) => {
        const spot = `${where}.filter[${ruleIndex}]`;
        return {
          field: field(rule?.field, `${spot}.field`),
          op: choice(rule?.op, `${spot}.op`, FILTER_OPS, "eq"),
          value: rule?.value === undefined ? null : rule.value,
        };
      },
    );
    // The host draws the chip and remembers the pick, exactly as with groups.
    const filterable = (
      Array.isArray(view?.filterable) ? view.filterable : []
    ).map((value, filterIndex) =>
      field(value, `${where}.filterable[${filterIndex}]`, [
        "boolean",
        "select",
      ]),
    );
    const sort = (Array.isArray(view?.sort) ? view.sort : []).map(
      (rule, sortIndex) => {
        const spot = `${where}.sort[${sortIndex}]`;
        return {
          field: field(rule?.field, `${spot}.field`, [
            "date",
            "select",
            "text",
            "boolean",
          ]),
          dir: choice(rule?.dir, `${spot}.dir`, SORT_DIRS, "asc"),
        };
      },
    );
    const summary = (Array.isArray(view?.summary) ? view.summary : []).map(
      (tile, tileIndex) => {
        const spot = `${where}.summary[${tileIndex}]`;
        return {
          label: requiredString(tile?.label, `${spot}.label`),
          field: field(tile?.field, `${spot}.field`),
          value: tile?.value === undefined ? null : tile.value,
          tone: choice(tile?.tone, `${spot}.tone`, TONES, "neutral"),
        };
      },
    );
    if (summary.length > MAX_SUMMARY)
      throw new Error(`${where}.summary lists at most ${MAX_SUMMARY} tiles.`);
    const actions = (Array.isArray(view?.actions) ? view.actions : []).map(
      (value, actionIndex) => id(value, `${where}.actions[${actionIndex}]`),
    );
    if (actions.length > MAX_ACTIONS)
      throw new Error(
        `${where}.actions lists at most ${MAX_ACTIONS} commands.`,
      );
    // A board is a pivot over one closed set of values. Taking the columns
    // from a select means an empty column still shows, instead of the board
    // changing shape as records move.
    const columns = view?.columns
      ? field(view.columns, `${where}.columns`, ["select"])
      : undefined;
    if (layout === "board" && !columns && !groupable.length)
      throw new Error(
        `${where}: a board needs "columns" naming a select field, or something to group by.`,
      );
    if (layout === "table" && !meta.length)
      throw new Error(`${where}: a table needs "meta" for its columns.`);
    return {
      kind: "records",
      layout,
      primary: field(view?.primary, `${where}.primary`, ["text"]),
      ...(view?.secondary
        ? { secondary: field(view.secondary, `${where}.secondary`, ["text"]) }
        : {}),
      ...(view?.toggle
        ? { toggle: field(view.toggle, `${where}.toggle`, ["boolean"]) }
        : {}),
      meta,
      ...(editable.length ? { editable } : {}),
      ...(filter.length ? { filter } : {}),
      ...(filterable.length ? { filterable } : {}),
      ...(sort.length ? { sort } : {}),
      ...(summary.length ? { summary } : {}),
      ...(actions.length ? { actions } : {}),
      ...(columns ? { columns } : {}),
      ...(groupable.length ? { groupable } : {}),
      ...(view?.defaultGroup && groupable.includes(view.defaultGroup)
        ? { defaultGroup: view.defaultGroup }
        : {}),
    };
  });
}

/** The one-field checklist every v1 manifest describes, expressed in the same
 * shape a v2 manifest produces. Keeping the stored field ids identical is what
 * lets an existing list survive the upgrade. */
function expandCollection(document, at) {
  if (
    !document ||
    typeof document !== "object" ||
    document.kind !== "collection"
  )
    throw new Error(
      `${at}.document must be {"kind":"collection", …}; for anything richer use "schemaVersion": 2 with data.fields.`,
    );
  const fields = [
    { id: "label", type: "text", label: document.itemLabel || "Item" },
    { id: "done", type: "boolean", label: "Done" },
  ];
  return {
    title: requiredString(document.title, `${at}.document.title`),
    ...(typeof document.description === "string"
      ? {
          description: requiredString(
            document.description,
            `${at}.document.description`,
          ),
        }
      : {}),
    itemLabel: document.itemLabel
      ? requiredString(document.itemLabel, `${at}.document.itemLabel`)
      : "item",
    allowAdd: document.allowAdd !== false,
    allowToggle: document.allowToggle !== false,
    allowRemove: document.allowRemove === true,
    fields,
    views: [
      {
        kind: "records",
        layout: "list",
        primary: "label",
        toggle: "done",
        meta: [],
      },
    ],
    seed: Array.isArray(document.items)
      ? document.items.map((item, index) => ({
          id: id(item?.id, `${at}.document.items[${index}].id`),
          label: requiredString(
            item?.label,
            `${at}.document.items[${index}].label`,
          ),
          ...(typeof item?.done === "boolean" ? { done: item.done } : {}),
        }))
      : [],
  };
}

function validateView(view, sourceKind, at, aggregate) {
  if (!view || typeof view !== "object")
    throw new Error(`${at}.view is required.`);
  if (view.kind === "core") {
    if (sourceKind !== "builtin")
      throw new Error("Only built-in extensions may request core views.");
    return { kind: "core", viewId: id(view.viewId, "view.viewId") };
  }
  if (view.kind !== "declarative")
    throw new Error(`${at}.view.kind must be "declarative".`);
  if (view.schemaVersion === 1)
    return {
      kind: "declarative",
      schemaVersion: 2,
      document: expandCollection(view.document, `${at}.view`),
    };
  if (view.schemaVersion !== 2)
    throw new Error(
      `${at}.view.schemaVersion must be 1 or 2; this app renders up to 2.`,
    );
  const data = view.data || {};
  const source = Array.isArray(data.fields) ? data.fields : [];
  if (!source.length || source.length > MAX_FIELDS)
    throw new Error(
      `${at}.view.data.fields must declare 1-${MAX_FIELDS} fields.`,
    );
  const fields = source.map((field, index) =>
    validateField(field, `${at}.view.data.fields[${index}]`),
  );
  const seen = new Set();
  for (const field of fields) {
    if (seen.has(field.id))
      throw new Error(`${at}.view.data.fields: ${field.id} is declared twice.`);
    seen.add(field.id);
  }
  const views = validateViews(
    view.views,
    fields,
    `${at}.view.views`,
    aggregate,
  );
  return {
    kind: "declarative",
    schemaVersion: 2,
    document: {
      title: requiredString(view.title ?? "", `${at}.view.title`),
      ...(typeof view.description === "string"
        ? {
            description: requiredString(
              view.description,
              `${at}.view.description`,
            ),
          }
        : {}),
      itemLabel: view.itemLabel
        ? requiredString(view.itemLabel, `${at}.view.itemLabel`)
        : "item",
      allowAdd: !aggregate && view.allowAdd !== false,
      allowToggle: !aggregate && view.allowToggle !== false,
      // Removal is off unless asked for: losing a record is not recoverable,
      // and most embedded views have no business offering it.
      allowRemove: !aggregate && view.allowRemove === true,
      fields,
      views,
      seed: [],
    },
  };
}

const TOKENS = {
  accent: new Set(["sage", "blue", "amber", "violet"]),
  density: new Set(["comfortable", "compact"]),
  radius: new Set(["sm", "md"]),
  elevation: new Set(["flat", "raised"]),
};

function tokens(input) {
  if (input === undefined) return undefined;
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new Error("Extension tokens must be an object.");
  const result = {};
  for (const [key, value] of Object.entries(input)) {
    if (!TOKENS[key]?.has(value))
      throw new Error(`Unsupported extension token: ${key}.`);
    result[key] = value;
  }
  return Object.keys(result).length ? result : undefined;
}

function validateExtensionManifest(input) {
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new Error("Extension manifest must be an object.");
  const extensionId = id(input.id, "id");
  const name = requiredString(input.name, "name");
  const version = requiredString(input.version, "version");
  if (!VERSION.test(version)) throw new Error("Invalid extension version.");
  if (input.apiVersion !== 1)
    throw new Error("Unsupported extension API version.");
  if (input.scope !== undefined && input.scope !== "app")
    throw new Error("Only app-wide extensions are supported.");
  const source = validateExtensionSource(input.source);
  const contributions = input.contributions || {};
  const surfaces = (contributions.surfaces || []).map((surface) => {
    const surfaceId = id(surface?.id, "surface.id");
    const at = `surfaces[${surfaceId}]`;
    const allowedHosts = list(
      surface.allowedHosts,
      `${at}.allowedHosts`,
      HOSTS,
    );
    const defaultHost = requiredString(
      surface.defaultHost,
      `${at}.defaultHost`,
    );
    if (!allowedHosts.includes(defaultHost))
      throw new Error(
        `${at}.defaultHost is ${JSON.stringify(defaultHost)}, which is not in its own allowedHosts.`,
      );
    const instancePolicy = choice(
      surface.instancePolicy,
      `${at}.instancePolicy`,
      INSTANCE_POLICIES,
      "singleton",
    );
    const stateScope = choice(
      surface.stateScope,
      `${at}.stateScope`,
      STATE_SCOPES,
      "instance",
    );
    // Two surfaces can be two views of one list - a pane you edit and a page
    // that reads every project at once. Pointing at the slice by name is what
    // keeps them the same data instead of two lists that look alike.
    const stateId =
      surface.stateId === undefined
        ? surfaceId
        : id(surface.stateId, `${at}.stateId`);
    const stateVersion =
      surface.stateVersion === undefined ? 1 : surface.stateVersion;
    if (
      !Number.isInteger(stateVersion) ||
      stateVersion < 1 ||
      stateVersion > 1e6
    )
      throw new Error(
        `${at}.stateVersion must be a whole number from 1 upwards.`,
      );
    // Several panes sharing one slice of state would race each other, and the
    // loser's edit would vanish silently. Say so at load, not at runtime.
    // An aggregate surface reads every project's slice of itself at once, so
    // it has no single project of its own and cannot be edited in place.
    const aggregate = surface.aggregate === true;
    if (aggregate && stateScope !== "project")
      throw new Error(
        `${at}.aggregate needs "stateScope": "project"; there is nothing to aggregate otherwise.`,
      );
    if (
      aggregate &&
      !allowedHosts.every(
        (host) => host.endsWith(".page") || host.endsWith(".section"),
      )
    )
      throw new Error(
        `${at}.aggregate can only be shown on a page or a section, not in a workspace pane.`,
      );
    if (instancePolicy === "multiple" && stateScope !== "instance")
      throw new Error(
        `${at} keeps ${stateScope} state, so it must be "instancePolicy": "singleton" - several panes sharing one slice would overwrite each other.`,
      );
    return {
      id: surfaceId,
      extensionId,
      title: requiredString(surface.title, `${at}.title`),
      ...(typeof surface.description === "string"
        ? { description: surface.description }
        : {}),
      ...(surface.icon === undefined
        ? {}
        : { icon: icon(surface.icon, "surface.icon") }),
      allowedHosts,
      defaultHost,
      instancePolicy,
      stateId,
      stateVersion,
      stateScope,
      ...(tokens(surface.tokens) ? { tokens: tokens(surface.tokens) } : {}),
      ...(aggregate ? { aggregate } : {}),
      view: validateView(surface.view, source.kind, at, aggregate),
    };
  });
  // A borrowed slice has to be a real one, the same shape of storage, and
  // read-only on the borrowing side - two editable views of one slice each
  // write their whole list, so the slower one's edit disappears without a
  // word. That is the race "instancePolicy": "singleton" exists to prevent.
  const byId = new Map(surfaces.map((surface) => [surface.id, surface]));
  // Every way a view can change the slice, not just the two obvious ones:
  // removing a record and editing a field write the whole list back exactly
  // as adding does.
  const writes = (surface) =>
    surface.view.document.allowAdd ||
    surface.view.document.allowToggle ||
    surface.view.document.allowRemove ||
    surface.view.document.views.some((view) => view.editable?.length);
  for (const surface of surfaces) {
    const at = `surfaces[${surface.id}]`;
    if (surface.stateId === surface.id) {
      if (surface.aggregate)
        throw new Error(
          `${at}.aggregate reads its own slice, which nothing can write to, so the page would always be empty. Point "stateId" at the surface people edit.`,
        );
      continue;
    }
    const owner = byId.get(surface.stateId);
    if (!owner)
      throw new Error(
        `${at}.stateId: ${JSON.stringify(surface.stateId)} is not a surface in this extension. Declared: ${[...byId.keys()].join(", ")}.`,
      );
    if (surface.stateScope === "instance")
      throw new Error(
        `${at}.stateId needs "stateScope": "project" or "global"; instance state is keyed per pane, so the two would never share anything.`,
      );
    if (owner.stateScope !== surface.stateScope)
      throw new Error(
        `${at}.stateId: ${owner.id} keeps ${owner.stateScope} state, not ${surface.stateScope}.`,
      );
    if (owner.stateVersion !== surface.stateVersion)
      throw new Error(
        `${at}.stateId: ${owner.id} keeps version ${owner.stateVersion} state, not ${surface.stateVersion}.`,
      );
    if (writes(surface))
      throw new Error(
        `${at}.stateId reads ${owner.id}'s contents, so it must not write them: set "allowAdd", "allowToggle" and "allowRemove" to false, drop "editable", or set "aggregate": true.`,
      );
    // The host validates a write against the owner's fields, so a borrower
    // that names a field the owner lacks would be refused at runtime instead.
    const known = new Set(owner.view.document.fields.map((field) => field.id));
    const stray = surface.view.document.fields.find(
      (field) => !known.has(field.id),
    );
    if (stray)
      throw new Error(
        `${at}.stateId: ${owner.id} has no "${stray.id}" field, so that column would always be empty.`,
      );
  }
  const navigation = (contributions.navigation || []).map((item) => {
    const allowedPlacements = list(
      item.allowedPlacements,
      "navigation.allowedPlacements",
      PLACEMENTS,
    );
    const defaultPlacement = requiredString(
      item.defaultPlacement,
      "navigation.defaultPlacement",
    );
    if (!allowedPlacements.includes(defaultPlacement))
      throw new Error("Navigation defaultPlacement must be allowed.");
    return {
      id: id(item.id, "navigation.id"),
      extensionId,
      targetSurfaceId: id(item.targetSurfaceId, "navigation.targetSurfaceId"),
      allowedPlacements,
      defaultPlacement,
      label: requiredString(item.label, "navigation.label"),
      icon: icon(item.icon, "navigation.icon"),
      order: order(item.order),
    };
  });
  const actions = (contributions.actions || []).map((item) => {
    const allowedPlacements = list(
      item.allowedPlacements,
      "action.allowedPlacements",
      ACTION_PLACEMENTS,
    );
    const defaultPlacement = requiredString(
      item.defaultPlacement,
      "action.defaultPlacement",
    );
    if (!allowedPlacements.includes(defaultPlacement))
      throw new Error("Action defaultPlacement must be allowed.");
    return {
      id: id(item.id, "action.id"),
      extensionId,
      commandId: id(item.commandId, "action.commandId"),
      allowedPlacements,
      defaultPlacement,
      label: requiredString(item.label, "action.label"),
      icon: icon(item.icon, "action.icon"),
      order: order(item.order),
    };
  });
  // Opening a surface is the only thing a command does, so one that names no
  // surface has nothing to do: every route resolved it to "unavailable".
  const commands = (contributions.commands || []).map((item) => ({
    id: id(item.id, "command.id"),
    extensionId,
    title: requiredString(item.title, "command.title"),
    surfaceId: id(item.surfaceId, "command.surfaceId"),
  }));
  for (const [kind, items] of Object.entries({
    surfaces,
    navigation,
    actions,
    commands,
  })) {
    const ids = new Set();
    for (const item of items) {
      if (ids.has(item.id))
        throw new Error(
          `Duplicate extension ${kind} contribution: ${item.id}.`,
        );
      ids.add(item.id);
    }
  }
  const surfaceIds = new Set(surfaces.map((item) => item.id));
  const commandIds = new Set(commands.map((item) => item.id));
  for (const item of navigation)
    if (!surfaceIds.has(item.targetSurfaceId))
      throw new Error(
        `Navigation targets an unknown extension surface: ${item.targetSurfaceId}.`,
      );
  for (const item of actions)
    if (!commandIds.has(item.commandId))
      throw new Error(
        `Action targets an unknown extension command: ${item.commandId}.`,
      );
  for (const item of commands)
    if (item.surfaceId && !surfaceIds.has(item.surfaceId))
      throw new Error(
        `Command targets an unknown extension surface: ${item.surfaceId}.`,
      );
  for (const surface of surfaces)
    for (const view of surface.view.document?.views || [])
      for (const commandId of view.actions || [])
        if (!commandIds.has(commandId))
          throw new Error(
            `surfaces[${surface.id}] puts an unknown command on its page: ${commandId}.`,
          );
  return {
    id: extensionId,
    name,
    version,
    apiVersion: 1,
    source,
    scope: "app",
    ...(typeof input.description === "string"
      ? { description: input.description }
      : {}),
    contributions: { surfaces, navigation, actions, commands },
  };
}

function extensionSourceLabel(source) {
  if (source.kind === "builtin") return "Built-in";
  if (source.kind === "local") return `Local:${path.basename(source.path)}`;
  if (source.kind === "npm") return `npm:${source.package}@${source.version}`;
  return `Git:${source.url}#${source.requestedRef}`;
}

/** Every closed set a manifest may draw from, in one place. The suite walks
 * this rather than a copy of it, so widening the contract fails the coverage
 * test until the probe fixture exercises the new value. */
const CONTRACT = {
  HOSTS,
  PLACEMENTS,
  ACTION_PLACEMENTS,
  INSTANCE_POLICIES,
  STATE_SCOPES,
  ICONS,
  FIELD_TYPES,
  LAYOUTS,
  TONES,
  FILTER_OPS,
  SORT_DIRS,
  PSEUDO_FIELDS,
  TOKEN_ACCENT: TOKENS.accent,
  TOKEN_DENSITY: TOKENS.density,
  TOKEN_RADIUS: TOKENS.radius,
  TOKEN_ELEVATION: TOKENS.elevation,
};

module.exports = {
  validateExtensionManifest,
  validateExtensionSource,
  extensionSourceLabel,
  CONTRACT,
};
