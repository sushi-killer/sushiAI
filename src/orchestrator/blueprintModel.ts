// The pipeline blueprint's model: `pipeline.describe` data in, diagram nodes
// and settings edits out. Pure - no React - so tests import it directly.
import { formatCost, formatDuration } from "./helpers.ts";
import type {
  Pipeline,
  Preset,
  PresetValues,
  Route,
  Settings,
  Task,
  Tier,
} from "./types";

export type NodeState =
  "current" | "done" | "failed" | "skipped" | "disabled" | "pending";

export type BlueprintNode = {
  id: string;
  label: string;
  /** `branch` is the open question's node, drawn off the stage that asked. */
  kind: "main" | "side" | "branch";
  state: NodeState;
  enabled: boolean;
  /** The owner may switch the stage off. */
  canToggle: boolean;
  ownerOnly: boolean;
  /** The stage's route can be changed from the inspector. */
  routeEditable: boolean;
  routeId?: string;
  routeLabel?: string;
  routeModel?: string;
  autoRouteLabel?: string;
  autoModel?: string;
  maxAttempts?: number;
  model?: string;
  time?: string;
  cost?: string;
  /** Branch only: the stage that asked. */
  from?: string;
  fromLabel?: string;
};

/** Stages whose route the inspector may change. */
const ROUTE_EDITABLE = new Set([
  "plan",
  "brief-check",
  "implement",
  "review",
  "judge",
]);

export function formatRouteLabel(
  label: string | undefined,
  model: string | undefined,
  automatic = false,
): string {
  const route = label
    ? model && model !== label
      ? `${label} (${model})`
      : label
    : model;
  return automatic ? `Auto: ${route || "route"}` : route || "";
}

const STATE_OF: Record<string, NodeState> = {
  running: "current",
  done: "done",
  failed: "failed",
  skipped: "skipped",
};

export function blueprintNodes(
  payload: Pipeline,
  routes: Route[],
): BlueprintNode[] {
  const labels = new Map(payload.stages.map((s) => [s.id, s.label]));
  const nodes = payload.stages.map((stage): BlueprintNode => {
    const route =
      stage.id === "judge" && !stage.routeLabel
        ? undefined
        : routes.find((r) => r.id === stage.routeId);
    const routeLabel = stage.routeLabel ?? route?.label;
    const routeModel =
      stage.routeLabel !== undefined
        ? stage.model
        : (stage.model ?? route?.model);
    const model =
      formatRouteLabel(routeLabel, routeModel, stage.routeId === "auto") ||
      undefined;
    return {
      id: stage.id,
      label: stage.label || stage.id,
      kind: stage.kind === "side" ? "side" : "main",
      state: !stage.enabled
        ? "disabled"
        : ((stage.state && STATE_OF[stage.state]) ?? "pending"),
      enabled: stage.enabled,
      canToggle: stage.optional && !stage.ownerOnly,
      ownerOnly: stage.ownerOnly,
      routeEditable: ROUTE_EDITABLE.has(stage.id),
      routeId: stage.routeId,
      routeLabel,
      routeModel,
      autoRouteLabel: stage.autoRouteLabel,
      autoModel: stage.autoModel,
      maxAttempts: stage.id === "implement" ? stage.maxAttempts : undefined,
      model,
      time:
        stage.durationMs === undefined
          ? undefined
          : formatDuration(stage.durationMs),
      cost: stage.costUsd === undefined ? undefined : formatCost(stage.costUsd),
    };
  });
  if (payload.branch) {
    const { from, to } = payload.branch;
    nodes.push({
      id: "branch",
      label: to === "overseer" ? "Overseer" : "You",
      kind: "branch",
      state: "current",
      enabled: true,
      canToggle: false,
      ownerOnly: false,
      routeEditable: false,
      from,
      fromLabel: labels.get(from) ?? from,
    });
  }
  return nodes;
}

/** A change the inspector makes to one stage. */
export type StageChange =
  | { kind: "enabled"; value: boolean; route?: string }
  | { kind: "route"; value: string; tier?: Tier }
  | { kind: "maxAttempts"; value: number };

/** The values `change` sets on a preset, laid over `values`. */
function presetValuesWith(
  values: PresetValues,
  settings: Settings,
  stageId: string,
  change: StageChange,
): PresetValues {
  const next: PresetValues = { ...values };
  switch (stageId) {
    case "advisor":
      if (change.kind === "enabled") next.advisor = change.value;
      break;
    case "held-out":
      if (change.kind === "enabled") next.heldOut = change.value;
      break;
    case "brief-check":
      if (change.kind === "enabled") {
        next.briefCheck = change.value;
        if (change.value)
          next.briefCheckRoute =
            change.route ||
            values.briefCheckRoute ||
            settings.briefCheckRoute ||
            settings.planner;
        else delete next.briefCheckRoute;
      } else if (change.kind === "route") next.briefCheckRoute = change.value;
      break;
    case "plan":
      if (change.kind === "route") next.planner = change.value;
      break;
    case "review":
      if (change.kind === "route") next.review = change.value;
      break;
    case "judge":
      if (change.kind === "route") next.judge = change.value;
      break;
    case "implement":
      if (change.kind === "route")
        next.tierRoutes = {
          ...values.tierRoutes,
          [change.tier ?? "standard"]: change.value,
        };
      else if (change.kind === "maxAttempts") next.maxAttempts = change.value;
      break;
  }
  return next;
}

function globalsWith(
  settings: Settings,
  stageId: string,
  change: StageChange,
): Settings {
  const experiments = settings.experiments;
  switch (stageId) {
    case "advisor":
      if (change.kind === "enabled")
        return {
          ...settings,
          experiments: { ...experiments, advisor: change.value },
        };
      break;
    case "held-out":
      if (change.kind === "enabled")
        return {
          ...settings,
          experiments: { ...experiments, heldOut: change.value },
        };
      break;
    case "land":
      if (change.kind === "enabled")
        return {
          ...settings,
          experiments: { ...experiments, land: change.value },
        };
      break;
    case "brief-check":
      if (change.kind === "enabled")
        return {
          ...settings,
          briefCheckRoute: change.value ? change.route || settings.planner : "",
        };
      if (change.kind === "route")
        return { ...settings, briefCheckRoute: change.value };
      break;
    case "plan":
      if (change.kind === "route")
        return { ...settings, planner: change.value };
      break;
    case "review":
      if (change.kind === "route") return { ...settings, review: change.value };
      break;
    case "judge":
      if (change.kind === "route")
        return {
          ...settings,
          experiments: { ...experiments, judgeRoute: change.value },
        };
      break;
    case "implement":
      if (change.kind === "route")
        return {
          ...settings,
          tiers: {
            ...settings.tiers,
            [change.tier ?? "standard"]: change.value,
          },
        };
      if (change.kind === "maxAttempts")
        return { ...settings, maxAttempts: change.value };
      break;
  }
  return settings;
}

function uniquePresetId(base: string, taken: Set<string>): string {
  let id = `${base}-custom`;
  for (let n = 2; taken.has(id); n += 1) id = `${base}-custom-${n}`;
  return id;
}

/** The settings after one stage edit. With no preset the global fields
 * change; with a custom preset only that preset's values do; a built-in
 * preset is never edited - a custom copy with the edit is added and becomes
 * the repo's preset. `known` is the describe payload's preset list (the
 * built-ins' values). Land has no preset field, so a preset leaves it
 * alone. */
export function stageEdit(
  settings: Settings,
  repo: string,
  presetId: string | null,
  stageId: string,
  change: StageChange,
  known: Preset[] = [],
): Settings {
  if (presetId === null || presetId === "")
    return globalsWith(settings, stageId, change);
  if (stageId === "land") return settings;
  const custom = (settings.presets ?? []).find((p) => p.id === presetId);
  if (custom) {
    const values = presetValuesWith(custom.values, settings, stageId, change);
    return {
      ...settings,
      presets: (settings.presets ?? []).map((p) =>
        p.id === presetId ? { ...p, values } : p,
      ),
    };
  }
  const base = known.find((p) => p.id === presetId);
  const label = base?.label ?? presetId;
  const values = presetValuesWith(
    base?.values ?? {},
    settings,
    stageId,
    change,
  );
  const presets = settings.presets ?? [];
  const taken = new Set([
    ...presets.map((p) => p.id),
    ...known.map((p) => p.id),
  ]);
  const id = uniquePresetId(presetId, taken);
  return {
    ...settings,
    presets: [
      ...presets,
      { id, label: `${label} (custom)`, builtin: false, values },
    ],
    repoPresets: { ...settings.repoPresets, [repo]: id },
  };
}

/** A task's pipeline can still be chosen until it has an implement attempt. */
export function canPickTaskPreset(task: Task): boolean {
  return !task.attempts.some((a) => a.stage === "implement");
}
