function unwrapIpc(value: unknown): unknown {
  if (
    !value ||
    typeof value !== "object" ||
    !("__sushiaiIpc" in value) ||
    value.__sushiaiIpc !== 1
  )
    return value;
  if ("error" in value && value.error && typeof value.error === "object") {
    const details = value.error;
    const message =
      "message" in details && typeof details.message === "string"
        ? details.message
        : "IPC request failed";
    throw Object.assign(new Error(message), details);
  }
  return "value" in value ? value.value : undefined;
}

export function wrapBridge<T extends object>(bridge: T): T {
  return new Proxy(
    { ...bridge },
    {
      get(target, property, receiver) {
        const value = Reflect.get(target, property, receiver);
        if (typeof value !== "function") return value;
        return (...args: unknown[]) => {
          const result = Reflect.apply(value, target, args);
          return result instanceof Promise ? result.then(unwrapIpc) : result;
        };
      },
    },
  );
}
