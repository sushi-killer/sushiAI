/** The message of a rejected call, without Electron's "Error invoking remote
 * method 'x': Error:" prefix. */
export const errorText = (error: unknown) =>
  (error instanceof Error ? error.message : String(error)).replace(
    /^Error invoking remote method '[^']*':\s*(Error:\s*)?/,
    "",
  );

/** Herdr no longer has the pane or workspace the app still remembers. */
export const isGone = (error: unknown) =>
  /no longer open on the host/i.test(errorText(error));
