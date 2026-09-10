import React from "react";
import {
  AlertTriangle,
  CalendarClock,
  CheckCircle2,
  Clock3,
  Copy,
  FileText,
  Globe2,
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

type StatusFilter =
  | "all"
  | "recent"
  | "needs-review"
  | "changed"
  | "duplicates"
  | "unused"
  | "stale";
type ScopeFilter = "all" | SkillProvider | "Other agents";
type OwnerFilter = "Codex" | "Claude" | "Other agents";
type AvailabilityFilter = "all" | "active" | "disabled" | "external";

const skillGroups: {
  id: string;
  label: string;
  providers: SkillProvider[];
  dot: string;
}[] = [
  { id: "Codex", label: "Codex", providers: ["Codex"], dot: "codex" },
  { id: "Claude", label: "Claude", providers: ["Claude"], dot: "claude" },
  {
    id: "Other agents",
    label: "Other agents",
    providers: ["Agent", "Other"],
    dot: "other",
  },
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
const providers: OwnerFilter[] = ["Codex", "Claude", "Other agents"];
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
  return scope === "all"
    ? true
    : scope === "Other agents"
      ? item.provider === "Agent" || item.provider === "Other"
      : item.provider === scope;
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
  if (item.availability === "external")
    return "Owned by another harness; it is not loaded by Claude Code or Codex.";
  if (item.changed) return "Changed since the last scan — review this update.";
  if (item.isDuplicate) {
    return item.duplicateKind === "exact"
      ? `Exact copy found · compare ${item.duplicateCount || 2} copies and keep one.`
      : `Same name found in another source · choose the copy you want to keep.`;
  }
  if (item.isUnused)
    return "No usage signal for 180+ days · candidate for cleanup.";
  if (item.isStale)
    return "Not modified for 180+ days · review before keeping it.";
  return "No cleanup action suggested.";
}

function statusClass(item: SkillCatalogItem) {
  return `${item.needsReview ? " needs-review" : ""}${
    item.availability === "disabled" ? " disabled" : ""
  }${item.availability === "external" ? " external" : ""}`;
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
        .sort(
          (a, b) =>
            Number(Boolean(b.needsReview)) - Number(Boolean(a.needsReview)) ||
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
  const scopeCounts = React.useMemo(() => {
    const counts: Record<"Codex" | "Claude" | "Other agents", number> = {
      Codex: 0,
      Claude: 0,
      "Other agents": 0,
    };
    for (const item of queryMatches) {
      if (item.provider === "Codex") counts.Codex += 1;
      else if (item.provider === "Claude") counts.Claude += 1;
      else counts["Other agents"] += 1;
    }
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
      external: 0,
    };
    for (const item of scopedItems) {
      if (item.availability) counts[item.availability] += 1;
    }
    return counts;
  }, [queryMatches, scopeFilter]);
  const count = (filter: StatusFilter) => statusCounts[filter];
  const scopeCount = (scope: OwnerFilter) => scopeCounts[scope];
  const visibleFiltered = React.useMemo(
    () => filtered.slice(0, visibleLimit),
    [filtered, visibleLimit],
  );
  const groupedSkills = React.useMemo(
    () =>
      skillGroups
        .map((groupDefinition) => ({
          ...groupDefinition,
          allItems: filtered.filter((item) =>
            groupDefinition.providers.includes(item.provider as SkillProvider),
          ),
          items: visibleFiltered.filter((item) =>
            groupDefinition.providers.includes(item.provider as SkillProvider),
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
                <option value="external">
                  External harness ({availabilityCounts.external})
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
            <div className="skills-summary-external">
              <Globe2 size={16} />
              <strong>{availabilityCounts.external}</strong>
              <span>External harness</span>
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
            <Clock3 size={13} /> Last used comes from local skill events when
            available; calls are counted from local events or the usage cache.
            Otherwise the file access time is only a fallback. macOS may not
            update that signal on every read. Active skills are loadable by the
            owner harness; disabled and external skills are shown for cleanup
            context. Cache-only plugin files are excluded.
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
                            {item.availability !== "active" && (
                              <span
                                className={`skill-state ${item.availability || "external"}`}
                              >
                                {item.availability === "disabled"
                                  ? "Disabled"
                                  : "External"}
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
                            <span
                              title={
                                item.lastUsedSource === "skill event"
                                  ? "Read from a local skill event"
                                  : item.lastUsedSource === "usage cache"
                                    ? "Read from the local usage cache"
                                    : "Inferred from filesystem access time"
                              }
                            >
                              <Clock3 size={12} />
                              {item.lastUsedAt
                                ? `Last used ${relativeDate(item.lastUsedAt)}`
                                : "Last used never recorded"}
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
