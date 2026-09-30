import type { Settings } from "./types";

/** The whole settings object as the Advanced editor shows it. */
export function formatSettingsJson(settings: Settings): string {
  return JSON.stringify(settings, null, 2);
}

export type ParsedSettings =
  { ok: true; settings: Settings } | { ok: false; error: string };

/** Parses the Advanced editor's text into a settings object. Only the shape
 * the panel itself dereferences is checked (`routes`, `tiers`, `experiments`);
 * every other key, present or future, passes through for orchd to validate. */
export function parseSettingsJson(text: string): ParsedSettings {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : "Invalid JSON.",
    };
  }
  if (!value || typeof value !== "object" || Array.isArray(value))
    return { ok: false, error: "Settings must be a JSON object." };
  const record = value as Record<string, unknown>;
  if (!Array.isArray(record.routes))
    return { ok: false, error: '"routes" must be a list.' };
  for (const key of ["tiers", "experiments"]) {
    const part = record[key];
    if (!part || typeof part !== "object" || Array.isArray(part))
      return { ok: false, error: `"${key}" must be an object.` };
  }
  return { ok: true, settings: value as Settings };
}
