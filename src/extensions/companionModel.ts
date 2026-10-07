import type {
  CompanionResult,
  CompanionRow,
  CompanionValue,
  CompanionValues,
  Tone,
} from "./types.ts";

const ROW_TONES: readonly Tone[] = [
  "neutral",
  "info",
  "warning",
  "danger",
  "muted",
  "ok",
];
export const MAX_ROWS = 50;

/** The drawable rows of a `list` value: at most 50, a row without a string id
 * and label is dropped (not the whole list), an unknown tone reads as neutral.
 * The main process already cleans a reply; this keeps the renderer safe on
 * its own. */
export function listRows(value: CompanionValue | undefined): CompanionRow[] {
  if (!Array.isArray(value)) return [];
  const rows: CompanionRow[] = [];
  const seen = new Set<string>();
  const text = (item: unknown) =>
    typeof item === "string" && item ? item : undefined;
  for (const row of value as unknown[]) {
    if (rows.length >= MAX_ROWS) break;
    const item = row as Record<string, unknown> | null;
    const id = text(item?.id);
    const label = text(item?.label);
    if (!id || !label || seen.has(id)) continue;
    seen.add(id);
    rows.push({
      id,
      label,
      detail: text(item?.detail),
      status: text(item?.status),
      action: text(item?.action),
      tone: ROW_TONES.includes(item?.tone as Tone)
        ? (item?.tone as Tone)
        : "neutral",
    });
  }
  return rows;
}

/** The busy marker of a list row's button. */
export const rowBusyKey = (fieldId: string, rowId: string) =>
  `row:${fieldId}:${rowId}`;

/** The slice of the bridge a companion view talks to. */
export type CompanionBridge = {
  companionRead(
    extensionId: string,
    surfaceId: string,
  ): Promise<CompanionResult>;
  companionAction(
    extensionId: string,
    surfaceId: string,
    actionId: string,
  ): Promise<CompanionResult>;
  companionRow(
    extensionId: string,
    surfaceId: string,
    fieldId: string,
    rowId: string,
  ): Promise<CompanionResult>;
  onCompanionChanged(
    callback: (change: { extensionId: string; surfaceId: string }) => void,
  ): () => void;
};

export type CompanionViewState = {
  values: CompanionValues;
  /** True once the first read answered. */
  loaded: boolean;
  /** The id of the action in flight; no other action starts meanwhile. */
  busy: string | null;
  message?: string;
  error?: string;
};

export const INITIAL_COMPANION_STATE: CompanionViewState = {
  values: {},
  loaded: false,
  busy: null,
};

const failure = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

/** Keeps one companion view's values: reads on start and on every
 * `view.changed`, and runs a button's action with a busy flag around it. A
 * read that started before a newer one never overwrites it. */
export function createCompanionController(
  bridge: CompanionBridge,
  extensionId: string,
  surfaceId: string,
  onState: (state: CompanionViewState) => void,
) {
  let state = INITIAL_COMPANION_STATE;
  let stopped = false;
  let latestRead = 0;
  let unsubscribe: (() => void) | undefined;
  const set = (patch: Partial<CompanionViewState>) => {
    if (stopped) return;
    state = { ...state, ...patch };
    onState(state);
  };

  async function read() {
    const mine = ++latestRead;
    try {
      const result = await bridge.companionRead(extensionId, surfaceId);
      if (mine === latestRead)
        set({ values: result.values ?? {}, loaded: true, error: undefined });
    } catch (error) {
      if (mine === latestRead) set({ loaded: true, error: failure(error) });
    }
  }

  async function call(busy: string, request: () => Promise<CompanionResult>) {
    if (state.busy) return;
    set({ busy, message: undefined, error: undefined });
    try {
      const result = await request();
      // The answer is newer than any read already in flight.
      latestRead += 1;
      set({
        busy: null,
        values: { ...state.values, ...result.values },
        message: result.message,
      });
    } catch (error) {
      set({ busy: null, error: failure(error) });
    }
  }

  return {
    start() {
      stopped = false;
      unsubscribe = bridge.onCompanionChanged((change) => {
        if (
          change.extensionId === extensionId &&
          change.surfaceId === surfaceId
        )
          void read();
      });
      void read();
    },
    run: (actionId: string) =>
      call(actionId, () =>
        bridge.companionAction(extensionId, surfaceId, actionId),
      ),
    /** A list row's button. */
    runRow: (fieldId: string, rowId: string) =>
      call(rowBusyKey(fieldId, rowId), () =>
        bridge.companionRow(extensionId, surfaceId, fieldId, rowId),
      ),
    stop() {
      stopped = true;
      unsubscribe?.();
    },
  };
}
