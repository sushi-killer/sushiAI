const { spawn } = require("node:child_process");

const REQUEST_TIMEOUT_MS = 20_000;
const MAX_FRAME_BYTES = 36 * 1024 * 1024;
const MAX_QUEUE_LENGTH = 32;
const MAX_QUEUE_BYTES = 64 * 1024 * 1024;

function terminateProcess(child) {
  if (!child || child.exitCode != null || child.signalCode != null)
    return Promise.resolve();
  return new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(() => {
      if (child.exitCode == null && child.signalCode == null) {
        try {
          child.kill("SIGKILL");
        } catch {}
      }
      finish();
    }, 750);
    child.once("close", finish);
    child.once("exit", finish);
    try {
      child.kill("SIGTERM");
    } catch {
      finish();
    }
  });
}

class InspectionWorker {
  constructor({
    command,
    args,
    sourceLoader,
    spawnProcess = spawn,
    label = "project inspection",
  }) {
    this.command = command;
    this.args = args;
    this.sourceLoader = sourceLoader;
    this.spawnProcess = spawnProcess;
    this.label = label;
    this.child = null;
    this.startPromise = null;
    this.closePromise = null;
    this.closed = false;
    this.queue = [];
    this.queueBytes = 0;
    this.active = null;
    this.buffer = "";
    this.framed = false;
    this.preamble = "";
    this.nextId = 1;
    this.pumping = false;
    this.starts = 0;
  }

  request(payload) {
    if (this.closed)
      return Promise.reject(new Error(`${this.label} is closed.`));
    let frame;
    try {
      frame = `${JSON.stringify({ id: String(this.nextId++), request: payload })}\n`;
    } catch {
      return Promise.reject(
        new Error("Project inspection request is not serializable."),
      );
    }
    const bytes = Buffer.byteLength(frame);
    if (bytes > MAX_FRAME_BYTES)
      return Promise.reject(
        new Error("Project inspection request is too large."),
      );
    if (
      this.queue.length >= MAX_QUEUE_LENGTH ||
      this.queueBytes + bytes > MAX_QUEUE_BYTES
    )
      return Promise.reject(
        new Error("Project inspection is busy. Try again shortly."),
      );

    return new Promise((resolve, reject) => {
      const item = {
        id: String(this.nextId - 1),
        frame,
        bytes,
        deadline: Date.now() + REQUEST_TIMEOUT_MS,
        resolve,
        reject,
        settled: false,
        timer: null,
      };
      item.timer = setTimeout(() => this.expire(item), REQUEST_TIMEOUT_MS);
      item.timer.unref?.();
      this.queue.push(item);
      this.queueBytes += bytes;
      void this.pump();
    });
  }

  async ensureStarted() {
    if (this.child) return;
    if (!this.startPromise) {
      this.startPromise = this.start().finally(() => {
        this.startPromise = null;
      });
    }
    await this.startPromise;
  }

  async start() {
    const source = await this.sourceLoader();
    if (this.closed) throw new Error(`${this.label} is closed.`);
    const child = this.spawnProcess(this.command, this.args(source), {
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.starts += 1;
    this.child = child;
    this.buffer = "";
    this.framed = false;
    this.preamble = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    let stderr = "";
    child.stdout.on("data", (chunk) => this.receive(chunk, child));
    child.stderr.on("data", (chunk) => {
      stderr = (stderr + chunk).slice(-4000);
    });
    child.stdin.on("error", (error) => {
      if (this.child === child) this.invalidate(error);
    });
    child.once("error", (error) => {
      if (this.child === child) this.invalidate(error);
    });
    child.once("close", (code, signal) => {
      if (this.child !== child || this.closed) return;
      this.child = null;
      this.buffer = "";
      this.rejectPending(
        new Error(
          stderr ||
            `${this.label} stopped (${signal || `exit ${code ?? "unknown"}`}).`,
        ),
      );
    });
  }

  async pump() {
    if (this.pumping) return;
    this.pumping = true;
    try {
      while (!this.closed && !this.active && this.queue.length) {
        const item = this.queue.shift();
        this.queueBytes -= item.bytes;
        if (Date.now() >= item.deadline) {
          this.rejectItem(
            item,
            new Error("Project inspection timed out before dispatch."),
          );
          continue;
        }
        try {
          await this.ensureStarted();
          if (Date.now() >= item.deadline) {
            this.rejectItem(
              item,
              new Error("Project inspection timed out before dispatch."),
            );
            continue;
          }
          this.active = item;
          await this.write(item.frame);
        } catch (error) {
          if (this.active !== item) this.rejectItem(item, error);
          this.invalidate(error);
          break;
        }
      }
    } finally {
      this.pumping = false;
    }
  }

  async write(frame) {
    const child = this.child;
    if (!child || !child.stdin || child.stdin.destroyed)
      throw new Error(`${this.label} is unavailable.`);
    let settled = false;
    await new Promise((resolve, reject) => {
      const finish = (error) => {
        if (settled) return;
        settled = true;
        child.stdin.removeListener("drain", onDrain);
        child.stdin.removeListener("error", onError);
        error ? reject(error) : resolve();
      };
      const onDrain = () => finish();
      const onError = (error) => finish(error);
      child.stdin.once("error", onError);
      try {
        if (child.stdin.write(frame, "utf8")) finish();
        else child.stdin.once("drain", onDrain);
      } catch (error) {
        finish(error);
      }
    });
  }

  receive(chunk, owner = this.child) {
    if (owner !== this.child) return;
    this.buffer += chunk;
    if (
      Buffer.byteLength(this.buffer) > MAX_FRAME_BYTES &&
      !this.buffer.includes("\n")
    ) {
      this.invalidate(
        new Error(`${this.label} returned an oversized response.`),
      );
      return;
    }
    while (true) {
      const newline = this.buffer.indexOf("\n");
      if (newline < 0) return;
      const line = this.buffer.slice(0, newline);
      this.buffer = this.buffer.slice(newline + 1);
      if (Buffer.byteLength(line) > MAX_FRAME_BYTES) {
        this.invalidate(
          new Error(`${this.label} returned an oversized response.`),
        );
        return;
      }
      let envelope;
      try {
        envelope = JSON.parse(line);
      } catch {
        // A remote shell greets before the worker speaks: an SSH banner or a
        // noisy profile lands ahead of the first frame. Skip that preamble and
        // remember it, so a later desync still reports something actionable.
        if (!this.framed) {
          this.preamble =
            `${this.preamble}${this.preamble ? "\n" : ""}${line}`.slice(-2000);
          continue;
        }
        this.invalidate(new Error(`${this.label} returned malformed data.`));
        return;
      }
      this.framed = true;
      const item = this.active;
      if (
        !item ||
        typeof envelope !== "object" ||
        envelope === null ||
        envelope.id !== item.id
      ) {
        this.invalidate(
          new Error(
            `${this.label} returned an unexpected response.${this.preambleHint()}`,
          ),
        );
        return;
      }
      const hasError = Object.prototype.hasOwnProperty.call(envelope, "error");
      const hasResult = Object.prototype.hasOwnProperty.call(
        envelope,
        "result",
      );
      if (!hasError && !hasResult) {
        this.invalidate(
          new Error(`${this.label} returned an invalid response.`),
        );
        return;
      }
      this.active = null;
      if (hasError)
        this.rejectItem(
          item,
          new Error(String(envelope.error || "Inspection failed.")),
        );
      else this.resolveItem(item, envelope.result);
      void this.pump();
    }
  }

  /** Names the shell startup output when the stream never made sense, which is
   * the usual cause on a remote host. */
  preambleHint() {
    return this.preamble
      ? ` Check SSH shell startup output: ${this.preamble.slice(0, 300)}`
      : "";
  }

  expire(item) {
    if (item.settled) return;
    if (this.active === item) {
      // The worker is serialized, so a stuck request has to take the process
      // with it - but the requests queued behind it are innocent and are
      // replayed against the replacement worker.
      this.invalidate(
        new Error(
          "Project inspection timed out; the operation outcome is unknown.",
        ),
        { keepQueue: true },
      );
      return;
    }
    const index = this.queue.indexOf(item);
    if (index >= 0) {
      this.queue.splice(index, 1);
      this.queueBytes -= item.bytes;
      this.rejectItem(
        item,
        new Error("Project inspection timed out before dispatch."),
      );
      void this.pump();
    }
  }

  invalidate(error, { keepQueue = false } = {}) {
    const child = this.child;
    this.child = null;
    this.buffer = "";
    this.framed = false;
    this.preamble = "";
    this.rejectPending(error, keepQueue);
    if (child) void terminateProcess(child);
    if (keepQueue && this.queue.length) void this.pump();
  }

  rejectPending(error, keepQueue = false) {
    if (this.active) {
      const item = this.active;
      this.active = null;
      this.rejectItem(item, error);
    }
    if (keepQueue) return;
    for (const item of this.queue.splice(0)) this.rejectItem(item, error);
    this.queueBytes = 0;
  }

  resolveItem(item, value) {
    if (item.settled) return;
    item.settled = true;
    clearTimeout(item.timer);
    item.resolve(value);
  }

  rejectItem(item, error) {
    if (item.settled) return;
    item.settled = true;
    clearTimeout(item.timer);
    item.reject(error);
  }

  async close(reason = `${this.label} closed.`) {
    if (this.closePromise) return this.closePromise;
    this.closePromise = (async () => {
      this.closed = true;
      this.rejectPending(new Error(reason));
      const starting = this.startPromise;
      const child = this.child;
      this.child = null;
      if (child) await terminateProcess(child);
      await starting?.catch(() => {});
    })();
    return this.closePromise;
  }
}

module.exports = {
  InspectionWorker,
  MAX_FRAME_BYTES,
  MAX_QUEUE_BYTES,
  MAX_QUEUE_LENGTH,
  REQUEST_TIMEOUT_MS,
};
