import { useCallback, useEffect, useRef, useState } from "react";
import { Check } from "lucide-react";
import type {
  Project,
  ProjectImportPreview,
  ProjectImportResult,
} from "./types";

/** One sentence for what a pull did, with counts: never silent. */
export function importSummary(result: ProjectImportResult): string {
  const added = result.addedVariables.length;
  const filled = result.filledVariables?.length ?? 0;
  const servers = result.addedServers.length;
  const skipped =
    (result.skippedVariables?.length ?? 0) +
    (result.skippedServers?.length ?? 0);
  const parts = [
    added ? `${added} ${added === 1 ? "variable" : "variables"}` : "",
    filled ? `${filled} ${filled === 1 ? "value" : "values"} filled` : "",
    servers ? `${servers} MCP ${servers === 1 ? "server" : "servers"}` : "",
  ].filter(Boolean);
  const head = parts.length ? `Added ${parts.join(", ")}` : "Nothing new";
  return skipped ? `${head}, ${skipped} already there.` : `${head}.`;
}

/** How many things the folder holds that the project has not got. */
export function pending(preview: ProjectImportPreview): number {
  return (
    preview.newVariables.length +
    preview.fillVariables.length +
    preview.newServers.length
  );
}

/** Pulls a project's folder into the project: new keys are applied when the
 * tab opens, and whatever is left (or arrives later) is one click away. The
 * result is always said, an error too. */
export function FolderImport({
  project,
  cwd,
  endpoint,
  onProject,
}: {
  project: Project;
  cwd: string;
  endpoint?: string;
  onProject(project: Project): void;
}) {
  const [left, setLeft] = useState(0);
  const [removed, setRemoved] = useState<string[]>([]);
  const [notice, setNotice] = useState("");
  const [error, setError] = useState("");
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const opened = useRef("");
  const options = {
    endpoint: endpoint?.startsWith("ssh:") ? endpoint : "local",
  };

  const look = useCallback(async () => {
    const bridge = window.bridge;
    if (!bridge) return 0;
    const found = await bridge.projectImportPreview(project.id, cwd, options);
    setLeft(pending(found));
    setRemoved([...found.removedVariables, ...found.removedServers]);
    return pending(found);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.id, cwd, endpoint]);

  function fail(caught: unknown) {
    setReason(caught instanceof Error ? caught.message : String(caught));
    setError("Could not read this folder");
  }

  async function pull(force: boolean) {
    if (!window.bridge) return;
    setBusy(true);
    setError("");
    setReason("");
    setNotice("");
    try {
      const result = await window.bridge.projectImportLocal(project.id, cwd, {
        ...options,
        force,
      });
      onProject(result.project);
      setNotice(importSummary(result));
      await look();
    } catch (caught) {
      fail(caught);
    } finally {
      setBusy(false);
    }
  }

  // New keys are applied when the tab opens, once.
  useEffect(() => {
    const key = `${project.id}:${cwd}:${endpoint ?? ""}`;
    if (opened.current === key || !window.bridge) return;
    opened.current = key;
    void (async () => {
      try {
        if ((await look()) > 0) await pull(false);
      } catch (caught) {
        fail(caught);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project.id, cwd, endpoint]);

  // Coming back to the app after editing the folder's files: look again.
  useEffect(() => {
    const again = () => void look().catch(() => {});
    window.addEventListener("focus", again);
    return () => window.removeEventListener("focus", again);
  }, [look]);

  if (!notice && !error && !left && !removed.length) return null;
  return (
    <div className="pd-folder-import">
      {error ? (
        <span role="alert" className="pd-folder-error" title={reason}>
          {error}
        </span>
      ) : (
        notice && (
          <span role="status" className="pd-folder-ok">
            <Check size={13} aria-hidden />
            {notice}
          </span>
        )
      )}
      {(left > 0 || (error && left === 0)) && (
        <button
          type="button"
          className="ui-button ghost"
          disabled={busy}
          onClick={() => void pull(false)}
        >
          {busy
            ? "Importing…"
            : left > 0
              ? `Import ${left} new from folder`
              : "Try again"}
        </button>
      )}
      {removed.length > 0 && (
        <button
          type="button"
          className="ui-button ghost"
          disabled={busy}
          title={`You removed ${removed.join(", ")} earlier; they are not brought back unless you ask.`}
          onClick={() => void pull(true)}
        >
          {`Restore removed: ${removed.slice(0, 2).join(", ")}${removed.length > 2 ? ` +${removed.length - 2}` : ""}`}
        </button>
      )}
    </div>
  );
}
