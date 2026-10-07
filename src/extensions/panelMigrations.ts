import type { Panel } from "../types.ts";
import { migrateLegacyOrchestratorPanel } from "../orchestrator/panelMigration.ts";

/** Turns a panel of a kind core no longer knows into its current shape, or
 * returns undefined when the panel is not this module's. Kept apart from
 * modules.ts because workspaceState is loaded in node, where the module UI
 * composition root (React) is not. Like modules.ts, a composition root: the
 * only place core names a built-in module's migration. */
export type PanelMigration = (panel: Panel) => Panel | undefined;

export const panelMigrations: readonly PanelMigration[] = Object.freeze([
  migrateLegacyOrchestratorPanel,
]);
