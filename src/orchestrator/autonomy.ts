import type { Settings } from "./types";

/** One choice for how often the orchestrator stops to ask the owner. Each is
 * a fixed combination of three daemon settings; `custom` is any other
 * combination (the rows below the cards can be set one by one). */
export type AutonomyPreset = "ask" | "balanced" | "handsOff" | "custom";

export type AutonomyFlags = {
  autoAnswer: boolean;
  answerPolicy: boolean;
  land: boolean;
};

/** `handsOff` also retries failures and lands follow-ups, which the daemon
 * cannot do yet, so it has no mapping and is never applied or detected. */
export const AUTONOMY_FLAGS: Record<"ask" | "balanced", AutonomyFlags> = {
  ask: { autoAnswer: false, answerPolicy: false, land: false },
  balanced: { autoAnswer: true, answerPolicy: true, land: true },
};

export function autonomyOf(settings: Settings): AutonomyFlags {
  return {
    autoAnswer: settings.autoAnswer,
    answerPolicy: settings.answerPolicy,
    land: settings.experiments?.land === true,
  };
}

export function presetOf(settings: Settings): AutonomyPreset {
  const flags = autonomyOf(settings);
  for (const preset of ["ask", "balanced"] as const) {
    const want = AUTONOMY_FLAGS[preset];
    if (
      want.autoAnswer === flags.autoAnswer &&
      want.answerPolicy === flags.answerPolicy &&
      want.land === flags.land
    )
      return preset;
  }
  return "custom";
}

export function applyPreset(
  settings: Settings,
  preset: "ask" | "balanced",
): Settings {
  const flags = AUTONOMY_FLAGS[preset];
  return {
    ...settings,
    autoAnswer: flags.autoAnswer,
    answerPolicy: flags.answerPolicy,
    experiments: { ...settings.experiments, land: flags.land },
  };
}

/** Number of top-level fields that differ between two settings objects, for
 * the "n unsaved changes" bar. Compares by JSON, per settings field, and
 * folds the `experiments` object into one change per differing key. */
export function countChanges(saved: Settings, draft: Settings): number {
  const same = (a: unknown, b: unknown) =>
    JSON.stringify(a) === JSON.stringify(b);
  let count = 0;
  const keys = new Set([...Object.keys(saved), ...Object.keys(draft)]);
  for (const key of keys) {
    const a = (saved as Record<string, unknown>)[key];
    const b = (draft as Record<string, unknown>)[key];
    if (same(a, b)) continue;
    if (key === "experiments" || key === "tiers" || key === "classifier") {
      const ao = (a ?? {}) as Record<string, unknown>;
      const bo = (b ?? {}) as Record<string, unknown>;
      for (const sub of new Set([...Object.keys(ao), ...Object.keys(bo)]))
        if (!same(ao[sub], bo[sub])) count++;
    } else count++;
  }
  return count;
}
