import { useEffect, useRef, useState } from "react";
import {
  PrepareProgress,
  type PrepareFailure,
} from "../orchestrator/PrepareViews";
import { rememberPrepareTimes } from "../projectPrepare";
import { prepareAndStart } from "../projectDeliver";
import type { Project } from "../types";

/** The "+" picker's own run of the first-run flow for a host that is not set
 * up yet: Prepare with its steps, then the failure screen with its actions.
 * It hands back the checkout path once the host is ready, and the session is
 * started only if the owner has not walked away. */
export function PrepareAndStart({
  project,
  hostName,
  endpoint,
  onStart,
  onCancel,
}: {
  project: Project;
  hostName: string;
  endpoint: string;
  /** The host is prepared: start the session in `path`; false when it could
   * not be started. */
  onStart(path: string): Promise<boolean>;
  onCancel(): void;
}) {
  const [phase, setPhase] = useState<
    "checking" | "running" | "failed" | "unstarted"
  >("checking");
  const [failure, setFailure] = useState<PrepareFailure | null>(null);
  const [editToken, setEditToken] = useState(false);
  const [tokenDraft, setTokenDraft] = useState("");
  const [prepared, setPrepared] = useState("");
  const [found, setFound] = useState(false);
  // A host the owner switched off for this project gets no value.
  const noSecrets = !!project.hosts?.[endpoint]?.withheld;
  const gone = useRef(false);
  const started = useRef(false);
  useEffect(
    () => () => {
      gone.current = true;
    },
    [],
  );
  const leave = () => {
    gone.current = true;
    onCancel();
  };

  async function run(useHostLogin = false) {
    if (!window.bridge) return;
    setFailure(null);
    setPhase("checking");
    const outcome = await prepareAndStart({
      onPreparing: (info) => {
        setFound(info.found);
        setPhase("running");
      },
      bridge: window.bridge,
      projectId: project.id,
      endpoint,
      useHostLogin,
      isGone: () => gone.current,
      start: onStart,
    });
    if (outcome.kind === "started" || outcome.kind === "unstarted")
      rememberPrepareTimes(project.id, endpoint, outcome.steps);
    if (gone.current) return;
    if (outcome.kind === "failed") {
      setFailure(outcome.failure);
      setPhase("failed");
    } else if (outcome.kind === "unstarted") {
      setPrepared(outcome.path);
      setPhase("unstarted");
    }
  }

  // Choosing "Prepare <host> and start" is the whole question: it starts.
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function saveToken() {
    if (!window.bridge || !tokenDraft.trim()) return;
    try {
      await window.bridge.projectGitTokenSet(project.id, tokenDraft);
      setTokenDraft("");
      setEditToken(false);
      void run();
    } catch (reason) {
      setFailure({
        stage: "clone",
        message: reason instanceof Error ? reason.message : String(reason),
      });
    }
  }

  const eyebrow = `${hostName} · first run of ${project.name}`;
  return (
    <div className="pk-prepare">
      {phase === "checking" ? (
        <div className="orch-prep" role="status">
          <div className="orch-prep-eyebrow">{eyebrow}</div>
          <h2>{`Starting on ${hostName}`}</h2>
        </div>
      ) : phase === "unstarted" ? (
        <div className="orch-prep" role="alert">
          <div className="orch-prep-eyebrow">{eyebrow}</div>
          <h2>{`Prepared ${hostName}, but the session could not start`}</h2>
          <p>
            {`${project.name} is ready on ${hostName}. Opening the session there failed; nothing else was changed.`}
          </p>
          <div className="orch-prep-actions">
            <button
              type="button"
              className="ui-button secondary"
              onClick={() => {
                setPhase("running");
                void onStart(prepared)
                  .catch(() => false)
                  .then((ok) => {
                    if (!ok && !gone.current) setPhase("unstarted");
                  });
              }}
            >
              Try again
            </button>
            <button type="button" className="ui-button ghost" onClick={leave}>
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <PrepareProgress
          project={project}
          hostName={hostName}
          failure={failure}
          noSecrets={noSecrets}
          session
          found={found}
          eyebrow={eyebrow}
          editToken={editToken}
          tokenDraft={tokenDraft}
          onTokenDraft={setTokenDraft}
          onRetry={() => void run()}
          onHostLogin={() => void run(true)}
          onEditToken={() => setEditToken(true)}
          onSaveToken={() => void saveToken()}
          onCancel={leave}
        />
      )}
    </div>
  );
}
