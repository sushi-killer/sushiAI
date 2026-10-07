// The Inbox's items and filters, without React: module attention rows and
// agent sessions in one queue, grouped by what the owner has to do with them.
import type { InboxGroup, InboxRow } from "./attention.ts";
import { elapsedLabel, plural } from "../lib/text.ts";
import { groupKey } from "./workspaceMerge.ts";
import type { AttentionAction, AttentionItem } from "../extensions/modules.ts";

export type Kind =
  "answer" | "decide" | "review" | "panels" | "working" | "idle";

type Base = {
  key: string;
  title: string;
  project: string;
  /** Local key of the host the item lives on. */
  host: string;
  at: number | null;
};
/** A row a module asks the owner to act on. */
export type ModuleEntry = { extensionId: string; item: AttentionItem };
export type ModuleItem = Base & {
  source: "module";
  kind: AttentionItem["kind"];
  entry: ModuleEntry;
};
export type SessionItem = Base & {
  source: "session";
  kind: "answer" | "panels" | "working" | "idle";
  row: InboxRow;
};
export type Item = ModuleItem | SessionItem;

/** Display order; the first four are what "needs you" counts. */
export const KINDS: Kind[] = [
  "answer",
  "decide",
  "review",
  "panels",
  "working",
  "idle",
];
const NEEDS: Kind[] = ["answer", "decide", "review", "panels"];
export const needsYou = (item: Item) => NEEDS.includes(item.kind);

export const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  gemini: "Gemini",
  "cursor-agent": "Cursor",
};

export function agentName(row: InboxRow): string {
  return (row.panel.agent && AGENT_NAMES[row.panel.agent]) || row.panel.title;
}

const SESSION_KIND = {
  blocked: "answer",
  done: "panels",
  working: "working",
  idle: "idle",
} as const;

/** Every item, in group order and newest first within a group. Shells are
 * left out: they only ever appear in the cleanup review. */
export function inboxItems(
  modules: ModuleEntry[],
  groups: InboxGroup[],
  askPanels: ReadonlySet<string> = new Set(),
): Item[] {
  const out: Item[] = [];
  for (const entry of modules)
    out.push({
      source: "module",
      key: `${entry.extensionId}:${entry.item.key}`,
      kind: entry.item.kind,
      title: entry.item.title,
      project: entry.item.project,
      host: entry.item.host,
      at: entry.item.at,
      entry,
    });
  for (const group of groups) {
    if (group.key === "shells") continue;
    for (const row of group.rows)
      out.push({
        source: "session",
        key: `panel:${row.panel.id}`,
        // A panel with an open ask needs the owner whatever its status says.
        kind: askPanels.has(row.panel.id) ? "answer" : SESSION_KIND[group.key],
        title: `${row.workspace.name} · ${agentName(row)}`,
        project: row.workspace.name,
        host: groupKey(row.workspace.connection),
        at: row.since,
        row,
      });
  }
  const rank = (kind: Kind) => KINDS.indexOf(kind);
  return out.sort(
    (a, b) => rank(a.kind) - rank(b.kind) || (b.at ?? 0) - (a.at ?? 0),
  );
}

export type Scope = { project: string; host: string; query: string };

export function inScope(item: Item, { project, host, query }: Scope): boolean {
  if (project && item.project !== project) return false;
  if (host && item.host !== host) return false;
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return item.source === "module"
    ? item.entry.item.search.toLowerCase().includes(needle)
    : `${item.row.panel.title} ${item.title}`.toLowerCase().includes(needle);
}

/** What a module's "review all" acts on: exactly the review rows on screen,
 * never the ones a filter hides, as the module's own item keys. */
export function reviewTargets(
  visible: Item[],
): { extensionId: string; keys: string[] }[] {
  const out: { extensionId: string; keys: string[] }[] = [];
  for (const item of visible) {
    if (item.source !== "module" || item.kind !== "review") continue;
    const { extensionId, item: row } = item.entry;
    const group = out.find((target) => target.extensionId === extensionId);
    if (group) group.keys.push(row.key);
    else out.push({ extensionId, keys: [row.key] });
  }
  return out;
}

/** What "Clean up" offers for review within the current host and project
 * filters: idle agents (ticked by default) and plain shells (listed apart and
 * never ticked by default - a shell may be running a dev server). */
export function cleanupCandidates(
  groups: InboxGroup[],
  { project, host }: Pick<Scope, "project" | "host">,
): { agents: InboxRow[]; shells: InboxRow[] } {
  const rows = (key: "idle" | "shells") =>
    (groups.find((group) => group.key === key)?.rows ?? []).filter(
      (row) =>
        (!host || groupKey(row.workspace.connection) === host) &&
        (!project || row.workspace.name === project),
    );
  return { agents: rows("idle"), shells: rows("shells") };
}

/** "<1m" under a minute, else "2m", "1h", "3d". */
export function ageLabel(ms: number): string {
  return ms < 60_000 ? "<1m" : elapsedLabel(ms);
}

/** The subtitle for the items in view: "3 things need you across 1 project
 * on 1 host · oldest 2m", counted within the project, host and search
 * filters. */
export function scopedHeadline(
  needs: Item[],
  filtered: boolean,
  now: number,
): string {
  if (needs.length === 0)
    return filtered ? "Nothing here needs you" : "Nothing needs you";
  const oldest = Math.min(
    ...needs.map((item) => (item.at == null ? now : item.at)),
  );
  const projects = new Set(needs.map((item) => item.project)).size;
  const hosts = new Set(needs.map((item) => item.host)).size;
  const base = `${needs.length} ${needs.length === 1 ? "thing needs" : "things need"} you across ${plural(projects, "project")} on ${plural(hosts, "host")}`;
  return needs.some((item) => item.at != null)
    ? `${base} · oldest ${ageLabel(now - oldest)}`
    : base;
}

/** Whether a global key belongs to something else: an open dialog, a text
 * field, or - for anything but J/K - a focused button or link, where Enter is
 * that button's own click, not a second answer. */
export function ownsKey(target: EventTarget | null, key: string): boolean {
  if (document.querySelector("[role=dialog], dialog[open]")) return true;
  const el = target as HTMLElement | null;
  if (!el?.closest) return false;
  if (el.isContentEditable || el.closest("input, textarea, select"))
    return true;
  return key !== "j" && key !== "k" && !!el.closest("button, a");
}

/** The key after `delta` steps from `current`, clamped to the list; the first
 * key when `current` is not in it. */
export function stepSelection(
  keys: string[],
  current: string | null,
  delta: number,
): string | null {
  if (keys.length === 0) return null;
  const at = current == null ? -1 : keys.indexOf(current);
  if (at < 0) return keys[0];
  return keys[Math.min(keys.length - 1, Math.max(0, at + delta))];
}

/** The selected row's action bound to a pressed key, if any. */
export const actionForKey = (
  actions: AttentionAction[] | undefined,
  key: string,
): AttentionAction | undefined =>
  actions?.find((action) => action.key === key.toLowerCase());

/** Footer legend entries for the actions that have a key: ["L", "land"]. */
export const actionLegend = (
  actions: AttentionAction[] | undefined,
): string[][] =>
  (actions ?? [])
    .filter((action) => action.key)
    .map((action) => [action.key!.toUpperCase(), action.label.toLowerCase()]);
