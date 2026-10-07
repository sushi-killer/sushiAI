import type { CompanionResult, CompanionValues } from "./types.ts";

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
    async run(actionId: string) {
      if (state.busy) return;
      set({ busy: actionId, message: undefined, error: undefined });
      try {
        const result = await bridge.companionAction(
          extensionId,
          surfaceId,
          actionId,
        );
        // The action's answer is newer than any read already in flight.
        latestRead += 1;
        set({
          busy: null,
          values: { ...state.values, ...result.values },
          message: result.message,
        });
      } catch (error) {
        set({ busy: null, error: failure(error) });
      }
    },
    stop() {
      stopped = true;
      unsubscribe?.();
    },
  };
}
