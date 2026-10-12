function createModuleIpc({ handle }) {
  const modules = new Map();

  handle("module:invoke", async (moduleId, method, ...args) => {
    if (typeof moduleId !== "string" || !modules.has(moduleId))
      throw new Error(
        `Unknown module: ${typeof moduleId === "string" ? moduleId : typeof moduleId}`,
      );
    const methods = modules.get(moduleId);
    if (
      typeof method !== "string" ||
      !Object.hasOwn(methods, method) ||
      typeof methods[method] !== "function"
    )
      throw new Error(
        `Unknown method: ${moduleId}.${typeof method === "string" ? method : typeof method}`,
      );
    return methods[method](...args);
  });

  return {
    register(moduleId, methods) {
      if (modules.has(moduleId))
        throw new Error(`Duplicate module: ${moduleId}`);
      modules.set(moduleId, methods);
    },
  };
}

module.exports = { createModuleIpc };
