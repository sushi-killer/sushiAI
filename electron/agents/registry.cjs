const { EventEmitter } = require("node:events");

const VERSION = 1;
function object(value) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Expected an object.");
  return value;
}
function text(value, label, max = 200) {
  if (typeof value !== "string" || !value.trim() || value.length > max || value.includes("\0"))
    throw new Error(`Invalid ${label}.`);
  return value;
}

// Only explicitly installed modules can register operations. Renderers never
// choose an executable, RPC method, filesystem path or network endpoint here.
class AgentRegistry extends EventEmitter {
  constructor() {
    super();
    this.providers = new Map();
    this.closed = false;
  }
  register(provider) {
    if (this.closed) throw new Error("Agent registry is closed.");
    const descriptor = object(provider.descriptor);
    text(descriptor.id, "provider ID");
    if (descriptor.apiVersion !== VERSION) throw new Error("Unsupported provider contract.");
    if (this.providers.has(descriptor.id)) throw new Error("Provider is already installed.");
    if (!(provider.operations instanceof Map)) throw new Error("Provider operations must be explicit.");
    this.providers.set(descriptor.id, provider);
    provider.publish = (event) => {
      if (!this.closed) this.emit("event", { ...event, providerId: descriptor.id });
    };
  }
  list() {
    return [...this.providers.values()].map((p) => structuredClone(p.descriptor));
  }
  async call(providerId, operation, input = {}) {
    if (this.closed) throw new Error("Agent registry is closed.");
    text(providerId, "provider ID");
    text(operation, "operation");
    object(input);
    if (Buffer.byteLength(JSON.stringify(input)) > 16 * 1024 * 1024)
      throw new Error("Agent request is too large.");
    const provider = this.providers.get(providerId);
    if (!provider) throw new Error("Agent provider is not installed.");
    const handler = provider.operations.get(operation);
    if (!handler) throw new Error("This operation is not supported by the provider.");
    return handler(input);
  }
  async close() {
    if (this.closed) return;
    this.closed = true;
    await Promise.allSettled([...this.providers.values()].map((p) => p.close()));
    this.removeAllListeners();
  }
}

module.exports = { AgentRegistry, VERSION, object, text };
