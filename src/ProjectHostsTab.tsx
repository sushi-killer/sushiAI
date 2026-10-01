import { useCallback, useEffect, useRef, useState } from "react";
import { Plus, RefreshCw } from "lucide-react";
import type {
  Bridge,
  ConnectionProfile,
  Project,
  ProjectHostReadiness,
} from "./types";
import { Tag, Toggle } from "./orchestrator/ui";
import { GitRecovery } from "./orchestrator/GitRecovery";
import { openSettings } from "./app/openSettings";
import { ProjectPage } from "./ProjectPage";
import {
  checkoutPath,
  rememberPrepareTimes,
  tildePath,
} from "./projectPrepare";
import {
  hostDots,
  localReadiness,
  overridesToText,
  textToOverrides,
  type Dot,
} from "./projectHostView";

type Row = {
  key: string;
  name: string;
  detail: string;
  /** The owner switched sending this project's values to it off. */
  withheld: boolean;
  local: boolean;
};
type Check = ProjectHostReadiness | "checking" | { error: string };
type PrepareFailure = Extract<
  Awaited<ReturnType<Bridge["projectHostPrepare"]>>,
  { ok: false }
>;

const COLUMNS = ["checkout", "setup", "clis", "mcp", "secrets"] as const;
const LABELS: Record<(typeof COLUMNS)[number], string> = {
  checkout: "Checkout",
  setup: "Setup",
  clis: "CLIs",
  mcp: "MCP",
  secrets: "Secrets",
};
const PROBLEM: Record<string, string> = {
  setup: "setup",
  clis: "CLIs",
  mcp: "MCP",
  secrets: "secrets",
};

function StatusDot({ tone }: { tone: Dot }) {
  return tone ? (
    <span className={`ui-dot ui-tone-${tone === "ok" ? "ok" : tone}`} />
  ) : (
    <span className="pd-dash">—</span>
  );
}

export function ProjectHostsTab({
  project,
  hosts,
  cwd,
  endpoint,
  remote,
  onProject,
}: {
  project: Project | null;
  hosts: ConnectionProfile[];
  cwd: string;
  endpoint?: string;
  remote: boolean;
  onProject(project: Project): void;
}) {
  const [checks, setChecks] = useState<Record<string, Check>>({});
  const [selected, setSelected] = useState("");
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [failures, setFailures] = useState<Record<string, PrepareFailure>>({});
  const [system, setSystem] = useState<{
    home: string;
    agents: { name: string; path: string | null }[];
  }>({ home: "", agents: [] });
  const started = useRef("");
  const preparing = useRef(false);

  useEffect(() => {
    window.bridge
      ?.system()
      .then(setSystem)
      .catch(() => {});
  }, []);

  const visible = hosts.filter((host) => !host.hidden);
  const rows: Row[] = project
    ? [
        {
          key: "local",
          name: "This Mac",
          detail: remote ? "Not open on this Mac" : tildePath(cwd, system.home),
          withheld: false,
          local: true,
        },
        ...visible.map((host) => ({
          key: `ssh:${host.id}`,
          name: host.name || host.host,
          detail: host.host,
          withheld: !!project.hosts?.[`ssh:${host.id}`]?.withheld,
          local: false,
        })),
      ]
    : [];
  const current =
    rows.find((row) => row.key === selected) ?? rows[1] ?? rows[0];

  const checkHost = useCallback(
    async (key: string) => {
      if (!window.bridge || !project) return;
      setChecks((all) => ({ ...all, [key]: "checking" }));
      try {
        const matrix = await window.bridge.projectHostCheck(
          project.id,
          key,
          key === endpoint ? cwd : undefined,
        );
        setChecks((all) => ({ ...all, [key]: matrix }));
      } catch (reason) {
        setChecks((all) => ({
          ...all,
          [key]: {
            error: reason instanceof Error ? reason.message : String(reason),
          },
        }));
      }
    },
    [project, endpoint, cwd],
  );
  const checkAll = useCallback(
    () =>
      Promise.all(visible.map((host) => checkHost(`ssh:${host.id}`))).then(
        () => undefined,
      ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [checkHost, hosts],
  );
  // Every host is checked when the tab opens, so the matrix is never blank.
  useEffect(() => {
    if (!project) return;
    const id = `${project.id}:${hosts.map((host) => host.id).join(",")}`;
    if (started.current === id) return;
    started.current = id;
    void checkAll();
  }, [project, hosts, checkAll]);

  async function changeWithheld(row: Row, withheld: boolean) {
    if (!window.bridge || !project) return;
    setBusy(row.key);
    setError("");
    try {
      await window.bridge.projectHostWithhold(project.id, row.key, withheld);
      const next = await window.bridge.projectsGet(project.id);
      if (next) onProject(next);
      await checkHost(row.key);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy("");
    }
  }

  async function prepare(row: Row, gitUrl?: string) {
    if (!window.bridge || !project || busy || preparing.current) return;
    preparing.current = true;
    setSelected(row.key);
    setBusy(row.key);
    setError("");
    try {
      const result = await window.bridge.projectHostPrepare(
        project.id,
        row.key,
        false,
        gitUrl ? { gitUrl } : undefined,
      );
      if (!result.ok) setFailures((all) => ({ ...all, [row.key]: result }));
      else {
        rememberPrepareTimes(project.id, row.key, result.steps);
        setFailures((all) => {
          const next = { ...all };
          delete next[row.key];
          return next;
        });
      }
      await checkHost(row.key);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy("");
      preparing.current = false;
    }
  }

  async function saveOverrides(row: Row) {
    if (!window.bridge || !project) return;
    const text = draft[row.key];
    if (text === undefined) return;
    try {
      await window.bridge.projectHostOverrides(
        project.id,
        row.key,
        textToOverrides(text),
      );
      const next = await window.bridge.projectsGet(project.id);
      if (next) onProject(next);
      setDraft((all) => {
        const rest = { ...all };
        delete rest[row.key];
        return rest;
      });
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  const subtitle =
    "Where this project runs. Each host is checked before a run, so problems show up here, not in a failed task.";
  if (!project)
    return (
      <ProjectPage title="Hosts" subtitle={subtitle}>
        <p className="pd-empty">
          Open a project with a git remote to manage its hosts.
        </p>
      </ProjectPage>
    );
  const sendingCount = rows.filter((row) => !row.withheld).length;
  const notes = rows.flatMap((row) => {
    const check = checks[row.key];
    if (!check || check === "checking" || "error" in check) return [];
    const dots = hostDots({ ...check, withheld: row.withheld });
    if (dots.checkout === "danger")
      return [`${row.name}: not cloned yet — Prepare clones it and installs.`];
    const missing = check.mcp.missing ?? [];
    return [
      dots.setup === "warning" &&
        check.setup.stale &&
        `${row.name}: ${check.setup.lockFile || "the lock file"} changed since the last install — it reinstalls on the next run.`,
      dots.clis === "danger" && `${row.name}: no Claude or Codex CLI found.`,
      dots.mcp === "warning" &&
        (missing.length
          ? `${row.name}: ${[...new Set(missing.map((item) => item.command))].join(", ")} is missing, so the ${missing.map((item) => item.name).join(", ")} ${missing.length === 1 ? "server is" : "servers are"} skipped there.`
          : `${row.name}: some MCP servers cannot start.`),
      dots.secrets === "warning" &&
        row.withheld &&
        `${row.name}: no secrets are sent to it.`,
    ].filter((line): line is string => !!line);
  });
  const overrides =
    current && !current.local
      ? (project.hosts?.[current.key]?.overrides ?? {})
      : {};
  const failure = current ? failures[current.key] : undefined;

  return (
    <ProjectPage title="Hosts" subtitle={subtitle}>
      <div className="pd-toolbar">
        <span>
          {`${rows.length} ${rows.length === 1 ? "host" : "hosts"} · ${sendingCount} get secrets`}
        </span>
        <button
          className="ui-button secondary"
          disabled={!!busy || visible.length === 0}
          onClick={() => void checkAll()}
        >
          <RefreshCw size={14} aria-hidden /> Check all
        </button>
        <button
          className="ui-button primary"
          onClick={() => openSettings("connections")}
        >
          <Plus size={14} aria-hidden /> Add host
        </button>
      </div>
      {error && (
        <p role="alert" className="pd-alert">
          {error}
        </p>
      )}
      <div
        className="pd-table pd-hosts"
        role="table"
        aria-label="Project hosts"
      >
        <div className="pd-row head" role="row">
          <span role="columnheader">Host</span>
          {COLUMNS.map((column) => (
            <span key={column} className="pd-col" role="columnheader">
              {LABELS[column]}
            </span>
          ))}
          <span />
        </div>
        {rows.map((row) => {
          const local = row.local
            ? localReadiness(project, system.agents, cwd)
            : undefined;
          const check = row.local ? local : checks[row.key];
          const matrix =
            check && check !== "checking" && !("error" in check)
              ? check
              : undefined;
          const dots = matrix
            ? hostDots({ ...matrix, withheld: row.withheld })
            : undefined;
          const missing = !!dots && dots.checkout === "danger";
          return (
            <div
              key={row.key}
              className={`pd-row host${current?.key === row.key ? " selected" : ""}`}
              role="row"
              aria-selected={current?.key === row.key}
              tabIndex={0}
              aria-label={row.name}
              onClick={() => setSelected(row.key)}
              onKeyDown={(event) => {
                if (event.key === "Enter") setSelected(row.key);
              }}
            >
              <div className="pd-host-name" role="cell">
                <div>
                  <strong>{row.name}</strong>
                  <Tag tone={row.withheld ? "neutral" : "ok"} dot={false}>
                    {row.withheld ? "no secrets" : "gets secrets"}
                  </Tag>
                </div>
                <small>
                  {row.local || !matrix || !matrix.checkout.ok
                    ? row.detail
                    : `${row.detail} · ${checkoutPath(matrix)}`}
                  {check === "checking"
                    ? " · checking"
                    : check && "error" in check
                      ? ` · ${check.error}`
                      : missing
                        ? " · not cloned yet"
                        : ""}
                </small>
              </div>
              {COLUMNS.map((column) => (
                <span key={column} className="pd-col" role="cell">
                  <StatusDot tone={dots ? dots[column] : null} />
                </span>
              ))}
              <span className="pd-col end" role="cell">
                {!dots ? (
                  <Tag tone="neutral" dot={false}>
                    {check === "checking" ? "checking" : "unreachable"}
                  </Tag>
                ) : missing ? (
                  <button
                    className="ui-button secondary"
                    disabled={!!busy}
                    onClick={(event) => {
                      event.stopPropagation();
                      void prepare(row);
                    }}
                  >
                    {busy === row.key ? "Preparing…" : "Prepare"}
                  </button>
                ) : dots.problems.length ? (
                  <Tag tone="warning">
                    {dots.problems[0] === "setup" &&
                    typeof check === "object" &&
                    "setup" in check &&
                    check.setup.stale
                      ? "reinstall"
                      : PROBLEM[dots.problems[0]]}
                  </Tag>
                ) : (
                  <Tag tone="ok">ready</Tag>
                )}
              </span>
            </div>
          );
        })}
      </div>
      {rows.length === 1 && (
        <p className="pd-note">No SSH hosts are configured.</p>
      )}
      {notes.length > 0 && <p className="pd-note">{notes.join("  ")}</p>}
      {current && !current.local && failure && (
        <>
          <p role="alert" className="pd-alert">
            {failure.message}
          </p>
          {failure.git &&
            ["auth", "network", "host-key"].includes(failure.git.kind) && (
              <GitRecovery
                key={`${project.id}:${current.key}:${failure.git.url}`}
                projectId={project.id}
                endpoint={current.key}
                failure={failure.git}
                disabled={!!busy}
                onRetry={(gitUrl) => void prepare(current, gitUrl)}
              />
            )}
          <button
            className="ui-button secondary"
            disabled={!!busy}
            onClick={() =>
              void prepare(
                current,
                failure.git?.transport === "ssh" ? failure.git.url : undefined,
              )
            }
          >
            {busy === current.key ? "Preparing…" : "Retry preparation"}
          </button>
        </>
      )}
      {current && !current.local && (
        <>
          <h3 className="pd-group-label">{`Overrides · ${current.name}`}</h3>
          <div className="pd-overrides-box">
            <div className="pd-overrides-hint">
              {`# only on ${current.name}; everything else comes from Environment`}
            </div>
            <textarea
              className="pd-overrides"
              aria-label={`Overrides for ${current.name}`}
              spellCheck={false}
              rows={Math.max(
                1,
                (draft[current.key] ?? overridesToText(overrides)).split("\n")
                  .length,
              )}
              value={draft[current.key] ?? overridesToText(overrides)}
              onChange={(event) =>
                setDraft((all) => ({
                  ...all,
                  [current.key]: event.target.value,
                }))
              }
              onBlur={() => void saveOverrides(current)}
            />
          </div>
          {!current.local && (
            <div className="pd-withhold">
              <p>
                {`Hosts you add get this project’s secrets while a run is going. Switch this on to keep them off ${current.name}; it applies when sushiAI is connected to ${current.name}.`}
              </p>
              <Toggle
                checked={current.withheld}
                label="Don’t send secrets to this host"
                onChange={(value) => void changeWithheld(current, value)}
              />
              <span>Don’t send secrets to this host</span>
            </div>
          )}
        </>
      )}
    </ProjectPage>
  );
}
