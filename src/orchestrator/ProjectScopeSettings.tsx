import { useEffect, useState, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import "./settings-scope.css";
import { orchestratorClientFor } from "./client";
import { errorText } from "./helpers";
import { hostName, LOCAL_HOST } from "./hosts";
import {
  changedPaths,
  countOrchChanges,
  effectiveSettings,
  layerPaths,
  layerValue,
  originOf,
  projectHostIds,
  withHostOverride,
  withProjectValue,
  withoutHostOverride,
  withoutProjectValue,
  type ProjectOrch,
} from "./projectSettings";
import type { Project } from "../types";
import type { OrchestratorHost, Settings } from "./types";

export type SettingsScope = "host" | "project";

/** How the settings body works in project scope: it shows `view(saved)` of
 * the host it loaded, turns each edit into a project default or an override,
 * and saves through the project instead of one daemon. */
export type SettingsLayer = {
  /** What the controls show, from the shown host's saved settings. */
  view(saved: Settings): Settings;
  /** A control turned what `view(saved)` showed into `after`. */
  edit(saved: Settings, after: Settings): void;
  /** "inherited from project" / "overridden on <host> ↺" beside a value. */
  marker(path: string): ReactNode;
  changes: number;
  saving: boolean;
  error: string;
  discard(): void;
  save(): Promise<void>;
};

/** Host or Project: whose orchd settings the tab edits. */
export function SettingsScopeChoice({
  scope,
  onChange,
}: {
  scope: SettingsScope;
  onChange(scope: SettingsScope): void;
}) {
  const options: { id: SettingsScope; label: string }[] = [
    { id: "host", label: "Host" },
    { id: "project", label: "Project" },
  ];
  return (
    <div className="os-scope" role="radiogroup" aria-label="Settings scope">
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={scope === option.id}
          className={`os-scope-option${scope === option.id ? " selected" : ""}`}
          onClick={() => onChange(option.id)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** A labelled select of the scope bar (project, host). */
export function ScopePicker({
  label,
  ariaLabel = label,
  value,
  onChange,
  children,
}: {
  label: string;
  ariaLabel?: string;
  value: string;
  onChange(value: string): void;
  children: ReactNode;
}) {
  return (
    <label className="os-scope-field">
      <span>{label}</span>
      <span className="os-scope-select">
        <select
          aria-label={ariaLabel}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          {children}
        </select>
        <ChevronDown size={12} aria-hidden />
      </span>
    </label>
  );
}

type HostResult = { host: string; error?: string };

/** Settings -> Orchestration in project scope: a project default every host
 * of the project uses and per-host overrides. `render(host, layer, key)` is
 * the settings body for the shown host ("local" when no host is picked). */
export function ProjectScopeSettings({
  hosts,
  render,
}: {
  hosts: OrchestratorHost[];
  render(host: string, layer: SettingsLayer, key: string): ReactNode;
}) {
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [projectId, setProjectId] = useState("");
  // "" edits the project default; a host id shows and overrides that host.
  const [host, setHost] = useState(LOCAL_HOST);
  const [saved, setSaved] = useState<ProjectOrch>({});
  const [draft, setDraft] = useState<ProjectOrch>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [results, setResults] = useState<HostResult[]>([]);
  // Bumped after a save, so the body reloads the host's new saved settings.
  const [reload, setReload] = useState(0);

  useEffect(() => {
    if (!window.bridge) {
      setProjects([]);
      return;
    }
    window.bridge
      .projectsList()
      .then((list) => {
        setProjects(list);
        setProjectId(list[0]?.id ?? "");
        setSaved(list[0]?.orch ?? {});
        setDraft(list[0]?.orch ?? {});
      })
      .catch((e) => {
        setProjects([]);
        setError(errorText(e));
      });
  }, []);

  function pick(project: Project | undefined) {
    setProjectId(project?.id ?? "");
    setSaved(project?.orch ?? {});
    setDraft(project?.orch ?? {});
    setHost(LOCAL_HOST);
    setResults([]);
    setError("");
  }

  if (!projects) return <div className="loading">Loading projects…</div>;
  const project = projects.find((item) => item.id === projectId);
  if (!project)
    return (
      <p className="os-scope-empty">
        No projects yet. Add one in the sidebar to give its hosts shared
        orchestration settings.
      </p>
    );
  const hostIds = projectHostIds(project);
  const nameOf = (id: string) => hostName(hosts, id);

  /** `host`'s value of `path` without its override. */
  const inherited = (base: Settings, path: string) =>
    layerValue(
      effectiveSettings(base, { defaults: draft.defaults }, host),
      path,
    );

  const layer: SettingsLayer = {
    view: (base) =>
      host
        ? effectiveSettings(base, draft, host)
        : effectiveSettings(base, { defaults: draft.defaults }, ""),
    edit(base, after) {
      const before = layer.view(base);
      let next = draft;
      for (const path of changedPaths(before, after)) {
        const value = layerValue(after, path);
        if (!host) next = withProjectValue(next, path, value);
        else if (
          JSON.stringify(value) === JSON.stringify(inherited(base, path))
        )
          next = withoutHostOverride(next, host, path);
        else next = withHostOverride(next, host, path, value);
      }
      setDraft(next);
    },
    marker(path) {
      const origin = originOf(draft, host, path);
      if (host && origin === "override")
        return (
          <button
            type="button"
            className="os-origin overridden"
            aria-label={`Reset ${path} on ${nameOf(host)} to the project value`}
            title="Remove this host's override; Save writes it"
            onClick={() => setDraft(withoutHostOverride(draft, host, path))}
          >
            overridden on {nameOf(host)} ↺
          </button>
        );
      if (origin === "project")
        return host ? (
          <span className="os-origin inherited">inherited from project</span>
        ) : (
          <button
            type="button"
            className="os-origin inherited"
            aria-label={`Remove the project default of ${path}`}
            title="Remove the project default; Save writes it"
            onClick={() => setDraft(withoutProjectValue(draft, path))}
          >
            project default ↺
          </button>
        );
      return null;
    },
    changes: countOrchChanges(saved, draft),
    saving,
    error,
    discard: () => setDraft(saved),
    async save() {
      if (!window.bridge || !project) return;
      setSaving(true);
      setError("");
      setResults([]);
      let stored: ProjectOrch;
      try {
        stored = await window.bridge.projectOrchSet(
          project.id,
          "project",
          draft.defaults ?? {},
        );
        const scopes = new Set([
          ...Object.keys(saved.hosts ?? {}),
          ...Object.keys(draft.hosts ?? {}),
        ]);
        for (const id of scopes)
          stored = await window.bridge.projectOrchSet(
            project.id,
            id,
            draft.hosts?.[id] ?? {},
          );
      } catch (e) {
        setError(errorText(e));
        setSaving(false);
        return;
      }
      setSaved(stored);
      setDraft(stored);
      setProjects((list) =>
        (list ?? []).map((item) =>
          item.id === project.id ? { ...item, orch: stored } : item,
        ),
      );
      // Each host of the project gets its effective settings through its own
      // daemon, one at a time; a host that fails does not stop the rest.
      const done: HostResult[] = [];
      for (const id of hostIds) {
        try {
          const client = orchestratorClientFor(id);
          const base = await client.settingsGet();
          await client.settingsSet(effectiveSettings(base, stored, id));
          done.push({ host: id });
        } catch (e) {
          done.push({ host: id, error: errorText(e) });
        }
      }
      setResults(done);
      setReload((n) => n + 1);
      setSaving(false);
    },
  };

  const failed = results.filter((result) => result.error);
  const written = results.filter((result) => !result.error);
  const overrides = host ? layerPaths(draft.hosts?.[host]).length : 0;

  return (
    <>
      <div className="os-scope-bar">
        <ScopePicker
          label="Project"
          value={project.id}
          onChange={(id) => pick(projects.find((item) => item.id === id))}
        >
          {projects.map((item) => (
            <option key={item.id} value={item.id}>
              {item.name}
            </option>
          ))}
        </ScopePicker>
        <ScopePicker label="Host" value={host} onChange={setHost}>
          <option value="">No host (project default)</option>
          {hostIds.map((id) => (
            <option key={id} value={id}>
              {nameOf(id)}
            </option>
          ))}
        </ScopePicker>
      </div>
      <p className="os-scope-note">
        {host
          ? `Showing what ${nameOf(host)} runs with. A change here overrides the project default on ${nameOf(host)} only${overrides ? ` (${overrides} ${overrides === 1 ? "override" : "overrides"})` : ""}.`
          : `A change here is the project default for every host of ${project.name}: ${hostIds.map(nameOf).join(", ")}.`}{" "}
        Save writes each host's settings through its own daemon.
      </p>
      {written.length > 0 && (
        <p className="os-scope-ok" role="status">
          Saved on {written.map((result) => nameOf(result.host)).join(", ")}.
        </p>
      )}
      {failed.map((result) => (
        <p key={result.host} className="inline-error" role="alert">
          Not saved on {nameOf(result.host)}: {result.error}
        </p>
      ))}
      {render(host || LOCAL_HOST, layer, `${project.id}:${host}:${reload}`)}
    </>
  );
}
