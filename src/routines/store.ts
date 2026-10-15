// One shared copy of the routines of every host, kept fresh from
// `routines.event`. The page, the Inbox rows and the settings tab read it
// through `useRoutinesStore`; the first reader starts it, the last stops it.
import { useEffect, useSyncExternalStore } from "react";
import type { AttentionItem } from "../extensions/modules.ts";
import { attentionFromEntries, attentionFromEvent } from "./attention.ts";
import { routinesClient, type Unreachable } from "./client.ts";
import { ROUTINES_EXTENSION_ID } from "./id.ts";
import type { RoutineEntry, RoutinesEvent } from "./types.ts";

export type RoutinesSnapshot = {
  entries: RoutineEntry[];
  unreachable: Unreachable[];
  loaded: boolean;
  error: string;
  /** What the Inbox shows: derived rows, minus dismissed ones. */
  attention: AttentionItem[];
};

const DISMISSED_KEY = "sushiai.routines.dismissed";
const DISMISSED_MAX = 200;
const REFRESH_DELAY_MS = 250;

function loadDismissed(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(DISMISSED_KEY) || "[]");
    return Array.isArray(raw) ? raw.filter((k) => typeof k === "string") : [];
  } catch {
    return [];
  }
}

let dismissed = new Set<string>(loadDismissed());
let eventItems: AttentionItem[] = [];
let snapshot: RoutinesSnapshot = {
  entries: [],
  unreachable: [],
  loaded: false,
  error: "",
  attention: [],
};
const listeners = new Set<() => void>();
let readers = 0;
let off: (() => void) | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let generation = 0;

function publish(next: Partial<RoutinesSnapshot>) {
  const merged = { ...snapshot, ...next };
  const rows = new Map<string, AttentionItem>();
  for (const item of [...eventItems, ...attentionFromEntries(merged.entries)])
    rows.set(item.key, item);
  merged.attention = [...rows.values()].filter(
    (item) => !dismissed.has(item.key),
  );
  snapshot = merged;
  for (const listener of [...listeners]) listener();
}

export async function refreshRoutines(): Promise<void> {
  const mine = ++generation;
  try {
    const result = await routinesClient.list();
    if (mine !== generation) return;
    // The list is the truth: rows an event announced are now in it or gone.
    eventItems = [];
    publish({
      entries: result.routines,
      unreachable: result.unreachable,
      loaded: true,
      error: "",
    });
  } catch (error) {
    if (mine !== generation) return;
    publish({
      loaded: true,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function scheduleRefresh() {
  if (timer) return;
  timer = setTimeout(() => {
    timer = null;
    void refreshRoutines();
  }, REFRESH_DELAY_MS);
}

function onEvent(message: { moduleId: string; host: string; event: unknown }) {
  if (message.moduleId !== ROUTINES_EXTENSION_ID) return;
  const event = message.event as RoutinesEvent;
  if (!event || typeof event !== "object") return;
  const item = attentionFromEvent(event, message.host);
  if (item) {
    eventItems = [...eventItems.filter((i) => i.key !== item.key), item];
    publish({});
  }
  scheduleRefresh();
}

function start() {
  off = window.bridge?.onModuleEvent(onEvent) ?? null;
  void refreshRoutines();
}

function stop() {
  off?.();
  off = null;
  if (timer) clearTimeout(timer);
  timer = null;
  generation += 1;
}

export function dismissAttention(key: string): void {
  dismissed = new Set([...dismissed, key]);
  try {
    localStorage.setItem(
      DISMISSED_KEY,
      JSON.stringify([...dismissed].slice(-DISMISSED_MAX)),
    );
  } catch {
    // Not remembered across a restart; the row is gone for this session.
  }
  publish({});
}

/** The shared snapshot. Nothing is read or listened to while `enabled` is
 * false (the extension is off). */
export function useRoutinesStore(enabled: boolean): RoutinesSnapshot {
  useEffect(() => {
    if (!enabled) return;
    readers += 1;
    if (readers === 1) start();
    return () => {
      readers -= 1;
      if (readers === 0) stop();
    };
  }, [enabled]);
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => void listeners.delete(listener);
    },
    () => snapshot,
  );
}
