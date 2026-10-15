import { ROUTINES_EXTENSION_ID } from "./id.ts";
import type {
  BuilderSettings,
  Routine,
  RoutineDetail,
  RoutineEntry,
  RoutinesCatalog,
  RunResult,
  RunSummary,
  ValidationReport,
} from "./types.ts";

function invoke<T>(method: string, ...args: unknown[]): Promise<T> {
  if (!window.bridge)
    return Promise.reject(new Error("Open the desktop app first."));
  return window.bridge.moduleInvoke(
    ROUTINES_EXTENSION_ID,
    method,
    ...args,
  ) as Promise<T>;
}

/** One headless builder turn. */
export type BuilderTurn = {
  state: "running" | "done" | "failed";
  text: string;
  agentSession: string;
};
export type BuilderStart = {
  host: string;
  sessionHost: string;
  sessionId: string;
  harness: string;
  turn: BuilderTurn;
};

export type Unreachable = { host: string; name: string; error: string };

/** The typed client of the Routines module (electron/routines.cjs). */
export const routinesClient = {
  catalog: () => invoke<RoutinesCatalog>("catalog"),
  list: () =>
    invoke<{ routines: RoutineEntry[]; unreachable: Unreachable[] }>("list"),
  show: (host: string, id: string) => invoke<RoutineDetail>("show", host, id),
  runs: (host: string, id: string) =>
    invoke<{ runs: RunSummary[] }>("runs", host, id),
  result: (host: string, id: string, run?: number) =>
    invoke<{ run: RunSummary; result: RunResult | null }>(
      "result",
      host,
      id,
      run,
    ),
  validate: (host: string, payload: SavePayload) =>
    invoke<ValidationReport>("validate", host, payload),
  save: (host: string, payload: SavePayload) =>
    invoke<{ id: string; hash: string; enabled: boolean }>(
      "save",
      host,
      payload,
    ),
  setEnabled: (host: string, id: string, enabled: boolean) =>
    invoke<{ id: string; enabled: boolean }>("setEnabled", host, id, enabled),
  runNow: (host: string, id: string) =>
    invoke<{ session?: string; skipped?: string }>("runNow", host, id),
  dryRun: (host: string, id: string) =>
    invoke<{ run: number; session: string }>("dryRun", host, id),
  remove: (host: string, id: string) =>
    invoke<{ id: string }>("delete", host, id),
  move: (from: string, to: string, id: string) =>
    invoke<{ host: string; id: string }>("move", from, to, id),
  /** The owner's yes to the tools a routine proposed. */
  grant: (host: string, id: string, tools: string[] | "all") =>
    invoke<{ id: string; needsAccess: number }>("grant", host, id, tools),
  /** A shell session on `host` that runs the harness's login; the caller
   * shows it as a pane. */
  openLoginTerminal: (host: string, harness: string) =>
    invoke<{ host: string; sessionHost: string; sessionId: string }>(
      "open-login-terminal",
      { host, harness },
    ),
  /** The owner's yes to one change a run waits to apply (`proposal` is its
   * index in the run's proposals). */
  approve: (host: string, id: string, run: number, proposal: number) =>
    invoke<{ id: string; run: number; proposal: number; status: string }>(
      "approve",
      host,
      id,
      run,
      proposal,
    ),
  settingsGet: () => invoke<BuilderSettings>("settings.get"),
  settingsSet: (patch: Partial<BuilderSettings>) =>
    invoke<BuilderSettings>("settings.set", patch),
  /** Starts the routine builder: its first turn runs as a background session
   * on the builder host. */
  build: (text: string, routineId?: string) =>
    invoke<BuilderStart>("builder.start", {
      text,
      ...(routineId ? { routineId } : {}),
    }),
  /** The owner's next message: a new turn that resumes the conversation. */
  buildReply: (sessionId: string, text: string) =>
    invoke<BuilderStart & { agentSession: string }>("builder.reply", {
      sessionId,
      text,
    }),
  /** Where a turn is: running, or its plain-text answer. */
  buildTurn: (sessionId: string) =>
    invoke<BuilderTurn>("builder.turn", { sessionId }),
};

/** What `routines.save` and `routines.validate` take: the routine and its
 * brief. The host of the routine is the daemon the call goes to. */
export type SavePayload = { routine: Routine; brief: string };
