import React from "react";
import {
  AlertTriangle,
  CalendarClock,
  CheckCircle2,
  Clock3,
  Copy,
  FileText,
  PowerOff,
  RefreshCw,
  Search,
  Sparkles,
} from "lucide-react";
import type {
  SkillCatalogItem,
  SkillManagementAction,
  SkillProvider,
} from "./types";
import { SkillEditorPanel } from "./SkillEditorDrawer";
import { agentTitle } from "./lib/agent-title.ts";

type StatusFilter =
  | "all"
  | "recent"
  | "needs-review"
  | "changed"
  | "duplicates"
  | "unused"
  | "stale";
type ScopeFilter = "all" | SkillProvider;
type AvailabilityFilter = "all" | "active" | "disabled";

const skillGroups: {
  id: SkillProvider;
  label: string;
  dot: string;
}[] = [
  // agent label key: the skill store is named by this provider value, not shown as an agent name
  { id: "Codex", label: agentTitle("codex"), dot: "codex" },
  { id: "Claude", label: "Claude", dot: "claude" },
];

const statusFilters: { id: StatusFilter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "recent", label: "Recent" },
  { id: "needs-review", label: "Needs review" },
  { id: "changed", label: "Changed" },
  { id: "duplicates", label: "Duplicates" },
  { id: "unused", label: "Unused" },
  { id: "stale", label: "Stale" },
];
// agent label key: the skill store is named by this provider value, not shown as an agent name
const providers: SkillProvider[] = ["Codex", "Claude"];
const INITIAL_SKILLS_BATCH = 96;
const SKILLS_BATCH_SIZE = 96;
const day = 24 * 60 * 60 * 1000;

function relativeDate(value?: number) {
  if (!value) return "Never recorded";
  const age = Math.max(0, Date.now() - value);
  if (age < 60 * 1000) return "just now";
  if (age < day)
    return `${Math.max(1, Math.floor(age / (60 * 60 * 1000)))}h ago`;
  if (age < 30 * day) return `${Math.floor(age / day)}d ago`;
  return new Date(value).toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}

function formatSize(value?: number) {
  if (!value) return "Size unknown";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  return `${(value / (1024 * 1024)).toFixed(1)} MB`;
}

/** About four characters per token; shown as "~N tokens". */
function tokensOf(item: SkillCatalogItem) {
  return Math.ceil((item.descriptionChars ?? item.description.length) / 4);
}

function usageText(item: SkillCatalogItem) {
  if (!item.usageIndexedAt) return "Usage not indexed yet";
  if (!item.usageCount) return "No uses in 90 days";
  const uses = item.usageCount.toLocaleString();
  return `${uses} ${item.usageCount === 1 ? "use" : "uses"} in 90 days${
    item.lastUsedAt ? ` · last ${relativeDate(item.lastUsedAt)}` : ""
  }`;
}

function matchesStatus(item: SkillCatalogItem, filter: StatusFilter) {
  if (filter === "all") return true;
  if (filter === "needs-review") return Boolean(item.needsReview);
  if (filter === "recent") return Boolean(item.isRecent);
  if (filter === "changed") return Boolean(item.changed);
  if (filter === "duplicates") return Boolean(item.isDuplicate);
  if (filter === "unused") return Boolean(item.isUnused);
  return Boolean(item.isStale);
}

function matchesScope(item: SkillCatalogItem, scope: ScopeFilter): boolean {
  return scope === "all" || item.provider === scope;
}

function matchesAvailability(
  item: SkillCatalogItem,
  filter: AvailabilityFilter,
): boolean {
  return filter === "all" || item.availability === filter;
}

function itemKey(item: SkillCatalogItem) {
  return `${item.provider}:${item.path}`;
}

function recommendation(item: SkillCatalogItem) {
  if (item.availability === "disabled")
    return "Installed but disabled in the harness; it will not enter context.";
  if (item.changed) return "Changed since the last scan — review this update.";
  if (item.isDuplicate) {
    return item.duplicateKind === "exact"
      ? `Exact copy found · compare ${item.duplicateCount || 2} copies and keep one.`
      : `${item.harness || item.provider} sees this name more than once · choose the copy you want to keep.`;
  }
  if (item.isUnused)
    return "No uses in the last 90 days · candidate for cleanup.";
  if (item.isStale)
    return "Not modified for 180+ days · review before keeping it.";
  return "No cleanup action suggested.";
}

function statusClass(item: SkillCatalogItem) {
  return `${item.needsReview ? " needs-review" : ""}${
    item.availability === "disabled" ? " disabled" : ""
  }`;
}

export function LocalSkillsView({
  items,
  search,
  loading,
  onSearchChange,
  onRefresh,
  onManage,
  home,
}: {
  items: SkillCatalogItem[];
  search: string;
  loading: boolean;
  onSearchChange(value: string): void;
  onRefresh(): void;
  onManage(
    action: SkillManagementAction,
    item: SkillCatalogItem,
  ): Promise<void>;
  home?: string;
}) {
  const [statusFilter, setStatusFilter] = React.useState<StatusFilter>("all");
  const [scopeFilter, setScopeFilter] = React.useState<ScopeFilter>("all");
  const [availabilityFilter, setAvailabilityFilter] =
    React.useState<AvailabilityFilter>("all");
  const [selectedSkill, setSelectedSkill] = React.useState<string | null>(null);
  const [editorItem, setEditorItem] = React.useState<SkillCatalogItem | null>(
    null,
  );
  const [visibleLimit, setVisibleLimit] = React.useState(INITIAL_SKILLS_BATCH);
  const deferredSearch = React.useDeferredValue(search);
  const query = deferredSearch.trim().toLowerCase();
  const queryMatches = React.useMemo(
    () =>
      items.filter((item) =>
        query
          ? `${item.name} ${item.description} ${item.path} ${item.provider} ${item.source}`
              .toLowerCase()
              .includes(query)
          : true,
      ),
    [items, query],
  );
  const searchable = React.useMemo(
    () =>
      queryMatches
        .filter((item) => matchesScope(item, scopeFilter))
        .filter((item) => matchesAvailability(item, availabilityFilter)),
    [queryMatches, scopeFilter, availabilityFilter],
  );
  const filtered = React.useMemo(
    () =>
      searchable
        .filter((item) => matchesStatus(item, statusFilter))
        .sort((a, b) =>
          statusFilter === "unused"
            ? tokensOf(b) - tokensOf(a) ||
              a.name.localeCompare(b.name, undefined, { sensitivity: "base" })
            : Number(Boolean(b.needsReview)) - Number(Boolean(a.needsReview)) ||
              (b.lastUsedAt || 0) - (a.lastUsedAt || 0) ||
              a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
        ),
    [searchable, statusFilter],
  );
  const statusCounts = React.useMemo(() => {
    const counts: Record<StatusFilter, number> = {
      all: searchable.length,
      recent: 0,
      "needs-review": 0,
      changed: 0,
      duplicates: 0,
      unused: 0,
      stale: 0,
    };
    for (const item of searchable) {
      if (item.isRecent) counts.recent += 1;
      if (item.needsReview) counts["needs-review"] += 1;
      if (item.changed) counts.changed += 1;
      if (item.isDuplicate) counts.duplicates += 1;
      if (item.isUnused) counts.unused += 1;
      if (item.isStale) counts.stale += 1;
    }
    return counts;
  }, [searchable]);
  const weight = React.useMemo(() => {
    let total = 0;
    let unused = 0;
    for (const item of items) {
      if (item.availability === "disabled") continue;
      total += tokensOf(item);
      if (item.isUnused) unused += tokensOf(item);
    }
    return { total, unused };
  }, [items]);
  const scopeCounts = React.useMemo(() => {
    const counts: Record<SkillProvider, number> = { Codex: 0, Claude: 0 };
    for (const item of queryMatches)
      if (item.provider) counts[item.provider] += 1;
    return counts;
  }, [queryMatches]);
  const availabilityCounts = React.useMemo(() => {
    const scopedItems = queryMatches.filter((item) =>
      matchesScope(item, scopeFilter),
    );
    const counts: Record<AvailabilityFilter, number> = {
      all: scopedItems.length,
      active: 0,
      disabled: 0,
    };
    for (const item of scopedItems) {
      if (item.availability) counts[item.availability] += 1;
    }
    return counts;
  }, [queryMatches, scopeFilter]);
  const count = (filter: StatusFilter) => statusCounts[filter];
  const scopeCount = (scope: SkillProvider) => scopeCounts[scope];
  const visibleFiltered = React.useMemo(
    () => filtered.slice(0, visibleLimit),
    [filtered, visibleLimit],
  );
  const groupedSkills = React.useMemo(
    () =>
      skillGroups
        .map((groupDefinition) => ({
          ...groupDefinition,
          allItems: filtered.filter(
            (item) => item.provider === groupDefinition.id,
          ),
          items: visibleFiltered.filter(
            (item) => item.provider === groupDefinition.id,
          ),
        }))
        .filter((group) => group.items.length > 0),
    [filtered, visibleFiltered],
  );
  React.useEffect(() => {
    setVisibleLimit(INITIAL_SKILLS_BATCH);
  }, [items, query, scopeFilter, availabilityFilter, statusFilter]);
  const openSkill = (item: SkillCatalogItem) => {
    setSelectedSkill(itemKey(item));
    setEditorItem(item);
  };
  const editorPluginSkillCount = editorItem?.plugin
    ? items.filter(
        (item) => item.source === "Plugin" && item.plugin === editorItem.plugin,
      ).length
    : 0;

  return (
    <div className="skills-view">
      <div className="skills-workspace">
        <div className="skills-catalog">
          <div className="skills-toolbar">
            <label className="catalog-search skill-search">
              <Search size={15} />
              <input
                aria-label="Search skills"
                placeholder="Search local skills…"
                value={search}
                onChange={(event) => onSearchChange(event.target.value)}
              />
            </label>
            <button
              className="skill-refresh"
              aria-label="Refresh local skills"
              title="Scan local skill folders again"
              onClick={onRefresh}
              disabled={loading}
            >
              <RefreshCw size={15} className={loading ? "spin" : ""} />
              <span>{loading ? "Scanning…" : "Refresh"}</span>
            </button>
          </div>
          <div className="skill-meta">
            <span>
              Descriptions of active skills: ~{weight.total.toLocaleString()}{" "}
              tokens, ~{weight.unused.toLocaleString()} unused
            </span>
          </div>
          <div className="skills-filter-row">
            <div
              className="skills-filter-tabs"
              role="tablist"
              aria-label="Skill status"
            >
              {statusFilters.map((filter) => (
                <button
                  key={filter.id}
                  className={statusFilter === filter.id ? "selected" : ""}
                  aria-selected={statusFilter === filter.id}
                  role="tab"
                  onClick={() => setStatusFilter(filter.id)}
                >
                  {filter.label}
                  <span>{count(filter.id)}</span>
                </button>
              ))}
            </div>
            <label className="skills-scope">
              <span>Owner</span>
              <select
                aria-label="Filter skills by owner"
                value={scopeFilter}
                onChange={(event) =>
                  setScopeFilter(event.target.value as ScopeFilter)
                }
              >
                <option value="all">All agents ({queryMatches.length})</option>
                {providers.map((provider) => (
                  <option key={provider} value={provider}>
                    {provider} ({scopeCount(provider)})
                  </option>
                ))}
              </select>
            </label>
            <label className="skills-scope">
              <span>Context</span>
              <select
                aria-label="Filter skills by context availability"
                value={availabilityFilter}
                onChange={(event) =>
                  setAvailabilityFilter(
                    event.target.value as AvailabilityFilter,
                  )
                }
              >
                <option value="all">
                  All contexts ({availabilityCounts.all})
                </option>
                <option value="active">
                  Active / loadable ({availabilityCounts.active})
                </option>
                <option value="disabled">
                  Disabled ({availabilityCounts.disabled})
                </option>
              </select>
            </label>
          </div>
          <div className="skills-summary" aria-label="Skill summary">
            <div className="skills-summary-total">
              <Sparkles size={16} />
              <strong>{searchable.length}</strong>
              <span>Skills on device</span>
            </div>
            <div className="skills-summary-active">
              <CheckCircle2 size={16} />
              <strong>{availabilityCounts.active}</strong>
              <span>Active / loadable</span>
            </div>
            <div className="skills-summary-disabled">
              <PowerOff size={16} />
              <strong>{availabilityCounts.disabled}</strong>
              <span>Disabled</span>
            </div>
          </div>
          <div className="skills-health" aria-label="Cleanup signal summary">
            <span>
              <Clock3 size={12} /> Recent <strong>{count("recent")}</strong>
            </span>
            <span>
              <AlertTriangle size={12} /> Needs review{" "}
              <strong>{count("needs-review")}</strong>
            </span>
            <span>
              <Copy size={12} /> Duplicates{" "}
              <strong>{count("duplicates")}</strong>
            </span>
          </div>
          <p className="skills-note">
            <Clock3 size={13} /> Usage is counted from {agentTitle("claude")}{" "}
            and {agentTitle("codex")} transcripts over the last 90 days. Active
            skills are loadable by the owner harness; disabled skills are shown
            for cleanup context. Only {agentTitle("claude")} and{" "}
            {agentTitle("codex")} skills are listed, each file once per harness.
            Cache-only plugin files are excluded.
          </p>
          {filtered.length ? (
            <div className="skills-groups">
              {groupedSkills.map((groupDefinition) => {
                return (
                  <section className="skill-group" key={groupDefinition.id}>
                    <div className="skill-group-heading">
                      <div>
                        <span
                          className={`provider-dot ${groupDefinition.dot}`}
                        />
                        <h3>{groupDefinition.label}</h3>
                      </div>
                      <span>{groupDefinition.allItems.length}</span>
                    </div>
                    <div className="skills-grid">
                      {groupDefinition.items.map((item) => (
                        <article
                          className={`skill-card${statusClass(item)} ${
                            selectedSkill === itemKey(item) ? "selected" : ""
                          }`}
                          key={itemKey(item)}
                          tabIndex={0}
                          aria-pressed={selectedSkill === itemKey(item)}
                          onClick={() => openSkill(item)}
                          onKeyDown={(event) => {
                            if (event.target !== event.currentTarget) return;
                            if (event.key === "Enter" || event.key === " ") {
                              event.preventDefault();
                              openSkill(item);
                            }
                          }}
                        >
                          <div className="skill-card-heading">
                            <div className="skill-card-icon">
                              <FileText size={16} />
                            </div>
                            <div className="skill-card-name">
                              <strong title={item.name}>{item.name}</strong>
                              <span>
                                {item.harness || item.provider} · {item.source}{" "}
                                · {formatSize(item.size)}
                              </span>
                            </div>
                            {item.availability === "disabled" && (
                              <span className="skill-state disabled">
                                Disabled
                              </span>
                            )}
                          </div>
                          <p className="skill-description">
                            {item.description}
                          </p>
                          <div className="skill-badges">
                            {item.changed && (
                              <span className="skill-badge changed">
                                Changed
                              </span>
                            )}
                            {item.isDuplicate && (
                              <span className="skill-badge duplicate">
                                <Copy size={11} />
                                {item.duplicateKind === "exact"
                                  ? "Exact duplicate"
                                  : "Name duplicate"}
                              </span>
                            )}
                            {item.isUnused && (
                              <span className="skill-badge muted">Unused</span>
                            )}
                            {item.isStale && (
                              <span className="skill-badge muted">Stale</span>
                            )}
                          </div>
                          <div className="skill-meta">
                            <span title="Counted from the agents' session transcripts">
                              <Clock3 size={12} />
                              {usageText(item)}
                            </span>
                            <span title="Size of the description every session loads">
                              ~{tokensOf(item).toLocaleString()} tokens
                            </span>
                            <span>
                              <CalendarClock size={12} /> Updated{" "}
                              {relativeDate(item.updatedAt)}
                            </span>
                          </div>
                          <div className="skill-recommendation">
                            <AlertTriangle size={12} />
                            <span>{recommendation(item)}</span>
                          </div>
                          <small title={item.path}>{item.path}</small>
                        </article>
                      ))}
                    </div>
                  </section>
                );
              })}
              {visibleFiltered.length < filtered.length && (
                <div className="skills-load-more">
                  <span>
                    Showing {visibleFiltered.length.toLocaleString()} of{" "}
                    {filtered.length.toLocaleString()} skills
                  </span>
                  <button
                    type="button"
                    onClick={() =>
                      setVisibleLimit((limit) =>
                        Math.min(limit + SKILLS_BATCH_SIZE, filtered.length),
                      )
                    }
                  >
                    Show{" "}
                    {Math.min(
                      SKILLS_BATCH_SIZE,
                      filtered.length - visibleFiltered.length,
                    ).toLocaleString()}{" "}
                    more
                  </button>
                </div>
              )}
            </div>
          ) : (
            <div className="skills-no-results">
              <Sparkles size={25} />
              <strong>
                {items.length
                  ? "No skills match this view"
                  : "No local skills found"}
              </strong>
              <span>
                {items.length
                  ? "Try another status, owner, or search term."
                  : "Skills are read from canonical harness roots; cache-only plugin files are excluded."}
              </span>
            </div>
          )}
        </div>
        {editorItem && (
          <SkillEditorPanel
            item={editorItem}
            home={home}
            onClose={() => setEditorItem(null)}
            onManage={async (action) => {
              await onManage(action, editorItem);
              setEditorItem(null);
              setSelectedSkill(null);
            }}
            pluginSkillCount={editorPluginSkillCount}
          />
        )}
      </div>
    </div>
  );
}
