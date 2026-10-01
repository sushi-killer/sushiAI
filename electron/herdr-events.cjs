const net = require("node:net");
const { randomUUID } = require("node:crypto");
const { HERDR_CONTRACT } = require("./herdr-contract.cjs");
const { HerdrError, errorDetails } = require("./herdr.cjs");

class HerdrEvents {
  constructor({
    getConnections,
    send,
    reconnectDelay = 250,
    maxReconnectDelay = 5000,
  }) {
    this.getConnections = getConnections;
    this.send = send;
    this.reconnectDelay = reconnectDelay;
    this.maxReconnectDelay = maxReconnectDelay;
    this.endpoints = new Map();
    this.generations = new Map();
    this.closed = false;
  }

  subscribe(endpoint, subscriptionId) {
    if (this.closed) throw new Error("Herdr event subscriptions are closed.");
    let entry = this.endpoints.get(endpoint);
    if (!entry) {
      entry = {
        endpoint,
        listeners: new Set(),
        generation: 0,
        socket: null,
        timer: null,
        delay: this.reconnectDelay,
        connecting: false,
      };
      this.endpoints.set(endpoint, entry);
      const connections = this.getConnections();
      entry.unsubscribeConnection = connections.onStateChange?.((change) => {
        if (change.endpoint !== endpoint || !this.current(entry)) return;
        this.invalidate(entry);
        if (change.connected) void this.connect(entry);
        else this.notify(entry, "disconnected");
      });
    }
    entry.listeners.add(subscriptionId);
    if (!entry.socket && !entry.connecting && !entry.timer)
      void this.connect(entry);
    return { generation: entry.generation };
  }

  unsubscribe(endpoint, subscriptionId) {
    const entry = this.endpoints.get(endpoint);
    if (!entry) return;
    entry.listeners.delete(subscriptionId);
    if (entry.listeners.size) return;
    this.endpoints.delete(endpoint);
    this.invalidate(entry);
    entry.unsubscribeConnection?.();
  }

  current(entry, generation = entry.generation) {
    return (
      !this.closed &&
      this.endpoints.get(entry.endpoint) === entry &&
      entry.generation === generation
    );
  }

  invalidate(entry) {
    entry.generation = (this.generations.get(entry.endpoint) || 0) + 1;
    this.generations.set(entry.endpoint, entry.generation);
    clearTimeout(entry.timer);
    clearTimeout(entry.handshakeTimer);
    entry.handshakeTimer = null;
    entry.timer = null;
    entry.connecting = false;
    entry.socket?.destroy();
    entry.socket = null;
  }

  notify(entry, type, detail = {}) {
    if (this.current(entry))
      this.send("herdr-event", {
        endpoint: entry.endpoint,
        generation: entry.generation,
        type,
        ...detail,
      });
  }

  retry(entry, generation, error) {
    if (!this.current(entry, generation)) return;
    entry.socket?.destroy();
    entry.socket = null;
    entry.connecting = false;
    this.notify(
      entry,
      "disconnected",
      error ? { error: errorDetails(error) } : {},
    );
    entry.timer = setTimeout(() => {
      entry.timer = null;
      void this.connect(entry);
    }, entry.delay);
    entry.timer.unref?.();
    entry.delay = Math.min(entry.delay * 2, this.maxReconnectDelay);
  }

  async connect(entry) {
    if (!this.current(entry) || entry.connecting || entry.socket) return;
    this.invalidate(entry);
    entry.connecting = true;
    const generation = entry.generation;
    try {
      const connections = this.getConnections();
      const socketPath = await connections.socket(entry.endpoint);
      if (!this.current(entry, generation)) return;
      const socket = net.createConnection(socketPath);
      entry.socket = socket;
      const requestId = randomUUID();
      let buffer = "",
        started = false,
        failed = false;
      const handshakeTimer = setTimeout(
        () =>
          fail(
            new HerdrError(
              "HERDR_TIMEOUT",
              "Herdr event subscription did not respond.",
            ),
          ),
        5000,
      );
      entry.handshakeTimer = handshakeTimer;
      handshakeTimer.unref?.();
      const fail = (error) => {
        if (failed) return;
        failed = true;
        clearTimeout(handshakeTimer);
        if (entry.handshakeTimer === handshakeTimer)
          entry.handshakeTimer = null;
        this.retry(entry, generation, error);
      };
      socket.setEncoding("utf8");
      socket.on("connect", () => {
        if (!this.current(entry, generation)) return socket.destroy();
        socket.write(
          JSON.stringify({
            id: requestId,
            method: "events.subscribe",
            params: {
              subscriptions: HERDR_CONTRACT.eventTypes.map((type) => ({
                type,
              })),
            },
          }) + "\n",
        );
      });
      socket.on("error", fail);
      socket.on("close", () =>
        fail(
          new HerdrError(
            "HERDR_DISCONNECTED",
            "Herdr event stream disconnected.",
          ),
        ),
      );
      socket.on("data", (chunk) => {
        if (!this.current(entry, generation) || failed) return;
        buffer += chunk;
        if (Buffer.byteLength(buffer) > 1024 * 1024)
          return fail(
            new HerdrError(
              "HERDR_RESPONSE_TOO_LARGE",
              "Herdr event frame exceeds 1 MB.",
            ),
          );
        let boundary;
        while ((boundary = buffer.indexOf("\n")) >= 0) {
          const line = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 1);
          if (!line.trim()) continue;
          let message;
          try {
            message = JSON.parse(line);
          } catch {
            return fail(
              new HerdrError(
                "HERDR_INVALID_RESPONSE",
                "Invalid JSON in Herdr event stream.",
              ),
            );
          }
          if (!message || typeof message !== "object")
            return fail(
              new HerdrError(
                "HERDR_INVALID_RESPONSE",
                "Invalid Herdr event frame.",
              ),
            );
          if (!started) {
            if (message.id !== requestId)
              return fail(
                new HerdrError(
                  "HERDR_INVALID_RESPONSE",
                  "Invalid Herdr subscription response.",
                ),
              );
            if (message.error)
              return fail(
                new HerdrError(
                  message.error.code,
                  message.error.message,
                  message.error.data,
                ),
              );
            if (message.result?.type !== "subscription_started")
              return fail(
                new HerdrError(
                  "HERDR_INVALID_RESPONSE",
                  "Herdr did not start the event subscription.",
                ),
              );
            started = true;
            clearTimeout(handshakeTimer);
            if (entry.handshakeTimer === handshakeTimer)
              entry.handshakeTimer = null;
            entry.connecting = false;
            entry.delay = this.reconnectDelay;
            this.notify(entry, "connected");
          } else if (
            typeof message.event === "string" &&
            message.data &&
            typeof message.data === "object"
          ) {
            this.notify(entry, "changed", { event: message.event });
          } else
            return fail(
              new HerdrError(
                "HERDR_INVALID_RESPONSE",
                "Invalid Herdr event frame.",
              ),
            );
        }
      });
    } catch (error) {
      this.retry(entry, generation, error);
    }
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    for (const entry of this.endpoints.values()) {
      this.invalidate(entry);
      entry.unsubscribeConnection?.();
    }
    this.endpoints.clear();
  }
}

module.exports = { HerdrEvents };
