const { activityRows, readStore, writeStore } = require("../app-db.cjs");

const MAX_BYTES = 4 * 1024 * 1024;

// Provider-neutral, bounded local journal in sushiai.db (store
// `hermes-activity`: one row per entry plus their order, so a save writes only
// what changed). Never stores prompts or tool payloads.
class ActivityStore {
  constructor(userDataDir) {
    this.userDataDir = userDataDir;
    this.entries = [];
    this.error = null;
    this.pending = Promise.resolve();
    this.writeTimer = null;
    this.writeQueued = false;
    if (!userDataDir) return;
    try {
      const rows = readStore(userDataDir, "hermes-activity");
      const entries = (Array.isArray(rows.order) ? rows.order : []).map(
        (id) => rows[`e:${id}`],
      );
      if (
        entries.some(
          (row) =>
            !row ||
            typeof row.id !== "string" ||
            typeof row.title !== "string" ||
            typeof row.createdAt !== "number",
        )
      )
        throw Error("Invalid journal");
      this.entries = entries.slice(-500);
    } catch {
      this.error =
        "Saved activity could not be loaded. New events are still shown.";
    }
  }
  save(entries) {
    this.entries = structuredClone(entries.slice(-500));
    if (!this.userDataDir) return;
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
    if (!this.userDataDir || !this.writeQueued) return this.pending;
    this.writeQueued = false;
    const data = JSON.stringify(this.entries);
    this.pending = this.pending.then(() => {
      try {
        if (Buffer.byteLength(data) > MAX_BYTES)
          throw Error("Journal too large");
        writeStore(
          this.userDataDir,
          "hermes-activity",
          activityRows(this.entries),
        );
        this.error = null;
      } catch {
        this.error =
          "Activity could not be saved. It remains available until you close sushiAI.";
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
