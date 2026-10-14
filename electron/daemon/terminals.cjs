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
const { quote } = require("../connections.cjs");
const {
  OUTPUT_CREDIT_BYTES,
  OUTPUT_CHUNK_BYTES,
} = require("../terminal-flow.cjs");

const FIRST_ATTACH_SCROLLBACK = 2000;
const MAX_QUEUED_BYTES = 4 * OUTPUT_CREDIT_BYTES;
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const encoder = new TextEncoder();
// Where flush measures a message: encodeInto counts the bytes that fit.
const scratch = new Uint8Array(OUTPUT_CHUNK_BYTES);

// A shell reads the path as one word however the file was named.
const shellPath = (value) => `'${value.replace(/'/g, "'\\''")}' `;

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

  // The queue is dropped and output waits for a fresh snapshot. A renderer
  // that has not yet acked the window before its last snapshot is still
  // behind: another snapshot now would only be replaced, so the re-attach
  // waits for that ack (see `ack`) and output is dropped meanwhile.
  function overflow(panelId, entry) {
    if (entry.resyncing) return;
    entry.resyncing = true;
    entry.queue = [];
    entry.queuedBytes = 0;
    if (entry.stale > 0) entry.resyncDue = true;
    else reattach(panelId, entry);
  }

  function reattach(panelId, entry) {
    setImmediate(() => {
      if (entry.closed || panels.get(panelId) !== entry) return;
      attachPanel(
        {
          panelId,
          host: entry.host,
          sessionId: entry.sessionId,
          cols: entry.cols,
          rows: entry.rows,
        },
        true,
      ).catch(() => send("daemon-terminal-data", { panelId, exited: true }));
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
      scheduled: false, // a flush is due on the next turn
      resyncing: false, // output is dropped until the next snapshot
      resyncDue: false, // the re-attach waits for `stale` to be acked
      exitPending: false, // the session exited: tell the renderer once drained
    };
    // Sends queued text while credit lasts, merged into messages of at most
    // OUTPUT_CHUNK_BYTES. encodeInto stops before a character that does not
    // fit, so a message ends on a whole code point; when not even the next
    // character fits, the rest waits for an ack. `all` ignores the credit: the
    // host is gone, so nothing more arrives and nothing would ack it later.
    function flush(all = false) {
      while (
        !entry.closed &&
        entry.queue.length &&
        (all || entry.inFlight < OUTPUT_CREDIT_BYTES)
      ) {
        const room = all
          ? OUTPUT_CHUNK_BYTES
          : Math.min(OUTPUT_CHUNK_BYTES, OUTPUT_CREDIT_BYTES - entry.inFlight);
        const parts = [];
        let bytes = 0;
        while (entry.queue.length && bytes < room) {
          const head = entry.queue[0];
          const { read, written } = encoder.encodeInto(
            head,
            scratch.subarray(0, room - bytes),
          );
          if (read) parts.push(read < head.length ? head.slice(0, read) : head);
          bytes += written;
          if (read < head.length) {
            entry.queue[0] = head.slice(read);
            break;
          }
          entry.queue.shift();
        }
        if (!bytes) break;
        entry.queuedBytes -= bytes;
        entry.inFlight += bytes;
        send("daemon-terminal-data", { panelId, data: parts.join("") });
      }
      settle();
    }
    // The exit goes out after every byte queued before it, or after the
    // snapshot that replaced them.
    function settle() {
      if (
        !entry.exitPending ||
        entry.closed ||
        entry.queue.length ||
        entry.resyncing
      )
        return;
      entry.exitPending = false;
      send("daemon-terminal-data", { panelId, exited: true });
    }
    // Output that arrives in one event-loop turn leaves in one flush.
    function schedule() {
      if (entry.scheduled) return;
      entry.scheduled = true;
      setImmediate(() => {
        entry.scheduled = false;
        flush();
      });
    }
    entry.flush = flush;
    // An ack releases waiting output at once unless a flush is due anyway.
    entry.release = () => {
      if (!entry.scheduled) flush();
    };
    entry.onBytes = (buffer, info = {}) => {
      if (entry.closed) return;
      if (info.snapshot) {
        // The snapshot replaces everything older, queued or not. Bytes sent
        // before it and not yet acked still come back as acks: they must not
        // shrink the credit of the new window (`stale`).
        decoder.end();
        entry.queue = [];
        entry.queuedBytes = 0;
        entry.resyncing = entry.resyncDue = false;
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
        settle();
        return;
      }
      const text = decoder.write(buffer);
      if (!text || entry.resyncing) return;
      entry.queue.push(text);
      entry.queuedBytes += Buffer.byteLength(text);
      schedule();
      // A renderer that stopped acking (closed or throttled window) must not
      // grow this queue without bound: drop it and take a fresh snapshot. The
      // credit still open goes out with the next flush and does not count.
      const open = Math.max(0, OUTPUT_CREDIT_BYTES - entry.inFlight);
      if (entry.queuedBytes - open > MAX_QUEUED_BYTES) overflow(panelId, entry);
    };
    return entry;
  }

  // A renderer attach starts with full credit: after a reload or a remount the
  // acks of the old window never come. Only the overflow re-attach (`carry`),
  // where the same renderer still acks, carries the unacked bytes: they come
  // back as acks and belong to the old window, not to the new one.
  const attach = (input) => attachPanel(input, false);

  async function attachPanel({ panelId, host, sessionId, cols, rows }, carry) {
    const before = panels.get(panelId);
    const owed = carry && before ? before.inFlight + before.stale : 0;
    await detach(panelId);
    const manager = getManager();
    // A new attach always lands in an empty terminal, so it brings the history.
    const scrollback = FIRST_ATTACH_SCROLLBACK;
    const entry = createEntry(panelId, host, sessionId, cols, rows, owed);
    // An exit still waiting for the old queue now waits for this snapshot.
    entry.exitPending = carry && !!before?.exitPending;
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
    // The renderer caught up with its last snapshot: the resync can go out.
    if (entry.resyncDue && !entry.stale) {
      entry.resyncDue = false;
      reattach(panelId, entry);
    }
    entry.release();
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
    for (const entry of panels.values())
      if (entry.host === event.host && entry.sessionId === id) {
        entry.exitPending = true;
        entry.flush(); // the exit follows the output queued before it
      }
  }
  const unsubscribe = onEvent ? onEvent(handleEvent) : undefined;

  // A host was removed or disconnected: its terminals end.
  async function closeHost(host) {
    const ids = [...panels]
      .filter(([, e]) => e.host === host)
      .map(([id]) => id);
    for (const panelId of ids) {
      const entry = panels.get(panelId);
      if (!entry) continue;
      entry.exitPending = true;
      entry.flush(true);
      // A resync that was due will not come: the exit goes out anyway.
      if (entry.exitPending)
        send("daemon-terminal-data", { panelId, exited: true });
      await detach(panelId);
    }
  }

  // The renderer went away (reload, crash): nothing acks its terminals any more.
  async function detachAll() {
    await Promise.all([...panels.keys()].map(detach));
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
    detachAll,
    close,
  };
}

module.exports = { createTerminalHandlers, FIRST_ATTACH_SCROLLBACK };
