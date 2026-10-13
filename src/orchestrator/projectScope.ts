// Project-wide statistics: a project is one repository across hosts, so the
// task list and the spend of every host's folder are merged into one view.
// Pure logic - no React.
import type { OrchestratorClient } from "./client";
import type { SpendGroup, SpendRow, SpendSummary, Task } from "./types";

const LOCAL = "local";

/** One folder of the project: the host whose daemon knows it, and its path. */
export type ProjectSource = { host: string; repo: string };

type ProjectFolders = {
  folders?: { endpoint: string; cwd: string }[];
} | null;

/** The host+folder being viewed first, then every other attached folder once.
 * Hosts missing from `availableHostIds` (when given) are skipped, except the
 * current one. */
export function projectSources(
  project: ProjectFolders | undefined,
  current: ProjectSource,
  availableHostIds?: string[],
): ProjectSource[] {
  const sources = [current];
  const seen = new Set([`${current.host}\n${current.repo}`]);
  for (const folder of project?.folders ?? []) {
    const host = folder.endpoint || LOCAL;
    if (availableHostIds && !availableHostIds.includes(host)) continue;
    const key = `${host}\n${folder.cwd}`;
    if (seen.has(key)) continue;
    seen.add(key);
    sources.push({ host, repo: folder.cwd });
  }
  return sources;
}

function cacheHitRate(tokens: SpendRow["tokens"]): number {
  const seen = tokens.input + tokens.cached;
  return seen > 0 ? Math.min(1, tokens.cached / seen) : 0;
}

type Sum = Omit<SpendRow, "key" | "keys">;

function emptySum(): Sum {
  return {
    costUsd: 0,
    runs: 0,
    tokens: { input: 0, cached: 0, output: 0 },
    cacheHitRate: 0,
  };
}

function add(into: Sum, row: Sum): void {
  into.costUsd += row.costUsd;
  into.runs += row.runs;
  into.tokens.input += row.tokens.input;
  into.tokens.cached += row.tokens.cached;
  into.tokens.output += row.tokens.output;
}

/** Sums summaries row by row key and in the totals. Day rows come out oldest
 * first, other groupings by cost, highest first. */
export function mergeSpend(
  summaries: SpendSummary[],
  groupBy: SpendGroup = "day",
): SpendSummary {
  const rows = new Map<string, SpendRow>();
  const totals = emptySum();
  for (const summary of summaries) {
    add(totals, summary.totals);
    for (const row of summary.rows) {
      let into = rows.get(row.key);
      if (!into) {
        into = { key: row.key, ...emptySum() };
        if (row.keys) into.keys = row.keys;
        rows.set(row.key, into);
      }
      add(into, row);
    }
  }
  const merged = [...rows.values()];
  for (const row of merged) row.cacheHitRate = cacheHitRate(row.tokens);
  totals.cacheHitRate = cacheHitRate(totals.tokens);
  merged.sort(
    groupBy === "day"
      ? (a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0)
      : (a, b) => b.costUsd - a.costUsd,
  );
  return { rows: merged, totals };
}

type ClientFor = (host: string) => OrchestratorClient;

type Settled<T> = { host: string; value?: T; error?: unknown };

/** Runs `load` for every source in parallel; a host that throws is reported
 * in `failed`. Rejects (with the first error) only when every source fails. */
async function loadAll<T>(
  sources: ProjectSource[],
  load: (source: ProjectSource) => Promise<T>,
): Promise<{ values: T[]; failed: string[] }> {
  const settled = await Promise.all(
    sources.map(async (source): Promise<Settled<T>> => {
      try {
        return { host: source.host, value: await load(source) };
      } catch (error) {
        return { host: source.host, error: error ?? new Error("unreachable") };
      }
    }),
  );
  const failures = settled.filter((s) => "error" in s);
  if (sources.length > 0 && failures.length === sources.length)
    throw failures[0].error;
  const failed: string[] = [];
  for (const f of failures) if (!failed.includes(f.host)) failed.push(f.host);
  return {
    values: settled.filter((s) => !("error" in s)).map((s) => s.value as T),
    failed,
  };
}

/** Every source's tasks, each asked of its own host with its own folder. Tasks
 * of a remote host carry that host in `host`. */
export async function loadProjectTasks(
  clientFor: ClientFor,
  sources: ProjectSource[],
  includeArchived: boolean,
): Promise<{ tasks: Task[]; failed: string[] }> {
  const { values, failed } = await loadAll(sources, async (source) => {
    const list = await clientFor(source.host).taskList(
      source.repo,
      includeArchived,
    );
    return source.host === LOCAL
      ? list
      : list.map((t) => (t.host ? t : { ...t, host: source.host }));
  });
  return { tasks: values.flat(), failed };
}

/** Every source's spend for `query` (its `repo` is replaced per source),
 * merged into one summary. */
export async function loadProjectSpend(
  clientFor: ClientFor,
  sources: ProjectSource[],
  query: {
    taskId?: string;
    sinceDays?: number;
    from?: string;
    to?: string;
    groupBy: SpendGroup[];
  },
): Promise<{ summary: SpendSummary; failed: string[] }> {
  const { values, failed } = await loadAll(sources, (source) =>
    clientFor(source.host).costsSummary({ ...query, repo: source.repo }),
  );
  return { summary: mergeSpend(values, query.groupBy[0]), failed };
}
