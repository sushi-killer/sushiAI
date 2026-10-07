/** The message of a rejected call, without Electron's "Error invoking remote
 * method 'x': Error:" prefix. */
export const errorText = (error: unknown) =>
  (error instanceof Error ? error.message : String(error)).replace(
    /^Error invoking remote method '[^']*':\s*(Error:\s*)?/,
    "",
  );

/** The daemon no longer has, or no longer runs, the session the app still
 * remembers (SESSION_NOT_FOUND 1003, SESSION_NOT_RUNNING 1005). */
export const isGone = (error: unknown) =>
  typeof error === "object" &&
  error !== null &&
  "code" in error &&
  ["1003", "1005", "SESSION_NOT_FOUND", "SESSION_NOT_RUNNING"].includes(
    String(error.code),
  );
