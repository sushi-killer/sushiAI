export const HERDR_RECONCILE_MS = 60_000;

/** One snapshot flight per endpoint, with a trailing read after invalidation. */
export class SnapshotCoordinator<T> {
  private pending: Promise<void> | null = null;
  private requested = false;
  private revision = 0;
  private closed = false;
  private paused = false;
  private readonly request: () => Promise<T>;
  private readonly apply: (snapshot: T) => void;
  private readonly fail: (error: unknown) => void;

  constructor(
    request: () => Promise<T>,
    apply: (snapshot: T) => void,
    fail: (error: unknown) => void,
  ) {
    this.request = request;
    this.apply = apply;
    this.fail = fail;
  }

  refresh(): Promise<void> {
    if (this.closed || this.paused) return Promise.resolve();
    this.requested = true;
    if (!this.pending) {
      this.pending = Promise.resolve()
        .then(() => this.drain())
        .finally(() => {
          this.pending = null;
        });
    }
    return this.pending;
  }

  invalidate() {
    this.revision++;
    if (this.pending) this.requested = true;
  }

  disconnect() {
    this.paused = true;
    this.invalidate();
  }

  reconnect() {
    this.paused = false;
    this.invalidate();
    return this.refresh();
  }

  close() {
    this.closed = true;
    this.invalidate();
  }

  private async drain() {
    while (this.requested && !this.closed && !this.paused) {
      this.requested = false;
      const revision = this.revision;
      try {
        const snapshot = await this.request();
        if (!this.closed && !this.paused && revision === this.revision)
          this.apply(snapshot);
      } catch (error) {
        if (!this.closed && !this.paused && revision === this.revision)
          this.fail(error);
      }
    }
  }
}
