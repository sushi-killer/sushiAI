function ipcError(error) {
  return {
    code: error?.code || "IPC_ERROR",
    message: error?.message || String(error),
    ...Object.fromEntries(
      ["data", "stage", "retryable", "created"]
        .filter((key) => error?.[key] !== undefined)
        .map((key) => [key, error[key]]),
    ),
  };
}

async function ipcResult(callback, args) {
  try {
    return { __sushiaiIpc: 1, value: await callback(...args) };
  } catch (error) {
    return { __sushiaiIpc: 1, error: ipcError(error) };
  }
}

module.exports = { ipcError, ipcResult };
