import { useState } from "react";
import type { CoreViewProps } from "../extensions/coreViews.ts";
import { routinesClient } from "./client.ts";

/** "Log in on <host>": starts the harness's login in a terminal session on
 * the host where the run found it signed out, and shows that session as a
 * pane. */
export function LoginButton({
  host,
  loginHost,
  harness,
  openSession,
}: {
  /** The routine's host id: where the terminal starts. */
  host: string;
  /** The host's name as the run reported it. */
  loginHost: string;
  harness: string;
  openSession: NonNullable<CoreViewProps["openSession"]>;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  return (
    <>
      <button
        type="button"
        className="ui-button secondary"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError("");
          try {
            const started = await routinesClient.openLoginTerminal(
              host,
              harness,
            );
            openSession(started.sessionHost, started.sessionId, "Log in");
          } catch (reason) {
            setError(reason instanceof Error ? reason.message : String(reason));
          } finally {
            setBusy(false);
          }
        }}
      >
        {`Log in on ${loginHost}`}
      </button>
      {error && (
        <span role="alert" className="rt-error">
          {error}
        </span>
      )}
    </>
  );
}
