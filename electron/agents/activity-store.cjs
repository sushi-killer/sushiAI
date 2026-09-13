const fs = require("node:fs");
const promises = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");

// Provider-neutral, bounded local journal. Never stores prompts or tool payloads.
class ActivityStore {
  constructor(file) {
    this.file = file;
    this.entries = [];
    this.error = null;
    this.pending = Promise.resolve();
    this.writeTimer = null;
    this.writeQueued = false;
    if (!file) return;
    try {
      if (fs.statSync(file).size > 4 * 1024 * 1024)
        throw Error("Journal too large");
      const rows = JSON.parse(fs.readFileSync(file, "utf8"));
      if (
        !Array.isArray(rows) ||
        rows.some(
          (row) =>
            !row ||
            typeof row.id !== "string" ||
            typeof row.title !== "string" ||
            typeof row.createdAt !== "number",
        )
      )
        throw Error("Invalid journal");
      this.entries = rows.slice(-500);
    } catch (error) {
      if (error.code !== "ENOENT")
        this.error =
          "Saved activity could not be loaded. New events are still shown.";
    }
  }
  save(entries) {
    this.entries = structuredClone(entries.slice(-500));
    if (!this.file) return;
    // Writes are coalesced for up to 200ms to keep rapid activity updates off
    // the disk path. Normal shutdown calls close(), which flushes immediately;
    // a hard crash in this window can lose only the newest pending snapshot.
    this.writeQueued = true;
    if (this.writeTimer) return;
    this.writeTimer = setTimeout(() => {
      this.writeTimer = null;
      this.flush();
    }, 200);
    this.writeTimer.unref?.();
  }
  flush() {
    if (!this.file || !this.writeQueued) return this.pending;
    this.writeQueued = false;
    const data = JSON.stringify(this.entries);
    this.pending = this.pending.then(async () => {
      const temporary = `${this.file}.${randomUUID()}.tmp`;
      try {
        if (Buffer.byteLength(data) > 4 * 1024 * 1024)
          throw Error("Journal too large");
        await promises.mkdir(path.dirname(this.file), {
          recursive: true,
          mode: 0o700,
        });
        await promises.writeFile(temporary, data, { mode: 0o600, flag: "wx" });
        await promises.rename(temporary, this.file);
        this.error = null;
      } catch {
        this.error =
          "Activity could not be saved. It remains available until you close sushiAI.";
      } finally {
        await promises.rm(temporary, { force: true }).catch(() => {});
      }
    });
    return this.pending;
  }
  async close() {
    if (this.writeTimer) {
      clearTimeout(this.writeTimer);
      this.writeTimer = null;
    }
    this.flush();
    await this.pending;
  }
}
module.exports = { ActivityStore };
