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
const MAX_ATTACHMENT_BYTES = 20 * 1024 * 1024;

// A shell reads the path as one word however the file was named.
const shellPath = (value) => `'${value.replace(/'/g, "'\\''")}' `;

function createTerminalHandlers({
  getManager,
  send,
  onEvent,
  attachmentsDir = path.join(os.tmpdir(), "sushiai-attachments"),
}) {
  const panels = new Map(); // panelId -> entry
  const attachedBefore = new Set(); // host\0sessionId attached in this app run

  function entryFor(panelId) {
    const entry = panels.get(panelId);
    if (!entry) throw new Error("This terminal is not attached.");
    return entry;
  }

  function createEntry(panelId, host, sessionId, cols, rows) {
    const decoder = new StringDecoder("utf8");
    const entry = {
      host,
      sessionId,
      handle: undefined,
      cols,
      rows,
      closed: false,
      queue: [], // ordered strings waiting for credit
      inFlight: 0,
    };
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
        // A character takes at most 3 bytes in UTF-16 units, so this never overshoots.
        let end = Math.max(1, Math.floor(room / 3));
        let piece = head;
        if (end < head.length) {
          if (/^[\uDC00-\uDFFF]$/.test(head[end])) end--;
          piece = head.slice(0, end);
          entry.queue[0] = head.slice(end);
        } else entry.queue.shift();
        entry.inFlight += Buffer.byteLength(piece);
        send("daemon-terminal-data", { panelId, data: piece });
      }
    }
    entry.flush = flush;
    entry.onBytes = (buffer, info = {}) => {
      if (entry.closed) return;
      if (info.snapshot) {
        // The snapshot replaces everything older, queued or not.
        decoder.end();
        entry.queue = [];
        entry.inFlight = 0;
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
      if (text) {
        entry.queue.push(text);
        flush();
      }
    };
    return entry;
  }

  async function attach({ panelId, host, sessionId, cols, rows }) {
    await detach(panelId);
    const manager = getManager();
    const key = `${host}\0${sessionId}`;
    const scrollback = attachedBefore.has(key) ? 0 : FIRST_ATTACH_SCROLLBACK;
    const entry = createEntry(panelId, host, sessionId, cols, rows);
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
      attachedBefore.add(key);
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
    entry.inFlight = Math.max(0, entry.inFlight - bytes);
    entry.flush();
  }

  async function attachFile(panelId, path) {
    await write(panelId, shellPath(path));
  }

  // Pasted data without a path on disk: stored as a private temp file on this
  // Mac, then its path is handed over like a dropped file. Local host only.
  async function attachData(panelId, name, bytes) {
    const entry = entryFor(panelId);
    if (entry.host !== "local")
      throw new Error("attaching pasted data to a remote host comes later");
    if (!bytes.length || bytes.length > MAX_ATTACHMENT_BYTES)
      throw new Error("Choose non-empty files up to 20 MB.");
    const safe =
      path
        .basename(name)
        .replace(/[^\w.\- ]+/g, "_")
        .slice(-80) || "pasted";
    await fs.mkdir(attachmentsDir, { recursive: true, mode: 0o700 });
    const file = path.join(attachmentsDir, `${randomUUID()}-${safe}`);
    await fs.writeFile(file, bytes, { flag: "wx", mode: 0o600 });
    await attachFile(panelId, file);
  }

  function handleEvent(event) {
    if (event?.method !== "session.exited") return;
    const id = event.params?.id;
    for (const [panelId, entry] of panels)
      if (entry.host === event.host && entry.sessionId === id)
        send("daemon-terminal-data", { panelId, exited: true });
  }
  const unsubscribe = onEvent ? onEvent(handleEvent) : undefined;

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
    close,
  };
}

module.exports = { createTerminalHandlers, FIRST_ATTACH_SCROLLBACK };
