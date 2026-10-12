import { useState } from "react";
import { ArrowRight, CornerDownRight } from "lucide-react";
import type { Pipeline, Route, Tier } from "./types";
import {
  blueprintNodes,
  formatRouteLabel,
  type BlueprintNode,
  type NodeState,
  type StageChange,
} from "./blueprintModel";
import { Tag, Toggle } from "../ui";
import type { Tone } from "../ui/tone.ts";
import "./blueprint.css";

const STATE_LABEL: Record<NodeState, string> = {
  current: "running",
  done: "done",
  failed: "failed",
  skipped: "skipped",
  disabled: "off",
  pending: "pending",
};

const STATE_TONE: Record<NodeState, Tone> = {
  current: "info",
  done: "ok",
  failed: "danger",
  skipped: "neutral",
  disabled: "neutral",
  pending: "neutral",
};
const TIERS: Tier[] = ["mechanical", "standard", "hard"];

/** The preset picker's options: the value is a preset id, "" = none. */
export type PresetPicker = {
  options: { id: string; label: string }[];
  value: string;
  disabled?: boolean;
  /** Why the picker is disabled, or what it does. */
  note?: string;
  onChange(id: string): void;
};

function NodeFacts({ node }: { node: BlueprintNode }) {
  if (node.kind === "branch")
    return <span className="bp-fact">asked at {node.fromLabel}</span>;
  return (
    <>
      {node.model && <span className="bp-fact bp-model">{node.model}</span>}
      {(node.time || node.cost) && (
        <span className="bp-fact">
          {[node.time, node.cost].filter(Boolean).join(" · ")}
        </span>
      )}
    </>
  );
}

function Inspector({
  node,
  routes,
  tierRoutes,
  landLocked,
  onEdit,
}: {
  node: BlueprintNode;
  routes: Route[];
  tierRoutes: Record<Tier, string>;
  /** A preset is selected and presets carry no land switch. */
  landLocked: boolean;
  /** Absent = read-only (a task's pipeline). */
  onEdit?: (stageId: string, change: StageChange) => void;
}) {
  const editable = onEdit !== undefined;
  const routeId = node.routeId ?? "";
  const known = routes.some((r) => r.id === routeId);
  const route =
    node.id === "judge" && !node.routeLabel
      ? undefined
      : routes.find((r) => r.id === routeId);
  const routeName =
    formatRouteLabel(
      node.routeLabel ?? route?.label,
      node.routeLabel !== undefined
        ? node.routeModel
        : (node.routeModel ?? route?.model),
      node.routeId === "auto",
    ) ||
    node.model ||
    routeId;
  const toggleLocked = node.id === "land" && landLocked;
  return (
    <div
      className="bp-inspector"
      role="group"
      aria-label={`${node.label} stage`}
    >
      <div className="bp-inspector-head">
        <strong>{node.label}</strong>
        <Tag tone={STATE_TONE[node.state]}>{STATE_LABEL[node.state]}</Tag>
      </div>
      {node.ownerOnly && <p className="bp-note">shared checks: owner only</p>}
      {node.canToggle && (
        <label className="bp-row-field">
          <span>Enabled</span>
          <Toggle
            checked={node.enabled}
            label={`${node.label} enabled`}
            disabled={!editable || toggleLocked}
            onChange={(value) => onEdit?.(node.id, { kind: "enabled", value })}
          />
        </label>
      )}
      {toggleLocked && (
        <p className="bp-note">
          Land follows the global setting; presets do not change it.
        </p>
      )}
      {node.routeEditable &&
        editable &&
        (node.id === "implement" ? (
          TIERS.map((tier) => {
            const value = tierRoutes[tier];
            return (
              <label className="bp-row-field" key={tier}>
                <span>{tier[0].toUpperCase() + tier.slice(1)}</span>
                <select
                  className="bp-select"
                  aria-label={`${tier[0].toUpperCase() + tier.slice(1)} route`}
                  value={value}
                  onChange={(event) =>
                    onEdit?.(node.id, {
                      kind: "route",
                      value: event.target.value,
                      tier,
                    })
                  }
                >
                  {!routes.some((item) => item.id === value) && (
                    <option value={value}>{value || "none"}</option>
                  )}
                  {routes.map((item) => (
                    <option key={item.id} value={item.id}>
                      {formatRouteLabel(item.label, item.model)}
                    </option>
                  ))}
                </select>
              </label>
            );
          })
        ) : (
          <label className="bp-row-field">
            <span>Route</span>
            <select
              className="bp-select"
              aria-label={`${node.label} route`}
              value={routeId}
              onChange={(event) =>
                onEdit?.(node.id, { kind: "route", value: event.target.value })
              }
            >
              {!known && routeId !== "auto" && (
                <option value={routeId}>{routeId || "none"}</option>
              )}
              {["brief-check", "review", "judge"].includes(node.id) && (
                <option value="auto">
                  {formatRouteLabel(
                    node.autoRouteLabel ??
                      (routeId === "auto" ? node.routeLabel : undefined),
                    node.autoModel ??
                      (routeId === "auto" ? node.routeModel : undefined),
                    true,
                  )}
                </option>
              )}
              {routes.map((item) => (
                <option key={item.id} value={item.id}>
                  {formatRouteLabel(item.label, item.model)}
                </option>
              ))}
            </select>
          </label>
        ))}
      {(!node.routeEditable || !editable) && routeName && (
        <p className="bp-note">
          Route: {routeName}
          {node.id === "advisor" && " (the plan route)"}
        </p>
      )}
      {node.maxAttempts !== undefined &&
        (editable ? (
          <label className="bp-row-field">
            <span>Max attempts</span>
            <input
              className="bp-number"
              type="number"
              min={1}
              aria-label="Max attempts"
              defaultValue={node.maxAttempts}
              key={node.maxAttempts}
              onBlur={(event) => {
                const value = Math.floor(Number(event.target.value));
                if (value >= 1 && value !== node.maxAttempts)
                  onEdit?.(node.id, { kind: "maxAttempts", value });
              }}
            />
          </label>
        ) : (
          <p className="bp-note">Max attempts: {node.maxAttempts}</p>
        ))}
    </div>
  );
}

/** The orchd pipeline as a diagram: main stages left to right in a wrapping
 * row, side stages and the open question as a branch under it. Built only
 * from the `pipeline.describe` payload; a click opens a small inspector. */
export function Blueprint({
  pipeline,
  routes,
  tierRoutes,
  preset,
  onEdit,
  error,
}: {
  pipeline: Pipeline | null;
  routes: Route[];
  tierRoutes: Record<Tier, string>;
  preset: PresetPicker;
  onEdit?: (stageId: string, change: StageChange) => void;
  error?: string;
}) {
  const [openIds, setOpenIds] = useState<string[]>([]);
  const nodes = pipeline ? blueprintNodes(pipeline, routes) : [];
  const main = nodes.filter((n) => n.kind === "main");
  const side = nodes.filter((n) => n.kind !== "main");
  const open = nodes.filter(
    (n) => openIds.includes(n.id) && n.kind !== "branch",
  );

  const draw = (node: BlueprintNode) => (
    <button
      key={node.id}
      type="button"
      className={`bp-node ${node.state}${openIds.includes(node.id) ? " open" : ""}${node.kind === "branch" ? " branch" : ""}`}
      aria-pressed={openIds.includes(node.id)}
      disabled={node.kind === "branch"}
      onClick={() =>
        setOpenIds((ids) =>
          ids.includes(node.id)
            ? ids.filter((id) => id !== node.id)
            : [...ids, node.id],
        )
      }
    >
      <span className="bp-node-label">{node.label}</span>
      <NodeFacts node={node} />
    </button>
  );

  return (
    <section className="bp" aria-label="Pipeline">
      <div className="bp-head">
        <label className="bp-preset">
          <span className="bp-eyebrow">PRESET</span>
          <select
            className="bp-select"
            aria-label="Preset"
            value={preset.value}
            disabled={preset.disabled}
            onChange={(event) => preset.onChange(event.target.value)}
          >
            {preset.options.map((option) => (
              <option key={option.id} value={option.id}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        {preset.note && <span className="bp-note">{preset.note}</span>}
      </div>
      {error && <p className="bp-error">{error}</p>}
      {!pipeline ? (
        !error && <p className="bp-note">Loading the pipeline…</p>
      ) : (
        <>
          <div className="bp-flow" role="list" aria-label="Stages">
            {main.map((node, index) => (
              <div key={node.id} className="bp-cell" role="listitem">
                {index > 0 && (
                  <ArrowRight size={12} className="bp-arrow" aria-hidden />
                )}
                {draw(node)}
              </div>
            ))}
          </div>
          {side.length > 0 && (
            <div className="bp-branch" role="list" aria-label="Side stages">
              <CornerDownRight size={14} className="bp-arrow" aria-hidden />
              {side.map((node) => (
                <div key={node.id} className="bp-cell" role="listitem">
                  {draw(node)}
                </div>
              ))}
            </div>
          )}
          {open.map((node) => (
            <Inspector
              key={node.id}
              node={node}
              routes={routes}
              tierRoutes={tierRoutes}
              landLocked={preset.value !== ""}
              onEdit={onEdit}
            />
          ))}
        </>
      )}
    </section>
  );
}
