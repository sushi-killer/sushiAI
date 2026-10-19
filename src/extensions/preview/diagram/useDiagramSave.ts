import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../../../lib/errors.ts";
import { projectRelative } from "../artifact.ts";
import { applyOps, parseDiagram } from "./model.ts";
import { createEditNotice, createSaveQueue } from "./saveOps.ts";
import type { Diagram, DiagramOp } from "./types.ts";

export type SaveStatus =
  | { state: "saved" }
  | { state: "saving" }
  | { state: "error"; message: string };

const DEBOUNCE_MS = 400;

type Input = {
  text: string;
  hash: string;
  path: string;
  cwd: string;
  connection: string | undefined;
  onEdited: () => void;
  /** Ids the owner's new items took because the file already used them. */
  onRemap: (remap: Map<string, string>) => void;
};

function decode(base64: string): string {
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

/** The diagram to draw and the owner's edits on top of it. Edits are ops: a
 * save reads the file fresh and replays them, so what an agent wrote during a
 * drag is kept. After a write the written diagram is drawn until the polled
 * file shows the written hash or moves on to another one. */
export function useDiagramSave(input: Input) {
  const { text, hash } = input;
  const parsed = useMemo(() => parseDiagram(text), [text]);
  const [pending, setPending] = useState<DiagramOp[]>([]);
  const [written, setWritten] = useState<{
    diagram: Diagram;
    before: string;
    hash: string;
  } | null>(null);
  const [status, setStatus] = useState<SaveStatus>({ state: "saved" });
  const hashRef = useRef(hash);
  hashRef.current = hash;
  const latest = useRef(input);
  latest.current = input;
  const before = useRef("");
  const timer = useRef(0);
  const notice = useRef<ReturnType<typeof createEditNotice> | null>(null);
  notice.current ??= createEditNotice(hash);

  const queue = useMemo(
    () =>
      createSaveQueue(
        () => {
          const at = latest.current;
          const file = projectRelative(at.path, at.cwd);
          const bridge = window.bridge!;
          return {
            read: async () => {
              const data = await bridge.projectInspect(at.connection, {
                operation: "read",
                root: at.cwd,
                path: file,
              });
              return { text: decode(data.base64), hash: data.hash };
            },
            write: (body, expectedHash) =>
              bridge.projectInspect(at.connection, {
                operation: "write",
                root: at.cwd,
                path: file,
                text: body,
                expectedHash,
              }),
          };
        },
        {
          pending: setPending,
          saving: () => {
            before.current = hashRef.current;
            notice.current?.saving(before.current);
            setStatus({ state: "saving" });
          },
          saved: (result) => {
            setWritten({
              diagram: result.diagram,
              before: before.current,
              hash: result.hash,
            });
            if (notice.current?.saved(result.hash, result.readHash))
              latest.current.onEdited();
          },
          remapped: (map) => latest.current.onRemap(map),
          idle: () => setStatus({ state: "saved" }),
          failed: (error) =>
            setStatus({ state: "error", message: errorText(error) }),
        },
      ),
    [],
  );

  useEffect(() => {
    notice.current?.polled(hash);
  }, [hash]);

  useEffect(() => {
    if (written && (hash === written.hash || hash !== written.before))
      setWritten(null);
  }, [hash, written]);

  const flush = useCallback(() => {
    window.clearTimeout(timer.current);
    if (window.bridge) void queue.flush();
  }, [queue]);

  // A failed save is tried again once the file moves.
  useEffect(() => {
    if (queue.hasFailed() && queue.pending().length) flush();
  }, [hash, queue, flush]);

  // Leaving the file with edits still waiting writes them out.
  useEffect(
    () => () => {
      if (queue.pending().length) void queue.flush();
    },
    [queue],
  );

  const push = useCallback(
    (op: DiagramOp, immediate = false) => {
      queue.push(op);
      window.clearTimeout(timer.current);
      if (immediate) void flush();
      else timer.current = window.setTimeout(() => void flush(), DEBOUNCE_MS);
    },
    [flush, queue],
  );

  const base =
    written?.diagram ?? ("diagram" in parsed ? parsed.diagram : null);
  const display = useMemo(
    () => (base ? applyOps(base, pending) : null),
    [base, pending],
  );
  const error = !base && "error" in parsed ? parsed.error : "";
  return { display, error, status, push };
}
