import { useEffect, useState } from "react";
import { errorText } from "./helpers";
import { useOrchestratorClient } from "./hostContext";
import type { Pipeline } from "./types";

/** The `pipeline.describe` diagram of a task or repo. It is fetched again
 * whenever `refreshKey` changes (a task update, a settings write). */
export function usePipeline(
  params: { repo?: string; taskId?: string },
  refreshKey: unknown,
): { pipeline: Pipeline | null; error: string } {
  const client = useOrchestratorClient();
  const [state, setState] = useState<{
    scope: string;
    pipeline: Pipeline | null;
    error: string;
  }>({ scope: "", pipeline: null, error: "" });
  const { repo, taskId } = params;
  const scope = `${repo ?? ""}|${taskId ?? ""}`;
  useEffect(() => {
    let live = true;
    client
      .pipelineDescribe({
        ...(repo ? { repo } : {}),
        ...(taskId ? { taskId } : {}),
      })
      .then((pipeline) => live && setState({ scope, pipeline, error: "" }))
      .catch(
        (e) => live && setState({ scope, pipeline: null, error: errorText(e) }),
      );
    return () => {
      live = false;
    };
  }, [client, repo, taskId, scope, refreshKey]);
  // Another task or repo's diagram is never shown under this one.
  return state.scope === scope
    ? { pipeline: state.pipeline, error: state.error }
    : { pipeline: null, error: "" };
}
