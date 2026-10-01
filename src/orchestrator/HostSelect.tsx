import { useEffect, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  FolderOpen,
  Plus,
  RefreshCw,
  Server,
} from "lucide-react";
import { hostStatus, LOCAL_HOST, type DaemonReach } from "./hosts";
import type { OrchestratorHost } from "./types";
import type { Project, ProjectHostReadiness } from "../types";
import { openProjectSettings } from "../app/openSettings";
import { readPrepareTimes } from "../projectPrepare";

export function RunOnSelect({
  hosts,
  currentHost,
  value,
  readiness,
  project,
  cwd,
  onChange,
}: {
  hosts: OrchestratorHost[];
  currentHost: string;
  value: string;
  readiness: Record<string, ProjectHostReadiness>;
  project?: Project | null;
  /** The folder the project is open in; the link to its settings needs it. */
  cwd?: string;
  onChange(host: string): void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  // The menu starts 12px left of the trigger, within the composer's frame.
  const [menuLeft, setMenuLeft] = useState(0);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);
  const selected = hosts.find((host) => host.id === value);
  const label =
    value === "local" ? "This Mac" : selected?.name || "Choose host";
  return (
    <div className="orch-host-select orch-run-on" ref={rootRef}>
      <button
        type="button"
        className="orch-run-on-toggle"
        aria-label={`Run on: ${label}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(event) => {
          const frame = event.currentTarget.closest(".orch-composer");
          if (frame)
            setMenuLeft(
              event.currentTarget.getBoundingClientRect().left -
                frame.getBoundingClientRect().left -
                12,
            );
          setOpen((current) => !current);
        }}
      >
        {project ? `${project.name} · on ${label}` : `Run on · ${label}`}{" "}
        <ChevronDown size={12} aria-hidden />
      </button>
      {open && (
        <div
          className="orch-host-menu orch-run-on-menu"
          role="menu"
          aria-label="Run on"
          style={{ left: menuLeft }}
        >
          <div className="orch-run-on-title">
            {`Run ${project?.name || "project"} on`}
          </div>
          <button
            type="button"
            role="menuitemradio"
            aria-checked={value === "local"}
            className={`orch-host-option${value === "local" ? " selected" : ""}`}
            disabled={currentHost !== "local"}
            onClick={() => {
              onChange("local");
              setOpen(false);
            }}
          >
            <HostIcon id="local" />
            <span className="orch-host-text">
              <span className="orch-host-name">This Mac</span>
              <span className="orch-host-detail">
                {project
                  ? `ready · ${project.env.length} variables · ${Object.keys((project.mcp as { mcpServers?: object }).mcpServers ?? {}).length} MCP servers`
                  : "checking project"}
              </span>
            </span>
            <span className="ui-dot ui-tone-ok" aria-hidden />
            {value === "local" && (
              <Check size={12} aria-hidden className="orch-host-check" />
            )}
          </button>
          {hosts
            .filter((host) => host.id.startsWith("ssh:"))
            .map((host) => {
              const ready = readiness[host.id];
              const reinstall = ready?.checkout.ok && ready.setup.stale;
              const tone = !ready
                ? "neutral"
                : !ready.checkout.ok
                  ? "danger"
                  : reinstall || ready.withheld
                    ? "warning"
                    : "ok";
              return (
                <button
                  key={host.id}
                  type="button"
                  role="menuitemradio"
                  aria-checked={value === host.id}
                  className={`orch-host-option${value === host.id ? " selected" : ""}`}
                  onClick={() => {
                    onChange(host.id);
                    setOpen(false);
                  }}
                >
                  <HostIcon id={host.id} />
                  <span className="orch-host-text">
                    <span className="orch-host-name">{host.name}</span>
                    <span className="orch-host-detail">
                      {!ready
                        ? host.state === "ready"
                          ? "checking project"
                          : "connect to prepare"
                        : !ready.checkout.ok
                          ? "not cloned · clones and installs"
                          : reinstall
                            ? `reinstalls first — ${ready.setup.lockFile || "lock file"} changed${(() => {
                                const seconds = readPrepareTimes(
                                  project?.id ?? "",
                                  host.id,
                                )?.find(
                                  (step) => step.id === "install",
                                )?.seconds;
                                return seconds === undefined
                                  ? ""
                                  : ` · ~${seconds} s`;
                              })()}`
                            : ready.withheld
                              ? "ready · no secrets sent"
                              : "ready"}
                    </span>
                  </span>
                  <span className={`ui-dot ui-tone-${tone}`} aria-hidden />
                  {value === host.id && (
                    <Check size={12} aria-hidden className="orch-host-check" />
                  )}
                </button>
              );
            })}
          <div className="orch-run-on-foot">
            Same variables, secrets and MCP servers on every host.
            {cwd && (
              <button
                type="button"
                role="menuitem"
                className="orch-run-on-link"
                onClick={() => {
                  setOpen(false);
                  openProjectSettings(
                    cwd,
                    "Hosts",
                    currentHost.startsWith("ssh:") ? currentHost : undefined,
                  );
                }}
              >
                Project settings → Hosts
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

function HostIcon({ id }: { id: string }) {
  return id === LOCAL_HOST ? (
    <FolderOpen size={14} aria-hidden className="orch-host-icon" />
  ) : (
    <Server size={14} aria-hidden className="orch-host-icon" />
  );
}

/** Orch/HostSelect: the host the panel drives, at the top of the rail, with
 * a menu of Local and every SSH connection (their running counts and
 * status), the repo on the current remote host, and a way to add a host. */
export function HostSelect({
  hosts,
  current,
  daemon,
  running,
  repo,
  suggestions,
  onChoose,
  onRepo,
  onRecheck,
  onAddHost,
  onOpen,
}: {
  hosts: OrchestratorHost[];
  current: string;
  /** What the panel body last saw for the current host's daemon. */
  daemon?: DaemonReach;
  /** Running tasks per host id, filled in while the menu is open. */
  running: Record<string, number>;
  /** The repo path on the current remote host. */
  repo: string;
  suggestions: string[];
  onChoose(host: string): void;
  onRepo(repo: string): void;
  onRecheck(): void;
  /** Opens Settings -> Connections; the row is disabled without it. */
  onAddHost?: () => void;
  /** Called when the menu opens, so the counts can be loaded. */
  onOpen(): void;
}) {
  const [open, setOpen] = useState(false);
  const [repoDraft, setRepoDraft] = useState(repo);
  useEffect(() => setRepoDraft(repo), [repo]);
  const rootRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);

  const info = hosts.find((host) => host.id === current) ?? {
    id: current,
    name: current.replace(/^ssh:/, ""),
    state: "idle" as const,
    enabled: false,
  };
  const status = hostStatus(info, daemon);
  const remoteReady = current !== LOCAL_HOST && info.state === "ready";
  /** Saves an edited repo path. Enter also closes the menu; a blur never
   * does - it fires on the mousedown of the Recheck or host row the owner is
   * clicking, and closing then would swallow that click. */
  const commitRepo = (close: boolean) => {
    const next = repoDraft.trim();
    if (next && next !== repo) {
      onRepo(next);
      if (close) setOpen(false);
    }
  };

  function menuDetail(host: OrchestratorHost): string {
    const own = host.id === current ? daemon : undefined;
    const base = hostStatus(host, own);
    const ok = base.tone === "ok";
    const count = running[host.id];
    if (!ok || count === undefined) return base.detail;
    return `${host.id === LOCAL_HOST ? "This Mac" : "SSH"} · ${count} running`;
  }

  return (
    <div
      ref={rootRef}
      className="orch-host-select"
      onKeyDown={(event) => {
        if (event.key === "Escape" && open) {
          event.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        className="orch-host-toggle"
        aria-label={`Orchestrator host: ${info.name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          if (!open) onOpen();
          setOpen(!open);
        }}
      >
        <HostIcon id={current} />
        <span className="orch-host-text">
          <span className="orch-host-name">{info.name}</span>
          <span className="orch-host-detail">{status.detail}</span>
        </span>
        <span className={`ui-dot ui-tone-${status.tone}`} aria-hidden />
        <ChevronDown size={12} aria-hidden className="orch-host-chevron" />
      </button>
      {open && (
        <div className="orch-host-menu" role="menu" aria-label="Hosts">
          {hosts.map((host) => {
            const own = host.id === current ? daemon : undefined;
            const selected = host.id === current;
            return (
              <button
                key={host.id}
                type="button"
                role="menuitemradio"
                aria-checked={selected}
                className={`orch-host-option${selected ? " selected" : ""}`}
                onClick={() => {
                  setOpen(false);
                  if (!selected) onChoose(host.id);
                }}
              >
                <HostIcon id={host.id} />
                <span className="orch-host-text">
                  <span className="orch-host-name">{host.name}</span>
                  <span className="orch-host-detail">{menuDetail(host)}</span>
                </span>
                <span
                  className={`ui-dot ui-tone-${hostStatus(host, own).tone}`}
                  aria-hidden
                />
                {selected && (
                  <Check size={12} aria-hidden className="orch-host-check" />
                )}
              </button>
            );
          })}
          {remoteReady && (
            <div className="orch-host-repo">
              <label className="orch-host-repo-label" htmlFor="orch-host-repo">
                Repo on {info.name}
              </label>
              <input
                id="orch-host-repo"
                className="orch-host-repo-input"
                aria-label={`Repo path on ${info.name}`}
                list="orch-repo-suggestions"
                placeholder="/path/to/repo"
                value={repoDraft}
                onChange={(event) => setRepoDraft(event.target.value)}
                onKeyDown={(event) => event.key === "Enter" && commitRepo(true)}
                onBlur={() => commitRepo(false)}
              />
              <datalist id="orch-repo-suggestions">
                {suggestions.map((path) => (
                  <option key={path} value={path} />
                ))}
              </datalist>
              <button
                type="button"
                role="menuitem"
                className="orch-host-action"
                onClick={() => {
                  setOpen(false);
                  onRecheck();
                }}
              >
                <RefreshCw size={14} aria-hidden />
                <span>Recheck git and the CLIs</span>
              </button>
            </div>
          )}
          <span className="orch-host-divider" />
          <button
            type="button"
            role="menuitem"
            className="orch-host-action"
            disabled={!onAddHost}
            onClick={() => {
              setOpen(false);
              onAddHost?.();
            }}
          >
            <Plus size={14} aria-hidden />
            <span>Add a host in Connections…</span>
          </button>
        </div>
      )}
    </div>
  );
}
