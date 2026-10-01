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
  bridge: Pick<Bridge, "projectHostPrepare">;
  projectId: string;
  endpoint: string;
  useHostLogin: boolean;
  isGone(): boolean;
  start(path: string): Promise<boolean>;
}): Promise<PrepareOutcome> {
  try {
    const result = await options.bridge.projectHostPrepare(
      options.projectId,
      options.endpoint,
      options.useHostLogin,
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
