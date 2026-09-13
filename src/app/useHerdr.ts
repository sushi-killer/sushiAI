import { useCallback, useEffect, useState } from "react";
import { errorText } from "./errors.ts";
import { reconcileHerdrWorkspaces } from "../herdrSnapshot.ts";
import type { Snapshot, System, Workspace } from "../types";

export type HerdrConnection = "connected" | "offline" | "connecting";

/** The Herdr session: which socket we talk to, whether it answers, and the
 * four-second poll that keeps the workspace list in step with it.
 *
 * `refreshHerdr` accepts an endpoint so a panel can refresh its own connection.
 * Only a refresh for the *current* socket may change the connection banner, so
 * a background endpoint never reports the main one as offline. */
export function useHerdr({
  savedSocket,
  notify,
  setWorkspaces,
}: {
  savedSocket: string;
  notify: (text: string) => void;
  setWorkspaces: React.Dispatch<React.SetStateAction<Workspace[]>>;
}) {
  const [system, setSystem] = useState<System | null>(null);
  const [socket, setSocket] = useState(savedSocket);
  const [connection, setConnection] = useState<HerdrConnection>("connecting");
  const [connectionError, setConnectionError] = useState("");

  useEffect(() => {
    if (!window.bridge) {
      setConnection("offline");
      setConnectionError(
        "Browser preview. Start the desktop app for terminal and Herdr access.",
      );
      return;
    }
    window.bridge
      .system()
      .then((info) => {
        setSystem(info);
        setSocket((current) => current || info.socketPath);
        setWorkspaces((items) =>
          items.map((w) => ({ ...w, cwd: w.cwd || info.cwd })),
        );
      })
      .catch((error) => notify(errorText(error)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [notify]);

  const home = system?.home || "";
  const refreshHerdr = useCallback(
    async (path = socket) => {
      if (!window.bridge || !path) return;
      try {
        const response = await window.bridge.herdr(path, "session.snapshot");
        const snapshot: Snapshot = response.snapshot || response;
        if (
          !Array.isArray(snapshot.workspaces) ||
          !Array.isArray(snapshot.panes)
        )
          throw new Error("Unsupported Herdr snapshot response.");
        setWorkspaces((current) =>
          reconcileHerdrWorkspaces(current, snapshot, path, home),
        );
        if (path === socket) {
          setConnection("connected");
          setConnectionError("");
        }
      } catch (error) {
        if (path === socket) {
          setConnection("offline");
          setConnectionError(errorText(error));
        }
      }
    },
    [socket, home, setWorkspaces],
  );

  useEffect(() => {
    if (!socket || !system) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      await refreshHerdr();
      if (!stopped) timer = setTimeout(poll, 4000);
    }
    poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [socket, system, refreshHerdr]);

  return {
    system,
    socket,
    setSocket,
    connection,
    setConnection,
    connectionError,
    setConnectionError,
    refreshHerdr,
  };
}
