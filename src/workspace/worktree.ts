import type { Panel } from "../types";

/** A local-time, minute-precision suggestion for the branch field, prefilled
 * when the "New worktree" launch choice opens. */
export function suggestWorktreeBranch(now: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  const year = now.getFullYear();
  const month = pad(now.getMonth() + 1);
  const day = pad(now.getDate());
  const hours = pad(now.getHours());
  const minutes = pad(now.getMinutes());
  return `sushi/${year}${month}${day}-${hours}${minutes}`;
}

const FORBIDDEN_SEQUENCES = [
  "..",
  "~",
  "^",
  ":",
  "?",
  "*",
  "[",
  "\\",
  "@{",
  "//",
];

function hasWhitespaceOrControlChar(name: string): boolean {
  if (/\s/.test(name)) return true;
  for (let i = 0; i < name.length; i++)
    if (name.charCodeAt(i) <= 0x1f) return true;
  return false;
}

/** Git check-ref-format basics, good enough for a branch name typed into the
 * launch dialog - not the full grammar, just the failure modes reachable
 * from this input. Mirrored in `electron/worktree.cjs`, which cannot import
 * a .ts source and so keeps its own copy of the same rules. */
export function worktreeBranchError(name: string): string {
  if (!name) return "Branch name is required.";
  if (name.length > 100) return "Branch name is too long.";
  if (hasWhitespaceOrControlChar(name))
    return "Branch name cannot contain whitespace.";
  for (const sequence of FORBIDDEN_SEQUENCES)
    if (name.includes(sequence))
      return `Branch name cannot contain "${sequence}".`;
  if (name.startsWith("-")) return 'Branch name cannot start with "-".';
  if (name.startsWith("/")) return 'Branch name cannot start with "/".';
  if (name.endsWith("/")) return 'Branch name cannot end with "/".';
  if (name.endsWith(".")) return 'Branch name cannot end with ".".';
  if (name.endsWith(".lock")) return 'Branch name cannot end with ".lock".';
  return "";
}

/** Only a session runs somewhere: a terminal or an agent. Files, Browser and
 * Thread are views of the project and open in the checkout they are given, so
 * the picker must not promise them a worktree either. */
export function launchesInWorktree(kind: Panel["kind"]): boolean {
  return kind === "terminal" || kind === "agent";
}

export function worktreeBaseRef(branch: {
  ref: string;
  local: boolean;
}): string {
  return `refs/${branch.local ? "heads" : "remotes"}/${branch.ref}`;
}
