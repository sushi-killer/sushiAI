// Pure logic behind the Orchestrator's host selector: which host an event or
// task belongs to, what a host's preflight rules out, and which repo paths to
// suggest on a host. No React, no bridge.
import type { Workspace } from "../types.ts";
import { planDrafts } from "./helpers.ts";
import { unstartedDraft } from "./planModel.ts";
import type { OrchestratorHost, Preflight, Route, Task } from "./types.ts";

export const LOCAL_HOST = "local";

/** The host a task, event or notice belongs to; local when it names none. */
export function hostOf(item: { host?: string } | null | undefined): string {
  return item?.host || LOCAL_HOST;
}

/** Why a host cannot run a route's harness, or null when it can (or when
 * nothing is known yet - a host that was never probed rules nothing out). */
export function routeProblem(
  preflight: Preflight | null | undefined,
  route: Pick<Route, "harness">,
): string | null {
  const state = preflight?.[route.harness];
  if (!state) return null;
  if (!state.installed) return `${route.harness} is not installed on this host`;
  if (!state.loggedIn) return `${route.harness} is not logged in on this host`;
  return null;
}

/** The routes a host cannot run, id -> reason. */
export function unavailableRoutes(
  preflight: Preflight | null | undefined,
  routes: Route[],
): Record<string, string> {
  const result: Record<string, string> = {};
  for (const route of routes) {
    const problem = routeProblem(preflight, route);
    if (problem) result[route.id] = problem;
  }
  return result;
}

export function hostName(hosts: OrchestratorHost[], id: string): string {
  return hosts.find((host) => host.id === id)?.name ?? id.replace(/^ssh:/, "");
}

/** Repo paths worth offering on a host: those of the workspaces opened on it,
 * then those of its existing tasks - each once, most recent first. */
export function repoSuggestions(
  host: string,
  workspaces: Pick<Workspace, "cwd" | "connection">[],
  tasks: Pick<Task, "repo" | "updatedAt">[],
): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const add = (repo: string | undefined) => {
    if (repo && !seen.has(repo)) {
      seen.add(repo);
      out.push(repo);
    }
  };
  for (const workspace of workspaces)
    if ((workspace.connection || LOCAL_HOST) === host) add(workspace.cwd);
  for (const task of [...tasks].sort((a, b) => b.updatedAt - a.updatedAt))
    add(task.repo);
  return out;
}

/** Where orchd lives on a host, as the app installs it
 * (electron/orchestrator-remote.cjs). */
const REMOTE_BINARY = "~/.sushiai/bin/orchd";
const REMOTE_DATA = "~/.sushiai/orchestrator";

export type HostTone = "ok" | "warning" | "danger" | "neutral";

/** The daemon state the panel body last saw for the host it shows. */
export type DaemonReach = "loading" | "ready" | "not-built" | "unavailable";

/** The platform (`uname -sm`) a setup error names, e.g. "Linux aarch64". */
export function hostPlatform(detail: string | undefined): string | null {
  return /\(((?:Linux|Darwin|FreeBSD)[^)]*)\)/.exec(detail ?? "")?.[1] ?? null;
}

/** The rustup command a "needs Rust" error carries, to copy as-is. */
export function rustupCommand(detail: string | undefined): string | null {
  return (
    /Install it on the host with: (.+?) - then connect again/.exec(
      detail ?? "",
    )?.[1] ?? null
  );
}

const SETTING_UP = new Set<OrchestratorHost["state"]>([
  "connecting",
  "installing",
  "building",
  "starting",
]);

/** A remote host is still being brought up (or failed to be): the panel
 * shows the setup card instead of its tasks. A host that answered once in
 * this panel keeps its tasks behind the offline banner while the app
 * reconnects it. */
export function needsSetup(
  host: Pick<OrchestratorHost, "id" | "state">,
  answeredOnce: boolean,
): boolean {
  if (host.id === LOCAL_HOST || answeredOnce) return false;
  return host.state !== "ready";
}

/** The host as the panel shows it: while the main process retries a host
 * that failed on its own, its failure stays up instead of flickering back
 * to "connecting" on every retry. */
export function shownHost<T extends Pick<OrchestratorHost, "state" | "detail">>(
  host: T,
  lastError: string,
): T {
  return lastError && (host.state === "connecting" || host.state === "idle")
    ? { ...host, state: "error", detail: lastError }
    : host;
}

/** The status dot and the second line of the host selector. `daemon` is
 * what the panel body saw for the host it shows (undefined for others). */
export function hostStatus(
  host: Pick<OrchestratorHost, "id" | "state" | "detail" | "enabled">,
  daemon?: DaemonReach,
): { tone: HostTone; detail: string } {
  const local = host.id === LOCAL_HOST;
  const where = local ? "This Mac" : "SSH";
  if (daemon === "not-built")
    return { tone: "warning", detail: `${where} · orchd not built` };
  if (daemon === "unavailable")
    return { tone: "danger", detail: `${where} · orchd unreachable` };
  if (local)
    return daemon === "loading"
      ? { tone: "warning", detail: "This Mac · connecting" }
      : { tone: "ok", detail: "This Mac · connected" };
  if (SETTING_UP.has(host.state))
    return { tone: "warning", detail: "SSH · setting up orchd" };
  if (host.state === "error")
    return rustupCommand(host.detail)
      ? { tone: "warning", detail: "SSH · orchd needs Rust" }
      : { tone: "danger", detail: "SSH · orchd unreachable" };
  if (host.state === "ready")
    return { tone: "ok", detail: "SSH · orchd connected" };
  return {
    tone: "neutral",
    detail: host.enabled ? "SSH · connecting" : "SSH · not set up",
  };
}

export type SetupStepState = "done" | "active" | "failed" | "pending";
export type SetupStep = {
  title: string;
  detail: string;
  state: SetupStepState;
  /** A command the owner runs on the host to get past a failed step. */
  command?: string;
  /** A button that gets past the failed step without the owner touching the
   * host: it runs the one-button setup. */
  action?: { label: string; hint: string };
};

/** What the setup card knows beyond the host record: the states this panel
 * saw it pass through, since when it is in the current one, and the SSH
 * address of its Connections profile. */
export type SetupSeen = {
  /** States seen, oldest first; the last is the current one. */
  states: OrchestratorHost["state"][];
  /** Milliseconds the host has been in its current state. */
  elapsedMs: number;
  address?: string;
};

/** Whether a failure is the one-button Rust install failing (its message
 * carries the tail of the host's output). */
function rustInstallFailed(detail: string): boolean {
  return /^Installing Rust on .+ failed:/.test(detail);
}

/** What the host still lacks for a Rust build: the C linker's package. */
export function buildToolsHint(platform: string | null | undefined): string {
  return /^Darwin/.test(platform ?? "")
    ? "Rust needs a C linker: run xcode-select --install on the host."
    : "Rust needs a C linker: install build-essential (Debian, Ubuntu) or gcc (Fedora, Arch) on the host.";
}

function elapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  return `${Math.floor(s / 60)}m ${s % 60}s`;
}

/** Which of the five steps a failure belongs to, from its message and the
 * last state the host was in before it failed. */
function failedStep(
  detail: string,
  before: OrchestratorHost["state"] | undefined,
): number {
  if (
    rustupCommand(detail) ||
    rustInstallFailed(detail) ||
    /no orchd source to build/.test(detail)
  )
    return 2;
  if (/did not answer/.test(detail)) return 4;
  if (before === "installing" || before === "building") return 2;
  if (before === "starting") return 3;
  if (/home directory/.test(detail)) return 1;
  return 0;
}

/** The five-step setup card (Figma "Home · remote setup"): connect over SSH,
 * check for orchd, upload or build it, start the daemon, connect to it. */
export function setupSteps(
  host: Pick<
    OrchestratorHost,
    "state" | "detail" | "platform" | "orchdInstalled"
  > &
    Partial<Pick<OrchestratorHost, "name">>,
  seen: SetupSeen,
): SetupStep[] {
  const detail = host.detail ?? "";
  const platform = host.platform || hostPlatform(detail);
  const machine = [platform, seen.address].filter(Boolean).join(" · ");
  const built = seen.states.includes("building");
  const uploaded = seen.states.includes("installing");
  const before = seen.states.filter((s) => s !== "error").at(-1);
  const time = elapsed(seen.elapsedMs);

  let current: number;
  let failed = false;
  switch (host.state) {
    case "connecting":
      current = 0;
      break;
    case "installing":
    case "building":
      current = 2;
      break;
    case "starting":
      current = 3;
      break;
    case "ready":
      current = 5;
      break;
    case "error":
      current = failedStep(detail, before);
      failed = true;
      break;
    default:
      current = 0;
  }
  const state = (index: number): SetupStepState =>
    index < current
      ? "done"
      : index > current
        ? "pending"
        : failed
          ? "failed"
          : "active";

  const install: SetupStep =
    host.state === "error" && current === 2
      ? rustupCommand(detail)
        ? {
            title: "Can’t build orchd here",
            detail: `no matching build for ${platform ?? "this host"}, and cargo isn’t installed`,
            state: "failed",
            command: rustupCommand(detail) ?? undefined,
            action: {
              label: "Install Rust and set up",
              hint: `Installs a minimal Rust toolchain in ~/.cargo on ${host.name ?? "the host"} (no sudo), then builds and starts the orchestrator.`,
            },
          }
        : rustInstallFailed(detail)
          ? {
              title: "Couldn’t install Rust",
              detail: detail.replace(/^Installing Rust on .+? failed: /, ""),
              state: "failed",
              action: {
                label: "Retry",
                hint: `Installs a minimal Rust toolchain in ~/.cargo on ${host.name ?? "the host"} (no sudo), then builds and starts the orchestrator.`,
              },
            }
          : { title: "Can’t install orchd here", detail, state: "failed" }
      : host.state === "building" && /^Installing Rust/.test(detail)
        ? {
            title: "Installing Rust…",
            detail: ["minimal toolchain in ~/.cargo", time]
              .filter(Boolean)
              .join(" · "),
            state: state(2),
          }
        : host.state === "building" || (built && current > 2)
          ? {
              title:
                host.state === "building"
                  ? "Building orchd from source"
                  : "Built orchd from source",
              detail: [
                `no matching build for ${platform ?? "this host"}`,
                "cargo build --release",
                host.state === "building" ? time : "",
              ]
                .filter(Boolean)
                .join(" · "),
              state: state(2),
            }
          : host.state === "installing" || (uploaded && current > 2)
            ? {
                title:
                  host.state === "installing"
                    ? "Uploading orchd"
                    : "Uploaded orchd",
                detail: [
                  "the build from this Mac",
                  host.state === "installing" ? time : "",
                ]
                  .filter(Boolean)
                  .join(" · "),
                state: state(2),
              }
            : {
                title: "Upload or build orchd",
                detail:
                  current > 2
                    ? "already up to date"
                    : "this Mac’s build, or from source on the host",
                state: state(2),
              };

  const failure = (index: number) =>
    failed && current === index ? detail : "";
  return [
    {
      title: current > 0 ? "Connected over SSH" : "Connecting over SSH",
      detail: failure(0) || machine || "over your Connections profile",
      state: state(0),
    },
    {
      title: current > 1 ? "Checked for orchd" : "Check for orchd",
      detail:
        failure(1) ||
        (built || uploaded || (failed && current === 2)
          ? host.orchdInstalled === false
            ? `not installed at ${REMOTE_BINARY}`
            : `no current build at ${REMOTE_BINARY}`
          : current > 2
            ? `found at ${REMOTE_BINARY}`
            : `looks in ${REMOTE_BINARY}`),
      state: state(1),
    },
    install,
    {
      title: "Start the daemon",
      detail: failure(3) || `detached, data in ${REMOTE_DATA}`,
      state: state(3),
    },
    {
      title: "Connect",
      detail: failure(4) || "forward orchd.sock, read the control token",
      state: state(4),
    },
  ];
}

export type PreflightItem = {
  name: "git" | "claude" | "codex";
  ok: boolean;
  note: string;
};

/** The preflight strip: git, then each harness CLI with its login state. */
export function preflightItems(preflight: Preflight): PreflightItem[] {
  const harness = (name: "claude" | "codex"): PreflightItem => {
    const state = preflight[name];
    return {
      name,
      ok: state.installed && state.loggedIn,
      note: !state.installed
        ? "not installed"
        : state.loggedIn
          ? "logged in"
          : "not logged in",
    };
  };
  return [
    { name: "git", ok: preflight.git, note: preflight.git ? "" : "missing" },
    harness("claude"),
    harness("codex"),
  ];
}

/** What a host missing a harness means: orchd falls back to an installed CLI
 * for those routes. "codex is not installed on lab; its routes fall back to an
 * installed CLI."; empty when every route can run. */
export function routesFallbackNote(
  preflight: Preflight,
  hostName: string,
): string {
  return (["claude", "codex"] as const)
    .filter((name) => !(preflight[name].installed && preflight[name].loggedIn))
    .map(
      (name) =>
        `${name} is not ${preflight[name].installed ? "logged in" : "installed"} on ${hostName}; its routes fall back to an installed CLI.`,
    )
    .join(" ");
}

/** Tasks a host counts as running in the selector: started work, not the
 * drafts that wait on the Plan. */
export function runningCount(tasks: Task[]): number {
  const drafts = new Set(planDrafts(tasks).filter(unstartedDraft));
  return tasks.filter(
    (task) =>
      !task.archived &&
      !drafts.has(task) &&
      (task.status === "running" ||
        task.status === "drafting" ||
        task.status === "queued"),
  ).length;
}
