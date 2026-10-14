// Unsent text of the Orchestrator composers, kept in this browser per host,
// repo and composer so a project switch (which remounts the panel) keeps it.

export type ComposerSlot = "task" | "chat";

export interface DraftStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function draftKey(host: string, repo: string, slot: ComposerSlot): string {
  return `sushiai.orchestrator.draft:${JSON.stringify([host, repo, slot])}`;
}

function defaultStorage(): DraftStorage {
  return window.localStorage;
}

export function readComposerDraft(
  host: string,
  repo: string,
  slot: ComposerSlot,
  storage?: DraftStorage,
): string {
  try {
    const raw = (storage ?? defaultStorage()).getItem(
      draftKey(host, repo, slot),
    );
    return typeof raw === "string" ? raw : "";
  } catch {
    return "";
  }
}

export function writeComposerDraft(
  host: string,
  repo: string,
  slot: ComposerSlot,
  text: string,
  storage?: DraftStorage,
): void {
  try {
    const target = storage ?? defaultStorage();
    const key = draftKey(host, repo, slot);
    if (text) target.setItem(key, text);
    else target.removeItem(key);
  } catch {
    // Storage blocked: the draft just does not survive a project switch.
  }
}
