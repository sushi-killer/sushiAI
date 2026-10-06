import { useState, type ReactNode } from "react";
import { ChevronDown, RefreshCw, Server } from "lucide-react";
import { projectSlug, repoSlug } from "../projectPrepare";
import { advice, cloneFailure } from "./prepareCopy";
import { GitRecovery } from "./GitRecovery";
import type {
  Project,
  ProjectHostReadiness,
  ProjectPrepareStep,
  ProjectGitFailure,
} from "../types";

export type PrepareFailure = {
  stage: string;
  timedOut?: boolean;
  status?: number;
  message: string;
  steps?: ProjectPrepareStep[];
  git?: ProjectGitFailure;
};

type Mark = "done" | "active" | "failed" | "pending";
type Row = {
  mark: Mark;
  title: string;
  sub: string;
  right?: string;
  /** More to read behind a Details button. */
  more?: string;
};

const clock = (seconds: number) =>
  seconds < 60 ? `${seconds} s` : `${Math.round(seconds / 60)} min`;

function StepList({ rows }: { rows: Row[] }) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <div className="orch-prep-steps">
      {rows.map((row) => (
        <div className={`orch-prep-step ${row.mark}`} key={row.title}>
          <span className="orch-prep-mark" aria-hidden />
          <span className="orch-prep-text">
            <span className="orch-prep-title">{row.title}</span>
            <span className="orch-prep-sub">
              {open === row.title && row.more ? row.more : row.sub}
            </span>
          </span>
          {row.right && <span className="orch-prep-right">{row.right}</span>}
          {row.more && (
            <button
              type="button"
              className="ui-button ghost"
              aria-expanded={open === row.title}
              onClick={() => setOpen(open === row.title ? null : row.title)}
            >
              Details
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

/** The variable the clone uses, or `null` when the host's own git login does. */
export function gitTokenName(project: Project): string | null {
  return (
    ["GIT_TOKEN", "GITHUB_TOKEN"].find((name) =>
      project.env.some((entry) => entry.name === name),
    ) ?? null
  );
}
const tokenName = (project: Project) =>
  gitTokenName(project) ?? "the host's git login";

/** What the install sees besides the clone token: secrets
 * by name, everything else only counted. */
function setupSeen(project: Project): { secrets: string[]; others: number } {
  const token = gitTokenName(project);
  const seen = project.env.filter(
    (entry) =>
      entry.hasValue &&
      entry.name !== token &&
      (entry.availableTo ?? ["setup", "agent"]).includes("setup"),
  );
  return {
    secrets: seen.filter((entry) => entry.secret).map((entry) => entry.name),
    others: seen.filter((entry) => !entry.secret).length,
  };
}

/** What a run of Prepare covers, in order, with the state each step is in. */
function planRows(
  project: Project,
  hostName: string,
  ready: ProjectHostReadiness | undefined,
  steps: ProjectPrepareStep[] | undefined,
  mode: "plan" | "running" | "failed",
  failure?: PrepareFailure,
  detail?: string,
  options: {
    /** The host is switched off for this project: nothing is sent and the
     * clone uses the host's own git login. */
    noSecrets?: boolean;
    /** A session start (the + picker), not a task run. */
    session?: boolean;
    /** The checkout is already on the host: nothing is cloned. */
    found?: boolean;
  } = {},
): Row[] {
  const slug = repoSlug(project.git?.url || project.name);
  const path = `~/sushiai/${project.slug || projectSlug(project.name)}`;
  const stateOf = (id: ProjectPrepareStep["id"]): Mark => {
    if (mode === "plan") return "pending";
    const step = steps?.find((item) => item.id === id);
    if (step?.state === "done") return "done";
    if (step?.state === "failed") return "failed";
    if (mode !== "running") return "pending";
    const firstPending = steps?.find((item) => item.state === "pending");
    return firstPending?.id === id || (!steps && id === "clone")
      ? "active"
      : "pending";
  };
  const seconds = (id: ProjectPrepareStep["id"]) => {
    const value = steps?.find((item) => item.id === id)?.seconds;
    // A plan shows how long each step took last time, as an estimate.
    return value === undefined
      ? undefined
      : `${mode === "plan" ? "~" : ""}${clock(value)}`;
  };
  const cloned = ready?.checkout.ok || options.found;
  const clis = ready?.clis;
  const connected: Row = {
    mark: "done",
    title: mode === "plan" ? `Connected over SSH` : "Connected over SSH",
    sub:
      mode === "plan" && clis
        ? [
            clis.git ? "git found" : "git missing",
            clis.claude.installed
              ? clis.claude.loggedIn
                ? "claude logged in"
                : "claude not logged in"
              : "claude not installed",
            clis.codex.installed ? "codex found" : "codex not installed",
          ].join(" · ")
        : detail || hostName,
    right: mode === "plan" ? "checked" : undefined,
  };
  const token = options.noSecrets
    ? `${hostName}’s git login`
    : tokenName(project);
  const setupOnly = options.noSecrets
    ? []
    : project.env
        .filter(
          (entry) =>
            entry.secret &&
            entry.hasValue &&
            entry.availableTo?.length === 1 &&
            entry.availableTo[0] === "setup",
        )
        .map((entry) => entry.name);
  const rows: Row[] = [
    connected,
    {
      mark: options.found ? "done" : stateOf("clone"),
      title: options.found
        ? "Checkout found"
        : cloned
          ? `Update ${slug}`
          : `Clone ${slug}`,
      sub:
        mode === "failed" && failure && stateOf("clone") === "failed"
          ? (cloneFailure({
              hostName,
              repo: slug,
              token: gitTokenName(project),
              noSecrets: !!options.noSecrets,
              status: failure.status,
              message: failure.message,
              git: failure.git,
            }) ?? firstLine(failure.message))
          : options.found
            ? `at ${path} · nothing to clone`
            : `${cloned ? "at" : "into"} ${path}, using ${token}`,
      right: seconds("clone"),
      more:
        failure?.git && stateOf("clone") === "failed"
          ? failure.message
              .split("\n")
              .filter((line) => !line.startsWith("SUSHIAI_"))
              .join("\n")
              .trim()
          : undefined,
    },
  ];
  if (project.setup.install)
    rows.push({
      mark: stateOf("install"),
      title: "Install",
      sub:
        stateOf("install") === "failed" && failure
          ? firstLine(failure.message)
          : `${project.setup.install} · ${
              mode === "failed" && stateOf("clone") !== "done"
                ? "waits for the clone"
                : setupOnly.length
                  ? `uses ${setupOnly.join(", ")} (setup only)`
                  : "runs when the lock file changes"
            }`,
      right: seconds("install"),
    });
  if (project.setup.check)
    rows.push({
      mark: stateOf("check"),
      title: "Check",
      sub:
        stateOf("check") === "failed" && failure
          ? firstLine(failure.message)
          : project.setup.check,
      right: seconds("check"),
    });
  const missing = ready?.mcp.missing ?? [];
  if (mode === "plan" && missing.length)
    rows.push({
      mark: "active",
      title:
        missing.length === 1
          ? `${missing[0].name} MCP server is skipped`
          : `${missing.length} MCP servers are skipped`,
      sub: `${[...new Set(missing.map((item) => item.command))].join(", ")} is not installed on ${hostName}`,
      more: `Install ${[...new Set(missing.map((item) => item.command))].join(", ")} on ${hostName} to use ${missing.map((item) => item.name).join(", ")}. The run starts without ${missing.length === 1 ? "it" : "them"}.`,
    });
  if (mode !== "plan") {
    const count = project.env.filter((e) => e.secret && e.hasValue).length;
    // A prepare without secrets sends none, so it has no such step; a
    // session start is not a task run.
    if (!options.noSecrets)
      rows.push({
        mark: "pending",
        title: "Send secrets",
        sub: `${count} values into the daemon's memory · not yet sent`,
      });
    if (!options.session)
      rows.push({
        mark: "pending",
        title: "Start the run",
        sub: "Your task starts once every step passes",
      });
  }
  return rows;
}

function firstLine(message: string): string {
  const line = message
    .split("\n")
    .map((part) => part.trim())
    .filter((part) => part && !part.startsWith("SUSHIAI_"))
    .at(-1);
  return line || "Preparing the host failed.";
}

/** The page that replaces the task list while a host is prepared: the steps,
 * and, when one fails, what the owner can do about it. */
export function PrepareProgress({
  project,
  hostName,
  endpoint,
  platform,
  address,
  failure,
  steps,
  editToken,
  tokenDraft,
  onTokenDraft,
  onRetry,
  onHostLogin,
  onEditToken,
  onSaveToken,
  onCancel,
  noSecrets = false,
  session = false,
  found = false,
  eyebrow,
}: {
  project: Project;
  hostName: string;
  endpoint?: string;
  /** The host is switched off for this project: nothing was sent. */
  noSecrets?: boolean;
  /** The + picker's session start: no task-run rows. */
  session?: boolean;
  /** The checkout is already on the host (only the install is missing). */
  found?: boolean;
  /** A line above the title (host and project). */
  eyebrow?: string;
  /** `uname -sm` of the host. */
  platform?: string;
  /** The host's SSH address, user@host. */
  address?: string;
  failure: PrepareFailure | null;
  steps?: ProjectPrepareStep[];
  editToken: boolean;
  tokenDraft: string;
  onTokenDraft(value: string): void;
  onRetry(gitUrl?: string): void;
  onHostLogin(): void;
  onEditToken(): void;
  onSaveToken(): void;
  onCancel(): void;
}) {
  const slug = repoSlug(project.git?.url || project.name);
  const token = tokenName(project);
  const hasToken = gitTokenName(project) !== null;
  const rows = planRows(
    project,
    hostName,
    undefined,
    failure?.steps ?? steps,
    failure ? "failed" : "running",
    failure ?? undefined,
    [platform, address].filter(Boolean).join(" · "),
    { noSecrets, session, found },
  );
  let extra: ReactNode = null;
  if (failure) {
    extra = (
      <div className="orch-prep-help">
        <div className="orch-prep-caption">What you can do</div>
        <p>
          {advice({
            hostName,
            repo: slug,
            token: gitTokenName(project),
            noSecrets,
            status: failure.status,
            timedOut: failure.timedOut,
            message: failure.message,
            stage: failure.stage,
            setupSecrets: setupSeen(project).secrets,
            setupOthers: setupSeen(project).others,
            git: failure.git,
          })}
        </p>
        {editToken && (
          <input
            className="orch-prep-token"
            type="password"
            autoComplete="new-password"
            aria-label="Git token"
            placeholder="Paste a token with read access"
            value={tokenDraft}
            onChange={(event) => onTokenDraft(event.target.value)}
          />
        )}
        <div className="orch-prep-actions">
          <button
            type="button"
            className="ui-button ghost"
            onClick={() =>
              onRetry(
                failure.git?.transport === "ssh" ? failure.git.url : undefined,
              )
            }
          >
            <RefreshCw size={14} aria-hidden /> Try again
          </button>
          {failure.stage === "clone" && !failure.timedOut && !noSecrets && (
            <>
              <button
                type="button"
                className="ui-button secondary"
                onClick={onHostLogin}
              >
                {`Use ${hostName}’s git login`}
              </button>
              {editToken ? (
                <button
                  type="button"
                  className="ui-button ghost"
                  disabled={!tokenDraft.trim()}
                  onClick={onSaveToken}
                >
                  Save token and try again
                </button>
              ) : (
                <button
                  type="button"
                  className="ui-button ghost"
                  onClick={onEditToken}
                >
                  {hasToken ? `Edit ${token}` : "Add a git token"}
                </button>
              )}
            </>
          )}
          <button type="button" className="ui-button ghost" onClick={onCancel}>
            Cancel
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="orch-prep" role={failure ? "alert" : "status"}>
      {eyebrow && <div className="orch-prep-eyebrow">{eyebrow}</div>}
      <h2>{`Prepare ${hostName} for ${project.name}`}</h2>
      <p>
        {`Clone, install and connect this project on ${hostName}. The daemon gets secrets only after every step passes.`}
      </p>
      <StepList rows={rows} />
      {extra}
      {failure?.stage === "clone" &&
        failure.git &&
        endpoint &&
        ["auth", "host-key", "network"].includes(failure.git.kind) && (
          <GitRecovery
            key={`${project.id}:${endpoint}:${failure.git.url}`}
            projectId={project.id}
            endpoint={endpoint}
            failure={failure.git}
            onRetry={onRetry}
          />
        )}
    </div>
  );
}

/** The rail's host card while a host is being prepared: the host the owner
 * is waiting on, not the one the panel normally drives. */
export function PreparingHost({ name }: { name: string }) {
  return (
    <div className="orch-host-select">
      <div className="orch-host-toggle" role="status">
        <Server size={14} aria-hidden className="orch-host-icon" />
        <span className="orch-host-text">
          <span className="orch-host-name">{name}</span>
          <span className="orch-host-detail">SSH · preparing</span>
        </span>
        <span className="ui-dot ui-tone-warning" aria-hidden />
        <ChevronDown size={12} aria-hidden className="orch-host-chevron" />
      </div>
    </div>
  );
}
