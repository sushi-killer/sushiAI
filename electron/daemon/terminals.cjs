"use strict";

// Terminal streaming over the daemon: one attach per panel, bytes delivered to
// the renderer in order with byte-credit pacing. The manager owns reconnects and
// resyncs; a fresh snapshot arrives through the same callback and replaces
// whatever the renderer shows.

const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { StringDecoder } = require("node:string_decoder");
const {
  OUTPUT_CREDIT_BYTES,
  OUTPUT_CHUNK_BYTES,
} = require("../terminal-flow.cjs");

const FIRST_ATTACH_SCROLLBACK = 2000;
const MAX_QUEUED_BYTES = 4 * OUTPUT_CREDIT_BYTES;
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;

// A shell reads the path as one word however the file was named.
const shellPath = (value) => `'${value.replace(/'/g, "'\\''")}' `;
const quote = (value) => `'${String(value).replace(/'/g, "'\\''")}'`;

// Stores stdin as ~/.sushiai/attachments/<name> on the host (file 0600, every
// directory it creates 0700) and prints the absolute path.
const uploadScript = (name) =>
  `umask 077; d="$HOME/.sushiai/attachments"; mkdir -p "$d" && chmod 700 "$d" && f="$d"/${quote(name)} && cat > "$f" && chmod 600 "$f" && printf '%s\\n' "$f"`;

function createTerminalHandlers({
  getManager,
  send,
  onEvent,
  attachmentsDir = path.join(os.tmpdir(), "sushiai-attachments"),
  exec,
}) {
  const panels = new Map(); // panelId -> entry

  function entryFor(panelId) {
    const entry = panels.get(panelId);
    if (!entry) throw new Error("This terminal is not attached.");
    return entry;
  }

  function overflow(panelId, entry) {
    if (entry.resyncing) return;
    entry.resyncing = true;
    entry.queue = [];
    entry.queuedBytes = 0;
    setImmediate(() => {
      if (entry.closed || panels.get(panelId) !== entry) return;
      attach({
        panelId,
        host: entry.host,
        sessionId: entry.sessionId,
        cols: entry.cols,
        rows: entry.rows,
      }).catch(() => send("daemon-terminal-data", { panelId, exited: true }));
    });
  }

  function createEntry(panelId, host, sessionId, cols, rows, stale = 0) {
    const decoder = new StringDecoder("utf8");
    const entry = {
      host,
      sessionId,
      handle: undefined,
      cols,
      rows,
      closed: false,
      queue: [], // ordered strings waiting for credit
      queuedBytes: 0,
      inFlight: 0, // bytes sent and not acked, snapshot included
      stale, // unacked bytes sent before the last snapshot
    };
    // Sends queued text while credit lasts. A piece is cut on a code point and
    // by UTF-8 bytes; when not even the next character fits, it waits for an ack.
    function flush() {
      while (
        !entry.closed &&
        entry.queue.length &&
        entry.inFlight < OUTPUT_CREDIT_BYTES
      ) {
        const head = entry.queue[0];
        const room = Math.min(
          OUTPUT_CHUNK_BYTES,
          OUTPUT_CREDIT_BYTES - entry.inFlight,
        );
        let end = 0;
        let bytes = 0;
        for (const character of head) {
          const size = Buffer.byteLength(character);
          if (bytes + size > room) break;
          bytes += size;
          end += character.length;
        }
        if (end === 0) break;
        const piece = head.slice(0, end);
        if (end < head.length) entry.queue[0] = head.slice(end);
        else entry.queue.shift();
        entry.queuedBytes -= bytes;
        entry.inFlight += bytes;
        send("daemon-terminal-data", { panelId, data: piece });
      }
    }
    entry.flush = flush;
    entry.onBytes = (buffer, info = {}) => {
      if (entry.closed) return;
      if (info.snapshot) {
        // The snapshot replaces everything older, queued or not. Bytes sent
        // before it and not yet acked still come back as acks: they must not
        // shrink the credit of the new window (`stale`).
        decoder.end();
        entry.queue = [];
        entry.queuedBytes = 0;
        entry.stale += entry.inFlight;
        const text = buffer.toString("utf8");
        entry.cols = info.cols ?? entry.cols;
        entry.rows = info.rows ?? entry.rows;
        entry.inFlight = Buffer.byteLength(text);
        send("daemon-terminal-data", {
          panelId,
          snapshot: text,
          cols: entry.cols,
          rows: entry.rows,
        });
        return;
      }
      const text = decoder.write(buffer);
      if (!text) return;
      entry.queue.push(text);
      entry.queuedBytes += Buffer.byteLength(text);
      flush();
      // A renderer that stopped acking (closed or throttled window) must not
      // grow this queue without bound: drop it and take a fresh snapshot.
      if (entry.queuedBytes > MAX_QUEUED_BYTES) overflow(panelId, entry);
    };
    return entry;
  }

  async function attach({ panelId, host, sessionId, cols, rows }) {
    // Bytes the old attach sent and the renderer has not acked yet still come
    // back as acks: they belong to the old window, not to the new one.
    const before = panels.get(panelId);
    const owed = before ? before.inFlight + before.stale : 0;
    await detach(panelId);
    const manager = getManager();
    // A new attach always lands in an empty terminal, so it brings the history.
    const scrollback = FIRST_ATTACH_SCROLLBACK;
    const entry = createEntry(panelId, host, sessionId, cols, rows, owed);
    panels.set(panelId, entry);
    try {
      const handle = await manager.attach(
        host,
        sessionId,
        { scrollback },
        entry.onBytes,
      );
      if (entry.closed) {
        await handle.detach().catch(() => {});
        return;
      }
      entry.handle = handle;
      entry.cols = handle.cols ?? entry.cols;
      entry.rows = handle.rows ?? entry.rows;
    } catch (error) {
      entry.closed = true;
      if (panels.get(panelId) === entry) panels.delete(panelId);
      throw error;
    }
    if (cols !== entry.cols || rows !== entry.rows) {
      await manager
        .request(host, "session.resize", { id: sessionId, cols, rows })
        .then(() => {
          entry.cols = cols;
          entry.rows = rows;
        })
        .catch(() => {});
    }
  }

  async function detach(panelId) {
    const entry = panels.get(panelId);
    if (!entry) return;
    panels.delete(panelId);
    entry.closed = true;
    entry.queue = [];
    if (entry.handle) await entry.handle.detach().catch(() => {});
  }

  async function write(panelId, data) {
    const entry = entryFor(panelId);
    await getManager().request(entry.host, "session.input", {
      id: entry.sessionId,
      data,
    });
  }

  async function resize(panelId, cols, rows) {
    const entry = entryFor(panelId);
    await getManager().request(entry.host, "session.resize", {
      id: entry.sessionId,
      cols,
      rows,
    });
    entry.cols = cols;
    entry.rows = rows;
  }

  function ack(panelId, bytes) {
    const entry = panels.get(panelId);
    if (!entry) return;
    const old = Math.min(entry.stale, bytes);
    entry.stale -= old;
    entry.inFlight = Math.max(0, entry.inFlight - (bytes - old));
    entry.flush();
  }

  const typePath = (panelId, file) => write(panelId, shellPath(file));

  // A dropped file: typed by its path on this Mac; on a remote host the Mac
  // path means nothing, so the file is uploaded like pasted data (20 MB cap).
  async function attachFile(panelId, file) {
    const entry = entryFor(panelId);
    if (entry.host === "local") return typePath(panelId, file);
    const stat = await fs.stat(file);
    if (!stat.isFile() || !stat.size || stat.size > MAX_ATTACHMENT_BYTES)
      throw new Error("Choose non-empty files up to 20 MB.");
    return attachData(panelId, path.basename(file), await fs.readFile(file));
  }

  // Pasted data without a path on disk: stored as a private file (a temp file
  // on this Mac, or ~/.sushiai/attachments on a remote host, uploaded over the
  // connection's ssh stdin), then its path is handed over like a dropped file.
  async function attachData(panelId, name, bytes) {
    const entry = entryFor(panelId);
    if (!bytes.length || bytes.length > MAX_ATTACHMENT_BYTES)
      throw new Error("Choose non-empty files up to 20 MB.");
    const safe =
      path
        .basename(name)
        .replace(/[^\w.\- ]+/g, "_")
        .slice(-80) || "pasted";
    const unique = `${randomUUID()}-${safe}`;
    if (entry.host !== "local") {
      if (!exec) throw new Error("This host cannot receive pasted data.");
      const out = await exec(
        `ssh:${entry.host}`,
        `sh -c ${quote(uploadScript(unique))}`,
        { input: Buffer.from(bytes), timeout: 120000 },
      );
      const remote = String(out).trim();
      if (!path.posix.isAbsolute(remote) || /[\r\n\0]/.test(remote))
        throw new Error("The host did not confirm the upload.");
      await typePath(panelId, remote);
      return;
    }
    await fs.mkdir(attachmentsDir, { recursive: true, mode: 0o700 });
    const file = path.join(attachmentsDir, unique);
    await fs.writeFile(file, bytes, { flag: "wx", mode: 0o600 });
    await typePath(panelId, file);
  }

  function handleEvent(event) {
    if (event?.method !== "session.exited") return;
    const id = event.params?.id;
    for (const [panelId, entry] of panels)
      if (entry.host === event.host && entry.sessionId === id)
        send("daemon-terminal-data", { panelId, exited: true });
  }
  const unsubscribe = onEvent ? onEvent(handleEvent) : undefined;

  // A host was removed or disconnected: its terminals end.
  async function closeHost(host) {
    const ids = [...panels]
      .filter(([, e]) => e.host === host)
      .map(([id]) => id);
    for (const panelId of ids) {
      send("daemon-terminal-data", { panelId, exited: true });
      await detach(panelId);
    }
  }

  async function close() {
    if (typeof unsubscribe === "function") unsubscribe();
    await Promise.all([...panels.keys()].map(detach));
  }

  return {
    attach,
    write,
    resize,
    detach,
    ack,
    attachFile,
    attachData,
    handleEvent,
    closeHost,
    close,
  };
}

module.exports = { createTerminalHandlers, FIRST_ATTACH_SCROLLBACK };
