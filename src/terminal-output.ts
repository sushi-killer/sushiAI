const encoder = new TextEncoder();
const utf8Bytes = (text: string) => encoder.encode(text).byteLength;

/** Keystrokes go out at once, each without waiting for the previous write to
 * settle; IPC and the daemon connection keep their order. The cap counts the
 * bytes and writes still in flight. */
export function createTerminalInput(
  write: (data: string) => Promise<void>,
  fail: (message: string) => void,
) {
  const limit = 1024 * 1024;
  let bytes = 0;
  let packets = 0;
  let closed = false;
  async function deliver(data: string, size: number) {
    try {
      await write(data);
    } catch (error) {
      fail(error instanceof Error ? error.message : String(error));
    } finally {
      bytes -= size;
      packets -= 1;
    }
  }
  return {
    send(data: string) {
      if (closed) return;
      const size = utf8Bytes(data);
      if (bytes + size > limit || packets >= 1024) {
        fail(
          "Terminal input queue is full. Wait for the terminal to process input and retry.",
        );
        return;
      }
      bytes += size;
      packets += 1;
      void deliver(data, size);
    },
    close() {
      closed = true;
    },
    get stats() {
      return { bytes, packets };
    },
  };
}

// The most text one xterm write gets, in UTF-16 units: as much as one message
// from main (OUTPUT_CHUNK_BYTES).
const WRITE_SLICE = 32 * 1024;
// The most text one frame hands to xterm. xterm parses for up to 12 ms per
// task and then renders: more than this in a frame and parsing runs into the
// next frame's rendering. The rest waits for the next frame, unacked.
export const FRAME_BUDGET = 2 * WRITE_SLICE;

/** `text` in pieces of at most WRITE_SLICE, cut after a line break where there
 * is one and never inside a surrogate pair. */
function slices(text: string) {
  const pieces: string[] = [];
  let at = 0;
  while (text.length - at > WRITE_SLICE) {
    let end = text.lastIndexOf("\n", at + WRITE_SLICE - 1) + 1;
    if (end <= at) {
      end = at + WRITE_SLICE;
      const code = text.charCodeAt(end - 1);
      if (code >= 0xd800 && code <= 0xdbff) end -= 1;
    }
    pieces.push(text.slice(at, end));
    at = end;
  }
  pieces.push(text.slice(at));
  return pieces;
}

/** Terminal output on its way into xterm: what arrives within one frame is
 * written in one `write` (cut between messages when it is more than
 * WRITE_SLICE, and spread over frames past FRAME_BUDGET), and its UTF-8 bytes
 * are acked once xterm has parsed them. A
 * snapshot replaces the terminal, with the next frame, and the output still
 * pending; main counts that output as in flight, so its bytes are acked all
 * the same. */
export function createOutputBatcher(sink: {
  write(data: string, done: () => void): void;
  reset(): void;
  ack(bytes: number): void;
  schedule(flush: () => void): void;
}) {
  let pending: string[] = [];
  let screen: string | undefined; // a snapshot due with the next frame
  let scheduled = false;
  let disposed = false;
  function schedule() {
    if (scheduled) return;
    scheduled = true;
    sink.schedule(flush);
  }
  const write = (text: string) =>
    sink.write(text, () => sink.ack(utf8Bytes(text)));
  function flush() {
    scheduled = false;
    if (disposed) return;
    const pieces = pending;
    pending = [];
    if (screen !== undefined) {
      sink.reset();
      pieces.unshift(...slices(screen));
      screen = undefined;
    }
    // xterm yields to rendering only between writes: a frame that brought
    // more than a slice is written in slices, cut between pieces (a snapshot,
    // up to 2000 lines, comes already cut).
    let group: string[] = [];
    let length = 0;
    let spent = 0;
    let index = 0;
    for (; index < pieces.length; index += 1) {
      const piece = pieces[index];
      if (spent && spent + piece.length > FRAME_BUDGET) break;
      if (length && length + piece.length > WRITE_SLICE) {
        write(group.join(""));
        group = [];
        length = 0;
      }
      group.push(piece);
      length += piece.length;
      spent += piece.length;
    }
    if (group.length) write(group.join(""));
    if (index < pieces.length) {
      pending = pieces.slice(index);
      schedule();
    }
  }
  return {
    data(text: string) {
      if (disposed || !text) return;
      pending.push(text);
      schedule();
    },
    // Snapshots that arrive within one frame (resyncs in a burst) cost one
    // reset and one parse: only the last is written.
    snapshot(text: string) {
      if (disposed) return;
      const dropped = (screen ?? "") + pending.join("");
      pending = [];
      screen = text;
      if (dropped) sink.ack(utf8Bytes(dropped));
      schedule();
    },
    dispose() {
      disposed = true;
      pending = [];
      screen = undefined;
    },
  };
}

/** Whether a cached terminal should attach again when its host comes up: its
 * first attach failed, or it was ended because the host went away while the
 * panel itself is not ended (a reconnect finds the session alive). */
export function shouldReattach(
  runtime: { ready: boolean; error: string; exited: boolean },
  panelEnded: boolean | undefined,
): boolean {
  if (panelEnded) return false;
  return runtime.exited || (!runtime.ready && !!runtime.error);
}
