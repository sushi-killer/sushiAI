const fs = require("node:fs");
const fsp = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
class HermesScheduler {
  constructor({ factory, file, publish }) {
    this.factory = factory;
    this.file = file;
    this.publish = publish;
    this.transport = null;
    this.closed = false;
    this.queue = Promise.resolve();
    this.state = { enabled: false, status: "stopped", error: null };
    if (file)
      try {
        this.state.enabled =
          JSON.parse(fs.readFileSync(file, "utf8")).enabled === true;
      } catch {}
    if (this.state.enabled)
      queueMicrotask(() => void this.set(true).catch(() => {}));
  }
  snapshot() {
    return { ...this.state };
  }
  set(enabled) {
    if (typeof enabled !== "boolean")
      return Promise.reject(Error("Invalid scheduler state."));
    const work = this.queue
      .catch(() => {})
      .then(async () => {
        if (this.closed) throw Error("Scheduler is closed.");
        this.state.enabled = enabled;
        if (enabled && this.state.status !== "ready") {
          await this.transport?.close();
          if (this.closed) throw Error("Scheduler is closed.");
          this.state.status = "starting";
          this.state.error = null;
          this.transport = this.factory({
            profile: "default",
            env: { HERMES_DESKTOP: "1" },
            onState: (s) => {
              if (this.closed) return;
              this.state.status = s.state;
              if (s.state === "error")
                this.state.error =
                  "The Hermes schedule executor stopped. Turn automatic scheduling off and on to retry.";
              this.publish({ type: "scheduler", state: this.snapshot() });
            },
          });
          try {
            await this.transport.start();
          } catch (error) {
            this.state.status = "error";
            this.state.error = error.message;
            throw error;
          }
        } else if (!enabled) {
          await this.transport?.close();
          this.transport = null;
          this.state.status = "stopped";
          this.state.error = null;
        }
        if (this.file) {
          const temporary = `${this.file}.${randomUUID()}.tmp`;
          try {
            await fsp.mkdir(path.dirname(this.file), {
              recursive: true,
              mode: 0o700,
            });
            await fsp.writeFile(temporary, JSON.stringify({ enabled }), {
              mode: 0o600,
              flag: "wx",
            });
            await fsp.rename(temporary, this.file);
          } finally {
            await fsp.rm(temporary, { force: true }).catch(() => {});
          }
        }
        this.publish({ type: "scheduler", state: this.snapshot() });
        return this.snapshot();
      });
    this.queue = work;
    return work;
  }
  async close() {
    this.closed = true;
    await this.transport?.close();
    await this.queue.catch(() => {});
    await this.transport?.close();
  }
}
module.exports = { HermesScheduler };
