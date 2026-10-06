export function createTerminalInput(
  write: (data: string) => Promise<void>,
  fail: (message: string) => void,
) {
  const limit = 1024 * 1024;
  const queue: { data: string; bytes: number }[] = [];
  let bytes = 0;
  let writing = false;
  let closed = false;
  async function flush() {
    if (writing || closed) return;
    writing = true;
    while (queue.length && !closed) {
      const item = queue.shift()!;
      try {
        await write(item.data);
      } catch (error) {
        fail(error instanceof Error ? error.message : String(error));
      } finally {
        bytes -= item.bytes;
      }
    }
    writing = false;
  }
  return {
    send(data: string) {
      if (closed) return;
      const size = new TextEncoder().encode(data).byteLength;
      if (bytes + size > limit || queue.length >= 1024) {
        fail(
          "Terminal input queue is full. Wait for the terminal to process input and retry.",
        );
        return;
      }
      bytes += size;
      queue.push({ data, bytes: size });
      void flush();
    },
    close() {
      closed = true;
      for (const item of queue) bytes -= item.bytes;
      queue.length = 0;
    },
    get stats() {
      return { bytes, packets: queue.length + (writing ? 1 : 0) };
    },
  };
}
