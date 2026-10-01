import { useState, type ReactNode } from "react";
import { ChevronDown, Play, RefreshCw, Server } from "lucide-react";
import { Tag, Toggle } from "./ui";
import { projectSlug, repoSlug } from "../projectPrepare";
import type {
  Project,
  ProjectHostReadiness,
  ProjectPrepareStep,
} from "../types";

export type PrepareFailure = {
  stage: string;
  timedOut?: boolean;
  status?: number;
  message: string;
  steps?: ProjectPrepareStep[];
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

/** What a run of Prepare covers, in order, with the state each step is in. */
function planRows(
  project: Project,
  hostName: string,
  ready: ProjectHostReadiness | undefined,
  steps: ProjectPrepareStep[] | undefined,
  mode: "plan" | "running" | "failed",
  failure?: PrepareFailure,
  detail?: string,
): Row[] {
  const slug = repoSlug(project.git?.url || project.name);
  const path = `~/sushiai/${projectSlug(project.name)}`;
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
  const cloned = ready?.checkout.ok;
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
  const token = tokenName(project);
  const setupOnly = project.env
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
      mark: stateOf("clone"),
      title: cloned ? `Update ${slug}` : `Clone ${slug}`,
      sub:
        mode === "failed" && failure
          ? failure.status === 403
            ? `git clone failed: ${token} has no access to ${slug} (403)`
            : firstLine(failure.message)
          : `${cloned ? "at" : "into"} ${path}, using ${token}`,
      right: seconds("clone"),
    },
  ];
  if (project.setup.install)
    rows.push({
      mark: stateOf("install"),
      title: "Install",
      sub: `${project.setup.install} · ${
        mode === "failed"
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
      sub: project.setup.check,
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
    rows.push(
      {
        mark: "pending",
        title: "Send secrets",
        sub: `${count} values into the daemon's memory · not yet sent`,
      },
      {
        mark: "pending",
        title: "Start the run",
        sub: "Your task starts once every step passes",
      },
    );
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

/** The first-run dialog: what Prepare will do and which secrets it sends. */
export function PrepareConsent({
  project,
  hostName,
  ready,
  remember,
  busy,
  canSkip,
  onRemember,
  onPrepare,
  onSkip,
  onCancel,
  estimates,
}: {
  project: Project;
  hostName: string;
  ready?: ProjectHostReadiness;
  /** What each step took the last time this host was prepared. */
  estimates?: ProjectPrepareStep[];
  remember: boolean;
  busy: boolean;
  /** The checkout is already on the host, so a run can start without it. */
  canSkip: boolean;
  onRemember(value: boolean): void;
  onPrepare(): void;
  onSkip(): void;
  onCancel(): void;
}) {
  const secrets = project.env.filter((entry) => entry.secret && entry.hasValue);
  return (
    <div className="orch-run-on-backdrop" role="presentation">
      <section
        className="orch-trust"
        role="dialog"
        aria-modal="true"
        aria-labelledby="orch-trust-title"
        onKeyDown={(event) => event.key === "Escape" && !busy && onCancel()}
      >
        <div className="orch-trust-eyebrow">
          {`${hostName} · first run of ${project.name}`}
        </div>
        <h2 id="orch-trust-title">
          {`Prepare ${hostName} and send it this project’s secrets?`}
        </h2>
        <p>
          {`While a run is going the values live in the memory of the sushiAI daemon on ${hostName}. They are never written to the repository, shell history or logs.`}
        </p>
        <StepList
          rows={planRows(project, hostName, ready, estimates, "plan")}
        />
        {secrets.length > 0 && (
          <>
            <div className="orch-trust-caption">{`Sent to ${hostName}`}</div>
            <div className="orch-trust-chips">
              {secrets.map((entry) => {
                const only =
                  entry.availableTo?.length === 1 && entry.availableTo[0];
                const note =
                  only === "setup"
                    ? " · setup"
                    : only === "mcp"
                      ? " · MCP"
                      : "";
                return (
                  <Tag
                    key={entry.name}
                    dot={false}
                    tone={note ? "info" : "neutral"}
                  >
                    <span className="orch-trust-chip">{`${entry.name}${note}`}</span>
                  </Tag>
                );
              })}
            </div>
          </>
        )}
        <div className="orch-trust-remember">
          <Toggle
            checked={remember}
            label={`Trust ${hostName} for ${project.name} and don’t ask again`}
            onChange={onRemember}
          />
          <span>{`Trust ${hostName} for ${project.name} and don’t ask again`}</span>
        </div>
        <div className="orch-trust-actions">
          <button
            type="button"
            className="ui-button ghost"
            disabled={busy || !canSkip}
            onClick={onSkip}
          >
            Run without secrets
          </button>
          <button
            type="button"
            className="ui-button primary"
            disabled={busy}
            onClick={onPrepare}
          >
            <Play size={13} aria-hidden />
            {busy ? "Preparing…" : "Prepare and start"}
          </button>
        </div>
        {!canSkip && (
          <p className="orch-trust-reason">{`${project.name} is not on ${hostName} yet, so there is nothing to run without preparing it.`}</p>
        )}
      </section>
    </div>
  );
}

/** The page that replaces the task list while a host is prepared: the steps,
 * and, when one fails, what the owner can do about it. */
export function PrepareProgress({
  project,
  hostName,
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
}: {
  project: Project;
  hostName: string;
  /** `uname -sm` of the host. */
  platform?: string;
  /** The host's SSH address, user@host. */
  address?: string;
  failure: PrepareFailure | null;
  steps?: ProjectPrepareStep[];
  editToken: boolean;
  tokenDraft: string;
  onTokenDraft(value: string): void;
  onRetry(): void;
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
  );
  let extra: ReactNode = null;
  if (failure) {
    extra = (
      <div className="orch-prep-help">
        <div className="orch-prep-caption">What you can do</div>
        <p>
          {failure.timedOut
            ? `${failure.message} Nothing was sent to ${hostName}. Try again; a slow install usually finishes the second time.`
            : failure.status === 403
              ? `Give the token read access to ${slug}, or use ${hostName}’s own git login. Nothing was sent to ${hostName}.`
              : `Nothing was sent to ${hostName}. Fix the step above, then try again.`}
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
          <button type="button" className="ui-button ghost" onClick={onRetry}>
            <RefreshCw size={14} aria-hidden /> Try again
          </button>
          {!failure.timedOut && (
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
      <h2>{`Prepare ${hostName} for ${project.name}`}</h2>
      <p>
        {`Clone, install and connect this project on ${hostName}. Your secrets are sent only after every step passes.`}
      </p>
      <StepList rows={rows} />
      {extra}
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
