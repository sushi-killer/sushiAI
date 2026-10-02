const { readStore, writeStore } = require("../app-db.cjs");
class HermesScheduler {
  constructor({ factory, userDataDir, publish }) {
    this.factory = factory;
    this.userDataDir = userDataDir;
    this.publish = publish;
    this.transport = null;
    this.closed = false;
    this.queue = Promise.resolve();
    this.state = { enabled: false, status: "stopped", error: null };
    if (userDataDir)
      try {
        this.state.enabled =
          readStore(userDataDir, "hermes-scheduler").enabled === true;
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
        if (this.userDataDir)
          writeStore(this.userDataDir, "hermes-scheduler", { enabled });
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
