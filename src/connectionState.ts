import type { Connector, DaemonState } from "./types";

export type HostAction = "retry" | "install" | "update";
export type HostView = {
  tone: "ok" | "info" | "warning" | "danger" | "neutral";
  label: string;
  /** Server message or version line, shown under the label. */
  detail?: string;
  /** Plain-language hint. */
  hint?: string;
  /** Text the owner copies into a terminal. */
  command?: string;
  action?: HostAction;
};

/** Maps one daemon state to the label, hint and action a connection row shows. */
export function describeHost(state: DaemonState | undefined): HostView {
  if (!state) return { tone: "neutral", label: "Not connected" };
  switch (state.state) {
    case "ready":
      return {
        tone: "ok",
        label: "Connected",
        detail: state.version ? `sushiai ${state.version}` : undefined,
      };
    case "connecting":
      return { tone: "info", label: "Connecting" };
    case "offline":
      return {
        tone: "warning",
        label: "Offline, reconnecting",
        detail: state.message,
      };
    case "need_auth":
      return {
        tone: "warning",
        label: "Needs sign-in",
        detail: state.message,
        hint: "sushiAI connects without a prompt, so the host needs a key. Add your key to the ssh agent (ssh-add) or set an IdentityFile for this host in ~/.ssh/config, then retry.",
        action: "retry",
      };
    case "failed":
      if (state.reason === "host_key_changed")
        return {
          tone: "danger",
          label: "Host key changed",
          detail: state.message,
          hint: "The host presented a different key. Do not retry until you trust the change. To forget the old key, run this in a terminal.",
          command: state.hint,
        };
      if (state.reason === "not_installed")
        return {
          tone: "warning",
          label: "sushiai is not installed",
          detail: state.message,
          action: "install",
        };
      if (state.reason === "incompatible")
        return {
          tone: "warning",
          label: "sushiai needs an update",
          detail: state.message || state.version,
          action: "update",
        };
      return {
        tone: "danger",
        label: "Connection failed",
        detail: state.message,
        action: "retry",
      };
  }
}

/** The button text of an action. */
export function actionLabel(action: HostAction): string {
  return action === "install"
    ? "Install sushiai"
    : action === "update"
      ? "Update sushiai"
      : "Retry";
}

/** Splits one line into argv on spaces; quotes group words, backslash escapes. */
export function parseArgv(line: string): string[] {
  const argv: string[] = [];
  let word = "";
  let started = false;
  let quote: "'" | '"' | null = null;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === quote) quote = null;
      else if (ch === "\\" && quote === '"' && i + 1 < line.length)
        word += line[++i];
      else word += ch;
    } else if (ch === "'" || ch === '"') {
      quote = ch;
      started = true;
    } else if (ch === "\\" && i + 1 < line.length) {
      word += line[++i];
      started = true;
    } else if (/\s/.test(ch)) {
      if (started || word) argv.push(word);
      word = "";
      started = false;
    } else {
      word += ch;
      started = true;
    }
  }
  if (quote) throw new Error("Close the quote in the command.");
  if (started || word) argv.push(word);
  return argv;
}

/** Inverse of parseArgv, for editing a saved command. */
export function formatArgv(argv: string[]): string {
  return argv
    .map((arg) =>
      arg !== "" && /^[^\s'"\\]+$/.test(arg)
        ? arg
        : `"${arg.replace(/(["\\])/g, "\\$1")}"`,
    )
    .join(" ");
}

/** Connector for the form: a blank command line means the default ssh. */
export function connectorFromLine(line: string): Connector {
  const argv = parseArgv(line);
  return argv.length ? { kind: "command", argv } : { kind: "ssh" };
}
