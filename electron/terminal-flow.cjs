const OUTPUT_CREDIT_BYTES = 256 * 1024;
const OUTPUT_CHUNK_BYTES = 32 * 1024;
const MAX_FRAME_BYTES = 4 * 1024 * 1024;
const MAX_INPUT_BYTES = 2 * 1024 * 1024;
const MAX_INPUT_MESSAGES = 1024;

function createOutputDelivery({ streamId, send, changed }) {
  const inFlight = new Map();
  let pending;
  let bytes = 0;
  let sequence = 0;
  let closed = false;
  let peakBytes = 0;
  function flush() {
    while (pending && !closed && inFlight.size < 64) {
      let end = Math.min(
        pending.data.length,
        pending.offset + Math.floor(OUTPUT_CHUNK_BYTES / 3),
      );
      if (/^[\uDC00-\uDFFF]$/.test(pending.data[end] || "")) end--;
      const data = pending.data.slice(pending.offset, end);
      const size = Buffer.byteLength(data);
      if (bytes + size > OUTPUT_CREDIT_BYTES) break;
      const reset = pending.reset;
      pending.reset = false;
      pending.offset = end;
      if (end === pending.data.length) pending = undefined;
      const current = ++sequence;
      inFlight.set(current, size);
      bytes += size;
      peakBytes = Math.max(peakBytes, bytes);
      send({ streamId, sequence: current, reset, data });
    }
  }
  return {
    enqueue(data, reset) {
      if (closed) return;
      if (pending) throw new Error("Terminal delivery is already paused.");
      pending = { data, reset, offset: 0 };
      flush();
    },
    ack(token, current) {
      if (closed || token !== streamId || !inFlight.has(current)) return;
      bytes -= inFlight.get(current);
      inFlight.delete(current);
      flush();
      changed();
    },
    get blocked() {
      return !!pending || bytes >= OUTPUT_CREDIT_BYTES || inFlight.size >= 64;
    },
    get stats() {
      return {
        bytes,
        peakBytes,
        pendingBytes: pending ? Buffer.byteLength(pending.data) : 0,
        packets: inFlight.size,
      };
    },
    close() {
      closed = true;
      pending = undefined;
      inFlight.clear();
      bytes = 0;
    },
  };
}

function createInputWriter(stdin) {
  const queue = [];
  const outstanding = new Set();
  let bytes = 0;
  let peakBytes = 0;
  let blocked = false;
  let failure;
  function rejectAll(error) {
    failure = error;
    for (const item of outstanding) item.reject(error);
    outstanding.clear();
    queue.length = 0;
    bytes = 0;
  }
  function flush() {
    while (!blocked && queue.length && !failure) {
      const item = queue.shift();
      try {
        blocked = !stdin.write(item.line, (error) => {
          if (!outstanding.delete(item)) return;
          bytes -= item.bytes;
          if (error) item.reject(error);
          else item.resolve();
        });
      } catch (error) {
        rejectAll(error);
      }
    }
  }
  stdin.on("drain", () => {
    blocked = false;
    flush();
  });
  stdin.on("error", rejectAll);
  stdin.on("close", () =>
    rejectAll(new Error("Terminal input stream closed.")),
  );
  return {
    write(value) {
      if (failure || stdin.destroyed || stdin.writableEnded)
        return Promise.reject(
          failure || new Error("Terminal input stream closed."),
        );
      const line = JSON.stringify(value) + "\n";
      const size = Buffer.byteLength(line);
      if (
        bytes + size > MAX_INPUT_BYTES ||
        outstanding.size >= MAX_INPUT_MESSAGES
      )
        return Promise.reject(
          new Error(
            "Terminal input queue is full. Wait for the terminal to process input and retry.",
          ),
        );
      return new Promise((resolve, reject) => {
        const item = { line, bytes: size, resolve, reject };
        queue.push(item);
        outstanding.add(item);
        bytes += size;
        peakBytes = Math.max(peakBytes, bytes);
        flush();
      });
    },
    close: rejectAll,
    get stats() {
      return { bytes, peakBytes, messages: outstanding.size };
    },
  };
}

module.exports = {
  OUTPUT_CREDIT_BYTES,
  OUTPUT_CHUNK_BYTES,
  MAX_FRAME_BYTES,
  MAX_INPUT_BYTES,
  MAX_INPUT_MESSAGES,
  createOutputDelivery,
  createInputWriter,
};
