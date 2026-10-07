import { useEffect, useRef, useState, type RefObject } from "react";
import { errorText } from "../../lib/errors.ts";
import { projectRelative, type ArtifactKind } from "./artifact.ts";

export type PreviewFile =
  | { state: "loading" }
  | { state: "error"; message: string }
  | {
      state: "ready";
      hash: string;
      /** Decoded text for markdown, html and text; empty for binary kinds. */
      text: string;
      /** A data URL for images; empty otherwise. */
      dataUrl: string;
      /** When this content first appeared, for "updated Ns ago". */
      changedAt: number;
    };

const POLL_MS = 2000;
const TEXT_LIMIT = 2 * 1024 * 1024;

function failure(error: unknown, path: string): string {
  const message = errorText(error);
  if (/regular file|no such file|not found|ENOENT/i.test(message))
    return `File not found: ${path}`;
  return message;
}

/** Reads one file and reads it again every 2 s while the pane is visible. A
 * new path loads at once; unchanged content (same hash) keeps its state, so
 * the view only re-renders when the file really changed. */
export function usePreviewFile(
  path: string,
  cwd: string,
  kind: ArtifactKind,
  endpoint: string | undefined,
  element: RefObject<HTMLElement | null>,
): PreviewFile {
  const [file, setFile] = useState<PreviewFile>({ state: "loading" });
  const hash = useRef("");
  useEffect(() => {
    hash.current = "";
    setFile({ state: "loading" });
    if (!path) return;
    let disposed = false;
    let busy = false;
    const load = async () => {
      if (busy || !window.bridge) return;
      busy = true;
      try {
        const data = await window.bridge.projectInspect(endpoint, {
          operation: "read",
          root: cwd,
          path: projectRelative(path, cwd),
        });
        if (disposed || data.hash === hash.current) return;
        hash.current = data.hash;
        const binary = kind === "image" || kind === "pdf";
        let text = "";
        if (!binary) {
          const bytes = Uint8Array.from(atob(data.base64), (c) =>
            c.charCodeAt(0),
          );
          text = new TextDecoder().decode(bytes.subarray(0, TEXT_LIMIT));
        }
        setFile({
          state: "ready",
          hash: data.hash,
          text,
          dataUrl:
            kind === "image" || kind === "svg"
              ? `data:${kind === "svg" ? "image/svg+xml" : data.mime};base64,${data.base64}`
              : "",
          changedAt: Date.now(),
        });
      } catch (error) {
        if (!disposed) {
          hash.current = "";
          setFile({ state: "error", message: failure(error, path) });
        }
      } finally {
        busy = false;
      }
    };
    void load();
    // A pane scrolled or tabbed out of sight does not poll either.
    let onScreen = true;
    const watcher =
      element.current && typeof IntersectionObserver !== "undefined"
        ? new IntersectionObserver((entries) => {
            const now = entries[entries.length - 1]?.isIntersecting ?? true;
            if (now && !onScreen) void load();
            onScreen = now;
          })
        : null;
    if (watcher && element.current) watcher.observe(element.current);
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible" && onScreen) void load();
    }, POLL_MS);
    return () => {
      disposed = true;
      watcher?.disconnect();
      window.clearInterval(timer);
    };
  }, [path, cwd, kind, endpoint, element]);
  return file;
}
