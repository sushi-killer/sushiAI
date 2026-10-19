import {
  materializeAfter,
  parseDiagram,
  reidOps,
  remapOp,
  serializeDiagram,
} from "./model.ts";
import type { Diagram, DiagramOp } from "./types.ts";

export type SaveIo = {
  read(): Promise<{ text: string; hash: string }>;
  write(text: string, expectedHash: string): Promise<{ hash: string }>;
};

export const ATTEMPTS = 3;

/** Replays the owner's ops on the file as it is now: read, parse, re-id ops
 * that collide with what the agent added, place nodes, write with the hash
 * that was read. A "changed on disk" refusal reads again, up to 3 times. An
 * unparseable file or any other error throws and writes nothing. */
export type SaveResult = {
  diagram: Diagram;
  hash: string;
  /** The hash of the file the write was based on. */
  readHash: string;
};

export async function saveOps(
  io: SaveIo,
  ops: DiagramOp[],
  remap: Map<string, string> = new Map(),
): Promise<SaveResult> {
  let refused = "File changed on disk";
  for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
    const file = await io.read();
    const fresh = parseDiagram(file.text);
    if ("error" in fresh) throw new Error(fresh.error);
    const diagram = materializeAfter(
      fresh.diagram,
      reidOps(fresh.diagram, ops, remap),
    );
    try {
      const { hash } = await io.write(serializeDiagram(diagram), file.hash);
      return { diagram, hash, readHash: file.hash };
    } catch (error) {
      refused = error instanceof Error ? error.message : String(error);
      if (!/changed on disk/i.test(refused)) throw error;
    }
  }
  throw new Error(refused);
}

/** One "the owner edited" notice per stretch of owner saves. A polled hash
 * that is neither one we saw before a save nor one our own write returned
 * means someone else wrote; the next owner save after that sends one more.
 * A poll can read our write before the write's reply arrives, so a hash
 * polled during a save is judged only once that save returns. */
export function createEditNotice(initialHash: string) {
  const own = new Set([initialHash]);
  const unsure = new Set<string>();
  let writing = false;
  let armed = true;
  const settle = () => {
    for (const hash of unsure) if (!own.has(hash)) armed = true;
    for (const hash of unsure) own.add(hash);
    unsure.clear();
  };
  return {
    polled(hash: string) {
      if (own.has(hash)) return;
      if (writing) {
        unsure.add(hash);
        return;
      }
      own.add(hash);
      armed = true;
    },
    saving(hash: string) {
      settle();
      writing = true;
      own.add(hash);
    },
    failed() {
      writing = false;
      settle();
    },
    /** True when the agent should be told about this save. `readHash` is the
     * file the write replaced: one we never saw means the agent wrote
     * between polls. */
    saved(hash: string, readHash: string): boolean {
      writing = false;
      if (!own.has(readHash)) armed = true;
      own.add(readHash);
      own.add(hash);
      settle();
      const send = armed;
      armed = false;
      return send;
    },
  };
}

export type QueueEvents = {
  /** The list of ops still waiting changed. */
  pending(ops: DiagramOp[]): void;
  /** A write is about to start. */
  saving(): void;
  saved(result: SaveResult): void;
  /** The ids the file made the owner's new items take instead. */
  remapped(remap: Map<string, string>): void;
  /** Every queued op was written. */
  idle(): void;
  /** A save failed; its ops stay queued. */
  failed(error: unknown): void;
};

/** The owner's unsaved ops and the loop that writes them: one flush at a
 * time, each write takes the ops queued when it started, and ops pushed while
 * it ran go out in the next write. A failed write keeps its ops. */
export function createSaveQueue(io: () => SaveIo, on: QueueEvents) {
  let queue: DiagramOp[] = [];
  let busy = false;
  let failed = false;
  const remap = new Map<string, string>();
  const set = (next: DiagramOp[]) => {
    queue = next;
    on.pending(queue);
  };
  return {
    push(op: DiagramOp) {
      set([...queue, op]);
    },
    pending: () => queue,
    hasFailed: () => failed,
    async flush(): Promise<void> {
      if (busy) return;
      busy = true;
      try {
        while (queue.length) {
          const ops = queue.slice();
          on.saving();
          const known = remap.size;
          const result = await saveOps(io(), ops, remap);
          failed = false;
          set(queue.slice(ops.length).map((op) => remapOp(op, remap)));
          if (remap.size > known) on.remapped(new Map(remap));
          on.saved(result);
        }
        on.idle();
      } catch (error) {
        failed = true;
        on.failed(error);
      } finally {
        busy = false;
      }
    },
  };
}
