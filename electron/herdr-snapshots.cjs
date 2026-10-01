const { request } = require("./herdr.cjs");

class HerdrSnapshots {
  constructor({ getConnections, rpc = request }) {
    this.getConnections = getConnections;
    this.rpc = rpc;
    this.endpoints = new Map();
  }

  entry(endpoint) {
    let entry = this.endpoints.get(endpoint);
    if (!entry) {
      entry = { revision: 0, pending: null };
      this.endpoints.set(endpoint, entry);
    }
    return entry;
  }

  invalidate(endpoint) {
    this.entry(endpoint).revision++;
  }

  read(endpoint) {
    const entry = this.entry(endpoint);
    if (!entry.pending)
      entry.pending = Promise.resolve()
        .then(async () => {
          for (;;) {
            const revision = entry.revision;
            try {
              const socket = await this.getConnections().socket(endpoint);
              if (revision !== entry.revision) continue;
              const response = await this.rpc(socket, "session.snapshot");
              if (revision === entry.revision) return response;
            } catch (error) {
              if (revision === entry.revision) throw error;
            }
          }
        })
        .finally(() => {
          entry.pending = null;
        });
    return entry.pending;
  }
}

module.exports = { HerdrSnapshots };
