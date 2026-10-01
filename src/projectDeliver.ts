import type { Bridge, ProjectPrepareStep } from "./types.ts";

export type PrepareOutcome =
  | { kind: "failed"; failure: PrepareFailureLike }
  | { kind: "started"; steps?: ProjectPrepareStep[] }
  | { kind: "unstarted"; path: string; steps?: ProjectPrepareStep[] }
  | { kind: "cancelled" };

type PrepareFailureLike = {
  stage: string;
  status?: number;
  timedOut?: boolean;
  message: string;
  steps?: ProjectPrepareStep[];
};

/** One run of the picker's flow: prepare the host, then start the session
 * there. The session starts only if nobody walked away, and a session that
 * cannot start is its own outcome, not a hang. */
export async function prepareAndStart(options: {
  bridge: Pick<Bridge, "projectHostPrepare" | "projectHostReady">;
  projectId: string;
  endpoint: string;
  useHostLogin: boolean;
  isGone(): boolean;
  start(path: string): Promise<boolean>;
  /** Called when a prepare really starts (not for a host that is ready). */
  onPreparing?(info: { found: boolean }): void;
}): Promise<PrepareOutcome> {
  try {
    // A host that already has the project, installed, starts at once: the
    // Prepare steps run only when something is missing.
    const ready = await options.bridge
      .projectHostReady(options.projectId, options.endpoint)
      .catch(
        () =>
          ({ ready: false }) as {
            ready: boolean;
            path?: string;
            reason?: string;
          },
      );
    if (ready.ready && ready.path && !options.useHostLogin) {
      if (options.isGone()) return { kind: "cancelled" };
      return (await options.start(ready.path))
        ? { kind: "started" }
        : { kind: "unstarted", path: ready.path };
    }
    options.onPreparing?.({ found: ready.reason === "install-stale" });
    const result = await options.bridge.projectHostPrepare(
      options.projectId,
      options.endpoint,
      options.useHostLogin,
      { pull: false },
    );
    if (!result.ok) return { kind: "failed", failure: result };
    if (options.isGone()) return { kind: "cancelled" };
    return (await options.start(result.path))
      ? { kind: "started", steps: result.steps }
      : { kind: "unstarted", path: result.path, steps: result.steps };
  } catch (reason) {
    return {
      kind: "failed",
      failure: {
        stage: "clone",
        message: reason instanceof Error ? reason.message : String(reason),
      },
    };
  }
}
