export type TerminalPacket = {
  data: string;
  streamId?: string;
  sequence?: number;
  reset?: boolean;
};

const CREDIT_BYTES = 256 * 1024;
const MAX_PACKETS = 64;

export function createTerminalOutput({
  streamId,
  write,
  reset,
  ack,
  fail,
}: {
  streamId?: string;
  write(data: string, done: () => void): void;
  reset(): void;
  ack(streamId: string, sequence: number): void;
  fail(message: string): void;
}) {
  const queue: (TerminalPacket & { bytes: number })[] = [];
  let ready = false;
  let paused = false;
  let writing = false;
  let closed = false;
  let bytes = 0;
  let sequence = 0;
  function flush() {
    if (closed || !ready || paused || writing || !queue.length) return;
    const packet = queue.shift()!;
    writing = true;
    if (packet.reset) reset();
    write(packet.data, () => {
      if (closed) return;
      writing = false;
      bytes -= packet.bytes;
      if (packet.streamId && packet.sequence)
        ack(packet.streamId, packet.sequence);
      flush();
    });
  }
  return {
    accepts(token?: string) {
      return !closed && (!token || token === streamId);
    },
    push(packet: TerminalPacket) {
      if (closed || (packet.streamId && packet.streamId !== streamId)) return;
      if (packet.streamId) {
        if (
          !Number.isSafeInteger(packet.sequence) ||
          packet.sequence! <= sequence
        )
          return;
        sequence = packet.sequence!;
      }
      const size = new TextEncoder().encode(packet.data).byteLength;
      if (
        packet.streamId &&
        (bytes + size > CREDIT_BYTES ||
          queue.length + (writing ? 1 : 0) >= MAX_PACKETS)
      ) {
        closed = true;
        queue.length = 0;
        bytes = 0;
        fail(
          "Terminal delivery exceeded its limit. Reconnect to refresh the terminal.",
        );
        return;
      }
      bytes += size;
      queue.push({ ...packet, bytes: size });
      flush();
    },
    start() {
      ready = true;
      flush();
    },
    pause(value: boolean) {
      paused = value;
      flush();
    },
    close() {
      closed = true;
      queue.length = 0;
      bytes = 0;
    },
    get stats() {
      return { bytes, packets: queue.length + (writing ? 1 : 0) };
    },
  };
}

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
