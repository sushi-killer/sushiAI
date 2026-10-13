// Pure logic behind the Orchestration tab's project scope and its "Copy
// settings to..." action. orchd holds one settings object per host daemon; a
// project keeps a default every host of the project uses plus per-host
// overrides, and the app writes the merged result to each host. No React, no
// bridge.
import type { Settings } from "./types";

/** A project's orchd settings: `defaults` for every host of the project,
 * `hosts[id]` the override of one host ("local" or "ssh:..."). */
export type ProjectOrch = {
  defaults?: Partial<Settings>;
  hosts?: Record<string, Partial<Settings>>;
};

/** Where an effective value comes from. */
export type SettingOrigin = "override" | "project" | "host";

/** Keys that name repos, paths or commands of one machine: copying settings
 * to another host keeps the target's own value. */
export const HOST_SPECIFIC_KEYS = [
  "worktreeRoot",
  "sandboxRepos",
  "landOnDefaultRepos",
  "gateAt",
  "afterLand",
  "scopedChecks",
  "mcpEnable",
  "mcpDisable",
  "repoPresets",
] as const;

/** Containers merged per sub-key (`tiers.hard`, `experiments.land`); every
 * other key is one value. */
const NESTED = new Set(["tiers", "experiments"]);

type Values = Record<string, unknown>;

function isObject(value: unknown): value is Values {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

/** `path` as [key] or [container, sub-key]. */
function split(path: string): [string] | [string, string] {
  const dot = path.indexOf(".");
  if (dot > 0 && NESTED.has(path.slice(0, dot)))
    return [path.slice(0, dot), path.slice(dot + 1)];
  return [path];
}

function has(layer: Values | undefined, path: string): boolean {
  if (!layer) return false;
  const [key, sub] = split(path);
  if (sub === undefined) return Object.hasOwn(layer, key);
  const inner = layer[key];
  return isObject(inner) && Object.hasOwn(inner, sub);
}

/** The value of `path` in `layer`, undefined when it holds none. */
export function layerValue(
  layer: Partial<Settings> | undefined,
  path: string,
): unknown {
  const values = layer as Values | undefined;
  if (!has(values, path)) return undefined;
  const [key, sub] = split(path);
  return sub === undefined ? values![key] : (values![key] as Values)[sub];
}

/** `layer` with `path` set to `value`; an undefined value removes it. Empty
 * containers are dropped. Never mutates. */
function setIn(
  layer: Partial<Settings> | undefined,
  path: string,
  value: unknown,
): Partial<Settings> {
  const next: Values = { ...(layer as Values | undefined) };
  const [key, sub] = split(path);
  if (sub === undefined) {
    if (value === undefined) delete next[key];
    else next[key] = value;
    return next as Partial<Settings>;
  }
  const inner: Values = isObject(next[key]) ? { ...(next[key] as Values) } : {};
  if (value === undefined) delete inner[sub];
  else inner[sub] = value;
  if (Object.keys(inner).length) next[key] = inner;
  else delete next[key];
  return next as Partial<Settings>;
}

/** `orch` with its parts that hold nothing left out. */
function tidy(defaults: Values, hosts: Record<string, Values>): ProjectOrch {
  const out: ProjectOrch = {};
  if (Object.keys(defaults).length)
    out.defaults = defaults as Partial<Settings>;
  const kept = Object.fromEntries(
    Object.entries(hosts).filter(([, layer]) => Object.keys(layer).length),
  );
  if (Object.keys(kept).length)
    out.hosts = kept as Record<string, Partial<Settings>>;
  return out;
}

/** `layer` laid over `base`: tiers and experiments per sub-key, every other
 * key whole. A falsy value (false, 0, "") overrides too. */
function overlay(base: Settings, layer: Partial<Settings> | undefined) {
  if (!layer) return base;
  const next: Values = { ...(base as unknown as Values) };
  for (const [key, value] of Object.entries(layer as Values)) {
    if (value === undefined) continue;
    next[key] =
      NESTED.has(key) && isObject(value) && isObject(next[key])
        ? { ...(next[key] as Values), ...value }
        : value;
  }
  return next as unknown as Settings;
}

/** What `host` runs with: its own saved settings, then the project default,
 * then the host's override. `base` unchanged when the project has none. */
export function effectiveSettings(
  base: Settings,
  orch: ProjectOrch | null | undefined,
  host: string,
): Settings {
  if (!orch) return base;
  return overlay(overlay(base, orch.defaults), orch.hosts?.[host]);
}

/** Where `host`'s value of `path` comes from: its override, the project
 * default, or the host's own settings. */
export function originOf(
  orch: ProjectOrch | null | undefined,
  host: string,
  path: string,
): SettingOrigin {
  if (has(orch?.hosts?.[host] as Values | undefined, path)) return "override";
  if (has(orch?.defaults as Values | undefined, path)) return "project";
  return "host";
}

/** `orch` with the project default of `path` set to `value`. */
export function withProjectValue(
  orch: ProjectOrch | null | undefined,
  path: string,
  value: unknown,
): ProjectOrch {
  return tidy(setIn(orch?.defaults, path, value) as Values, {
    ...(orch?.hosts as Record<string, Values> | undefined),
  });
}

/** `orch` without the project default of `path`. */
export function withoutProjectValue(
  orch: ProjectOrch | null | undefined,
  path: string,
): ProjectOrch {
  return withProjectValue(orch, path, undefined);
}

/** `orch` with `host`'s override of `path` set to `value`. */
export function withHostOverride(
  orch: ProjectOrch | null | undefined,
  host: string,
  path: string,
  value: unknown,
): ProjectOrch {
  return tidy(
    { ...(orch?.defaults as Values | undefined) },
    {
      ...(orch?.hosts as Record<string, Values> | undefined),
      [host]: setIn(orch?.hosts?.[host], path, value) as Values,
    },
  );
}

/** `orch` without `host`'s override of `path`; its other overrides stay. */
export function withoutHostOverride(
  orch: ProjectOrch | null | undefined,
  host: string,
  path: string,
): ProjectOrch {
  return withHostOverride(orch, host, path, undefined);
}

/** Every path `layer` sets, as `originOf` names them. */
export function layerPaths(layer: Partial<Settings> | undefined): string[] {
  const out: string[] = [];
  for (const [key, value] of Object.entries((layer ?? {}) as Values)) {
    if (NESTED.has(key) && isObject(value))
      for (const sub of Object.keys(value)) out.push(`${key}.${sub}`);
    else out.push(key);
  }
  return out;
}

/** The paths whose value differs between two settings objects, at the
 * granularity a project layer stores them. */
export function changedPaths(before: Settings, after: Settings): string[] {
  const a = before as unknown as Values;
  const b = after as unknown as Values;
  const out: string[] = [];
  for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
    if (NESTED.has(key) && isObject(a[key]) && isObject(b[key])) {
      const x = a[key] as Values;
      const y = b[key] as Values;
      for (const sub of new Set([...Object.keys(x), ...Object.keys(y)]))
        if (JSON.stringify(x[sub]) !== JSON.stringify(y[sub]))
          out.push(`${key}.${sub}`);
    } else if (JSON.stringify(a[key]) !== JSON.stringify(b[key])) out.push(key);
  }
  return out;
}

/** How many stored values differ between two versions of a project's
 * settings (each scope's paths compared one by one). */
export function countOrchChanges(
  saved: ProjectOrch | null | undefined,
  draft: ProjectOrch | null | undefined,
): number {
  const scopes = (orch: ProjectOrch | null | undefined) => {
    const out = new Map<string, string>();
    const add = (scope: string, layer: Partial<Settings> | undefined) => {
      for (const path of layerPaths(layer))
        out.set(
          `${scope}\u0000${path}`,
          JSON.stringify(layerValue(layer, path)),
        );
    };
    add("", orch?.defaults);
    for (const [host, layer] of Object.entries(orch?.hosts ?? {}))
      add(host, layer);
    return out;
  };
  const a = scopes(saved);
  const b = scopes(draft);
  let count = 0;
  for (const key of new Set([...a.keys(), ...b.keys()]))
    if (a.get(key) !== b.get(key)) count++;
  return count;
}

/** `target` with every key of `source` except the host-specific ones, which
 * keep the target's value (absent stays absent). Routes and tiers travel
 * together, so no tier points at a route the target lacks. A key only the
 * target has stays. Never mutates. */
export function copySettings(source: Settings, target: Settings): Settings {
  const from = source as unknown as Values;
  const to = target as unknown as Values;
  const keep = new Set<string>(HOST_SPECIFIC_KEYS);
  const next: Values = {};
  for (const key of Object.keys(to)) next[key] = to[key];
  for (const key of Object.keys(from))
    if (!keep.has(key)) next[key] = structuredClone(from[key]);
  for (const key of keep) {
    if (Object.hasOwn(to, key)) next[key] = to[key];
    else delete next[key];
  }
  return next as unknown as Settings;
}

/** The hosts a project knows: this machine first, then each SSH host of its
 * folders, targets and host entries, sorted. */
export function projectHostIds(project: {
  folders?: { endpoint: string }[];
  targets?: string[];
  hosts?: Record<string, unknown>;
}): string[] {
  const remote = new Set<string>();
  const add = (id: unknown) => {
    if (typeof id === "string" && id.startsWith("ssh:")) remote.add(id);
  };
  for (const folder of project.folders ?? []) add(folder.endpoint);
  for (const target of project.targets ?? []) add(target);
  for (const id of Object.keys(project.hosts ?? {})) add(id);
  return ["local", ...[...remote].sort()];
}
