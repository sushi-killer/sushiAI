import type { ProjectGitFailure } from "../types";

/** What the failure screens say, said honestly: which value (if any) was
 * used, and what the owner can do. Pure, so the wording is tested. */
export type CopyInput = {
  hostName: string;
  repo: string;
  /** The variable the clone would use, or null when there is none. */
  token: string | null;
  /** The host is switched off for this project: no value is sent. */
  noSecrets: boolean;
  status?: number;
  timedOut?: boolean;
  message: string;
  /** The step the run stopped in: "clone", or "setup" for install/check. */
  stage?: string;
  /** Secrets the install would see when values are sent, by name. */
  setupSecrets?: string[];
  /** How many non-secret variables the install would see as well. */
  setupOthers?: number;
  git?: ProjectGitFailure;
};

const plural = (count: number, noun: string) =>
  `${count} other ${count === 1 ? noun : `${noun}s`}`;

/** What reached the commands on the host at the point the run stopped. A
 * clone that failed never ran an install, so only its token was in play; an
 * install that failed ran with the setup variables. Only
 * secrets are named, the rest are counted. */
export function sentLine(input: CopyInput): string {
  if (input.git?.transport === "ssh" && input.stage !== "setup")
    return "SSH used the remote host’s keys. The install did not run.";
  if (input.noSecrets)
    return `No value was sent to ${input.hostName}; the clone used its own git login.`;
  if (input.stage !== "setup")
    return input.token
      ? `${input.token} was used for the clone. The install did not run, so no other value was used.`
      : "No value was used; the install did not run.";
  const names = input.setupSecrets ?? [];
  const others = input.setupOthers ?? 0;
  const seen = [...names, ...(others > 0 ? [plural(others, "variable")] : [])];
  const install = seen.length
    ? `the install ran with ${seen.join(", ")}`
    : "the install ran with no project values";
  return input.token
    ? `${input.token} was used for the clone, and ${install}.`
    : `${install[0].toUpperCase()}${install.slice(1)}.`;
}

/** The sub line of the clone row when the clone failed. */
export function cloneFailure(input: CopyInput): string | null {
  if (input.git) {
    const transport = input.git.transport === "ssh" ? "SSH" : "HTTPS";
    const problems = {
      auth: "repository access was refused",
      "host-key": input.git.changed
        ? "the Git server’s key changed"
        : "the Git server’s key is not trusted yet",
      network: "the Git server could not be reached",
      branch: "the requested branch was not found",
      path: "the checkout destination is unavailable",
      other: "the checkout failed",
    };
    return `${transport}: ${problems[input.git.kind]}`;
  }
  if (input.status !== 403) return null;
  return input.noSecrets
    ? `git clone failed: ${input.hostName}’s git login has no access to ${input.repo} (403)`
    : `git clone failed: ${input.token ?? `${input.hostName}’s git login`} has no access to ${input.repo} (403)`;
}

/** The "What you can do" paragraph. */
export function advice(input: CopyInput): string {
  if (input.timedOut)
    return `${input.message} ${sentLine(input)} Try again; a slow install usually finishes the second time.`;
  if (input.git) {
    const action = {
      auth: `Add the host’s public SSH key to an account or deploy key with read access, then try again.${input.noSecrets ? "" : " You can also use an HTTPS token."}`,
      "host-key":
        "Check and verify the Git server’s fingerprint below before trusting it, then try again.",
      network:
        "Check the Git server address and SSH port below, then try again. If the host cannot reach it, check the server or network.",
      branch:
        "Check the repository’s branch in Project settings, then try again.",
      path: "Choose another project folder or resolve the destination conflict, then try again. Existing files are kept.",
      other: "Read the failed step’s details, fix the cause, then try again.",
    };
    return `${action[input.git.kind]} ${sentLine(input)}`;
  }
  if (input.status === 403)
    return input.noSecrets
      ? `${input.hostName}’s git login cannot read ${input.repo}. ${sentLine(input)} Give its git login access, or turn off “Don’t send secrets to this host” in Project settings → Hosts to use ${input.token ?? "a git token"}.`
      : `Give the token read access to ${input.repo}, or use ${input.hostName}’s own git login. ${sentLine(input)}`;
  return `${sentLine(input)} Fix the step above, then try again.`;
}
