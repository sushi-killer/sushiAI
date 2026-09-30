import { useEffect, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Plus, Trash2, X } from "lucide-react";
import "./orchestrator-settings.css";
import { OrchestratorHostProvider, useOrchestratorClient } from "./hostContext";
import { LOCAL_HOST, routeProblem } from "./hosts";
import type { Preflight } from "./types";
import { useOrchestratorHosts } from "./useHosts";
import {
  errorText,
  landRepos,
  withLandRepos,
  type DefaultableSetting,
  resetSettingToDefault,
  settingValue,
  settingsDifferingFromDefaults,
} from "./helpers";
import { Stepper } from "./ui";
import { ConnectedTools } from "./ConnectedTools";
import {
  applyPreset,
  countChanges,
  presetOf,
  type AutonomyPreset,
} from "./autonomy";
import type { ChatModels, ModelProfile } from "../types";
import type { Harness, Route, Settings, Tier, Variant } from "./types";

const HARNESSES: Harness[] = ["claude", "codex"];

const SETTING_LABELS: Record<DefaultableSetting, string> = {
  "tiers.mechanical": "Mechanical tier",
  "tiers.standard": "Standard tier",
  "tiers.hard": "Hard tier",
  review: "Review",
  planner: "Planner",
  orchestrator: "Orchestrator",
  autoAnswer: "Answer stuck questions",
  sandbox: "Sandbox",
  codexNetwork: "Codex network access",
  allowedDomains: "Allowed network domains",
  protectedPaths: "Protected paths",
  maxAttempts: "Max attempts",
  parallel: "Parallel tasks",
  childParallel: "Parallel subtasks per task",
};

/** The route id a route-valued setting points at, or `null` when its value
 * is one of the non-route choices ("" or "auto") or it isn't a route field. */
function routeIdOf(field: DefaultableSetting, value: unknown): string | null {
  const isRouteField =
    field.startsWith("tiers.") ||
    field === "review" ||
    field === "planner" ||
    field === "orchestrator";
  if (!isRouteField || typeof value !== "string") return null;
  return value && value !== "auto" ? value : null;
}

/** How a default reads in its marker: a route's label rather than its id,
 * and the same words the control's own options use. */
function defaultText(
  field: DefaultableSetting,
  value: unknown,
  routes: Route[],
): string {
  const routeId = routeIdOf(field, value);
  if (routeId)
    return routes.find((route) => route.id === routeId)?.label || routeId;
  if (field === "review")
    return value === "auto" ? "Auto (never weaker)" : "Off";
  if (field === "planner") return "Off";
  if (field === "orchestrator") return "Standard route";
  if (typeof value === "boolean") return value ? "On" : "Off";
  if (Array.isArray(value)) return value.length ? value.join(", ") : "None";
  return String(value) || "None";
}

function newRoute(harness: Harness): Route {
  return {
    id: `route-${Date.now().toString(36)}`,
    label: harness === "claude" ? "Claude" : "Codex",
    harness,
  };
}

/** Mirrors the engine's built-in strength for a route's model. */
function defaultStrength(model?: string): number {
  const m = (model || "").toLowerCase();
  if (/haiku|luna|mini/.test(m)) return 1;
  return m.includes("opus") ? 3 : 2;
}

function Toggle({
  on,
  label,
  onChange,
}: {
  on: boolean;
  label: string;
  onChange(next: boolean): void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      className={`os-toggle${on ? " on" : ""}`}
      onClick={() => onChange(!on)}
    >
      <span className="os-knob" />
    </button>
  );
}

function Select({
  value,
  label,
  onChange,
  children,
  className = "",
}: {
  value: string;
  label: string;
  onChange(value: string): void;
  children: ReactNode;
  className?: string;
}) {
  return (
    <span className={`os-select ${className}`}>
      <select
        value={value}
        aria-label={label}
        onChange={(event) => onChange(event.target.value)}
      >
        {children}
      </select>
      <ChevronDown size={12} aria-hidden />
    </span>
  );
}

function ChipList({
  values,
  label,
  placeholder,
  onChange,
}: {
  values: string[];
  label: string;
  placeholder: string;
  onChange(next: string[]): void;
}) {
  const [text, setText] = useState("");
  function commit() {
    const items = text
      .split(/[,\n]/)
      .map((item) => item.trim())
      .filter((item) => item && !values.includes(item));
    if (items.length) onChange([...values, ...items]);
    setText("");
  }
  return (
    <div className="os-tokens">
      {values.map((value) => (
        <span key={value} className="os-token">
          {value}
          <button
            type="button"
            aria-label={`Remove ${value}`}
            onClick={() => onChange(values.filter((item) => item !== value))}
          >
            <X size={11} />
          </button>
        </span>
      ))}
      <input
        value={text}
        aria-label={label}
        placeholder={placeholder}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === ",") {
            event.preventDefault();
            commit();
          }
        }}
        onBlur={commit}
      />
    </div>
  );
}

/** A dollar cap: empty means none. Keeps its own text so "0." survives typing. */
function DollarInput({
  value,
  label,
  onChange,
}: {
  value: number | undefined;
  label: string;
  onChange(next: number | undefined): void;
}) {
  const [text, setText] = useState(value ? String(value) : "");
  useEffect(() => {
    setText((old) =>
      Number(old) === (value || 0) ? old : value ? String(value) : "",
    );
  }, [value]);
  return (
    <label className="os-input">
      <span>$</span>
      <input
        inputMode="decimal"
        value={text}
        aria-label={label}
        placeholder="No cap"
        onChange={(event) => {
          const raw = event.target.value;
          if (!/^\d*\.?\d*$/.test(raw)) return;
          setText(raw);
          const parsed = Number(raw);
          onChange(parsed > 0 ? parsed : undefined);
        }}
      />
    </label>
  );
}

function RouteRow({
  route,
  chatModels,
  profiles,
  problem,
  onChange,
  onDelete,
}: {
  route: Route;
  /** Why the host being edited cannot run this route, if it cannot. */
  problem?: string | null;
  chatModels: ChatModels;
  profiles: ModelProfile[];
  onChange(next: Route): void;
  onDelete(): void;
}) {
  const models = chatModels[route.harness]?.models || [];
  const efforts = chatModels[route.harness]?.efforts || [];
  return (
    <div
      className={`os-route-row ${problem ? "unavailable" : ""}`}
      title={problem ?? undefined}
    >
      <input
        className="os-route-label"
        value={route.label}
        onChange={(event) => onChange({ ...route, label: event.target.value })}
        aria-label="Route label"
        placeholder="Label"
      />
      <Select
        label="Harness"
        value={route.harness}
        onChange={(harness) =>
          onChange({
            ...route,
            harness: harness as Harness,
            model: undefined,
            profileId: undefined,
          })
        }
      >
        {HARNESSES.map((harness) => (
          <option key={harness} value={harness}>
            {harness}
          </option>
        ))}
      </Select>
      <Select
        label="Model"
        value={route.model || ""}
        onChange={(model) => onChange({ ...route, model: model || undefined })}
      >
        <option value="">Default model</option>
        {models.map((model) => (
          <option key={model.id} value={model.id}>
            {model.label}
          </option>
        ))}
      </Select>
      <Select
        label="Effort"
        value={route.effort || ""}
        onChange={(effort) =>
          onChange({ ...route, effort: effort || undefined })
        }
      >
        <option value="">Default</option>
        {efforts.map((effort) => (
          <option key={effort} value={effort}>
            {effort}
          </option>
        ))}
      </Select>
      {route.harness === "claude" ? (
        <Select
          label="Model profile"
          value={route.profileId || ""}
          onChange={(profileId) =>
            onChange({ ...route, profileId: profileId || undefined })
          }
        >
          <option value="">Anthropic</option>
          {profiles.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {profile.label}
            </option>
          ))}
        </Select>
      ) : (
        <span />
      )}
      <button
        className="icon-button os-route-delete"
        title={`Remove route ${route.label}`}
        aria-label={`Remove route ${route.label}`}
        onClick={onDelete}
        type="button"
      >
        <Trash2 size={14} />
      </button>
    </div>
  );
}

function SectionHead({ title, note }: { title: string; note: string }) {
  return (
    <div className="os-head">
      <p className="os-eyebrow">{title}</p>
      <p className="os-note">{note}</p>
    </div>
  );
}

const PRESET_CARDS: {
  id: "ask" | "balanced" | "handsOff";
  title: string;
  text: string;
}[] = [
  {
    id: "ask",
    title: "Ask me",
    text: "Every question comes to you. Nothing lands without your click.",
  },
  {
    id: "balanced",
    title: "Balanced",
    text: "Confident answers are taken for you and shown. Finished work lands on its branch, never on main.",
  },
  {
    id: "handsOff",
    title: "Hands-off",
    text: "Also retries failures with a note and lands follow-ups. You see a daily digest.",
  },
];

const TIER_CARDS: { tier: Tier; title: string; hint: string }[] = [
  { tier: "mechanical", title: "Mechanical", hint: "renames, config, copy" },
  { tier: "standard", title: "Standard", hint: "most tasks" },
  { tier: "hard", title: "Hard", hint: "architecture, tricky bugs" },
];

function OrchestratorSettingsBody({
  preflight,
}: {
  preflight: Preflight | null;
}) {
  const orchestratorClient = useOrchestratorClient();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saved, setSaved] = useState<Settings | null>(null);
  const [profiles, setProfiles] = useState<ModelProfile[]>([]);
  const [chatModels, setChatModels] = useState<ChatModels>({});
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [notBuilt, setNotBuilt] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  // orchd's built-in defaults; stays null on an older orchd without
  // `settings.defaults`, which just means no default markers.
  const [defaults, setDefaults] = useState<Settings | null>(null);

  useEffect(() => {
    Promise.all([
      orchestratorClient.settingsGet(),
      window.bridge?.modelProfilesList() ?? Promise.resolve([]),
      window.bridge?.chatModels() ?? Promise.resolve({}),
    ])
      .then(([s, m, c]) => {
        setSettings(s);
        setSaved(s);
        setProfiles(m);
        setChatModels(c);
      })
      .catch((e) => {
        const message = errorText(e);
        setError(message);
        setNotBuilt(message.includes("is not built"));
      });
    orchestratorClient
      .settingsDefaults()
      .then(setDefaults)
      .catch(() => {
        // An older orchd binary: the panel works exactly as before.
      });
  }, [orchestratorClient]);

  if (notBuilt)
    return (
      <p className="text-muted">
        The orchestrator isn't built yet. Run <code>npm run build:orchd</code>{" "}
        and reopen Settings.
      </p>
    );
  if (!settings || !saved)
    return error ? (
      <p className="inline-error" role="alert">
        {error}
      </p>
    ) : (
      <div className="loading">Loading orchestration settings…</div>
    );

  function update(patch: Partial<Settings>) {
    setSettings((old) => (old ? { ...old, ...patch } : old));
  }
  function updateExperiments(patch: Partial<Settings["experiments"]>) {
    setSettings((old) =>
      old ? { ...old, experiments: { ...old.experiments, ...patch } } : old,
    );
  }

  const differing = defaults
    ? settingsDifferingFromDefaults(settings, defaults)
    : [];
  const changes = countChanges(saved, settings);
  const preset: AutonomyPreset = presetOf(settings);

  function resetToDefault(field: DefaultableSetting) {
    if (!defaults) return;
    setSettings((old) =>
      old ? resetSettingToDefault(old, defaults, field) : old,
    );
  }

  /** The marker for a setting outside `DefaultableSetting`: `pick` reads
   * it, `put` copies the default back. Nothing while it matches orchd's
   * default. */
  function valueMarker(
    label: string,
    pick: (from: Settings) => unknown,
    put: (to: Settings, from: Settings) => Settings,
    text: (value: unknown) => string,
  ) {
    if (!defaults || !settings) return null;
    const value = pick(defaults);
    if (JSON.stringify(pick(settings)) === JSON.stringify(value)) return null;
    return (
      <button
        type="button"
        className="os-diff"
        aria-label={`Reset ${label} to default`}
        title={`Reset ${label} to default`}
        onClick={() =>
          setSettings((old) => (old && defaults ? put(old, defaults) : old))
        }
      >
        changed · default {text(value)} ↺
      </button>
    );
  }
  /** `valueMarker` for one experiment flag. */
  function experimentMarker(
    label: string,
    key: keyof Variant,
    text: (value: unknown) => string = (value) =>
      typeof value === "boolean" ? (value ? "On" : "Off") : String(value),
  ) {
    return valueMarker(
      label,
      (from) => from.experiments[key],
      (to, from) => ({
        ...to,
        experiments: { ...to.experiments, [key]: from.experiments[key] },
      }),
      text,
    );
  }
  const dollars = (value: unknown) =>
    typeof value === "number" && value > 0 ? `$${value}` : "No cap";

  /** "changed · default Off ↺" beside a setting saved away from orchd's
   * default. Reset only stages the value; Save still writes it. */
  function defaultMarker(field: DefaultableSetting) {
    if (!defaults || !settings || !differing.includes(field)) return null;
    const label = SETTING_LABELS[field];
    const value = settingValue(defaults, field);
    // The built-in route's label first: a same-id route the owner renamed
    // should not relabel orchd's default.
    const text = defaultText(field, value, [
      ...defaults.routes,
      ...settings.routes,
    ]);
    // Resetting to a route the owner has since removed would leave a
    // dangling id - show the default, but don't offer to restore it.
    const routeId = routeIdOf(field, value);
    const missing =
      routeId !== null &&
      !settings.routes.some((route) => route.id === routeId);
    return (
      <button
        type="button"
        className="os-diff"
        aria-label={`Reset ${label} to default`}
        title={
          missing
            ? `Route ${text} is not configured`
            : `Reset ${label} to default`
        }
        disabled={missing}
        onClick={() => resetToDefault(field)}
      >
        changed · default {text} ↺
      </button>
    );
  }

  /** One settings row; `marker` is its "changed · default" button. */
  function row(
    title: string,
    description: string,
    control: ReactNode,
    marker?: ReactNode,
  ) {
    return (
      <div className="os-row">
        <div className="os-row-text">
          <div className="os-row-head">
            <span className="os-row-title">{title}</span>
            {marker}
          </div>
          <span className="os-row-desc">{description}</span>
        </div>
        {control}
      </div>
    );
  }

  async function save() {
    if (!settings) return;
    setSaving(true);
    setError("");
    try {
      const result = await orchestratorClient.settingsSet(settings);
      setSettings(result);
      setSaved(result);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  }

  function routeOptions(extra: ReactNode) {
    return (
      <>
        {extra}
        {settings?.routes.map((route) => (
          <option key={route.id} value={route.id}>
            {route.label}
          </option>
        ))}
      </>
    );
  }

  function tierCard(
    key: string,
    title: string,
    hint: string | undefined,
    select: ReactNode,
    marker: ReactNode,
    warning?: ReactNode,
  ) {
    return (
      <div key={key} className="os-tier">
        <div className="os-tier-head">
          <span className="os-tier-title">{title}</span>
          {hint && <span className="os-tier-hint">{hint}</span>}
        </div>
        {select}
        {warning}
        {marker}
      </div>
    );
  }

  return (
    <div className="orchestrator-settings">
      <SectionHead
        title="AUTONOMY"
        note="One choice sets how often the orchestrator stops to ask you. Fine-tune below."
      />
      <div className="os-presets" role="radiogroup" aria-label="Autonomy">
        {PRESET_CARDS.map((card) => {
          const disabled = card.id === "handsOff";
          const selected = preset === card.id;
          return (
            <button
              key={card.id}
              type="button"
              role="radio"
              aria-checked={selected}
              disabled={disabled}
              className={`os-preset${selected ? " selected" : ""}`}
              onClick={() => {
                if (card.id !== "handsOff")
                  setSettings((old) =>
                    old ? applyPreset(old, card.id as "ask" | "balanced") : old,
                  );
              }}
            >
              <span className="os-preset-title">
                <i className="os-radio" />
                {card.title}
              </span>
              <span className="os-preset-text">{card.text}</span>
              {disabled && (
                <span className="os-preset-soon">
                  Coming: needs orchestrator support that isn't built yet.
                </span>
              )}
            </button>
          );
        })}
      </div>
      {preset === "custom" && (
        <p className="os-custom">
          Custom: the rows below don't match a preset. Pick a card to reset
          them.
        </p>
      )}
      {row(
        "Answer stuck questions for me",
        "When the orchestrator is confident it answers itself and tells you what it chose. You can overturn it.",
        <Toggle
          on={settings.autoAnswer}
          label="Answer stuck questions for me"
          onChange={(autoAnswer) => update({ autoAnswer })}
        />,
        defaultMarker("autoAnswer"),
      )}
      {row(
        "Land finished work",
        "Merge a passed task into its base branch. Never lands on the default branch.",
        <Toggle
          on={settings.experiments.land === true}
          label="Land finished work"
          onChange={(land) => updateExperiments({ land })}
        />,
        experimentMarker("Land finished work", "land"),
      )}
      <div className="os-block">
        <div className="os-row-head">
          <span className="os-row-title">
            Repos where landing on the default branch is allowed
          </span>
        </div>
        <span className="os-row-desc">
          Every other repo refuses to land on its default branch. Use the full
          repo path.
        </span>
        <ChipList
          values={landRepos(settings.landOnDefaultRepos)}
          label="Add a repo path"
          placeholder="Add a repo path…"
          onChange={(repos) =>
            update({
              landOnDefaultRepos: withLandRepos(
                settings.landOnDefaultRepos,
                repos,
              ),
            })
          }
        />
      </div>
      {row(
        "Max attempts",
        "How many times a task is retried before it comes to you.",
        <Stepper
          label="Max attempts"
          min={1}
          value={settings.maxAttempts}
          onChange={(maxAttempts) => update({ maxAttempts })}
        />,
        defaultMarker("maxAttempts"),
      )}

      <SectionHead
        title="MODELS & ROUTES"
        note="A route is a harness plus a model. Tiers pick a route by how hard the task looks."
      />
      <div className="os-routes">
        <div className="os-route-row os-route-header">
          <span>Route</span>
          <span>Harness</span>
          <span>Model</span>
          <span>Effort</span>
          <span>Profile</span>
          <span />
        </div>
        {settings.routes.map((route) => (
          <RouteRow
            key={route.id}
            route={route}
            problem={routeProblem(preflight, route)}
            chatModels={chatModels}
            profiles={profiles}
            onChange={(next) =>
              update({
                routes: settings.routes.map((r) =>
                  r.id === route.id ? next : r,
                ),
              })
            }
            onDelete={() =>
              update({
                routes: settings.routes.filter((r) => r.id !== route.id),
                // None of the three should keep pointing at a route that no
                // longer exists - fall back to "off" rather than a dangling id.
                review: settings.review === route.id ? "" : settings.review,
                planner: settings.planner === route.id ? "" : settings.planner,
                orchestrator:
                  settings.orchestrator === route.id
                    ? ""
                    : settings.orchestrator,
              })
            }
          />
        ))}
        <button
          type="button"
          className="os-add"
          onClick={() =>
            update({ routes: [...settings.routes, newRoute("claude")] })
          }
        >
          <Plus size={13} /> Add route
        </button>
      </div>
      <div className="os-tiers">
        {TIER_CARDS.map(({ tier, title, hint }) =>
          tierCard(
            tier,
            title,
            hint,
            <Select
              label={`${title} tier route`}
              value={settings.tiers[tier] || ""}
              onChange={(route) =>
                update({ tiers: { ...settings.tiers, [tier]: route } })
              }
            >
              {routeOptions(null)}
            </Select>,
            defaultMarker(`tiers.${tier}`),
          ),
        )}
        {tierCard(
          "review",
          "Review",
          "a second opinion from another vendor",
          <Select
            label="Review route"
            value={settings.review}
            onChange={(review) => update({ review })}
          >
            {routeOptions(
              <>
                <option value="">Off</option>
                <option value="auto">Auto (never weaker)</option>
              </>,
            )}
          </Select>,
          defaultMarker("review"),
        )}
        {tierCard(
          "planner",
          "Planner",
          undefined,
          <Select
            label="Planner route"
            value={settings.planner}
            onChange={(planner) => update({ planner })}
          >
            {routeOptions(<option value="">Off</option>)}
          </Select>,
          defaultMarker("planner"),
          settings.planner && settings.planner !== settings.tiers.hard ? (
            <span className="os-warn">Differs from the hard tier's route</span>
          ) : null,
        )}
        {tierCard(
          "orchestrator",
          "Orchestrator chat",
          undefined,
          <Select
            label="Orchestrator chat route"
            value={settings.orchestrator}
            onChange={(orchestrator) => update({ orchestrator })}
          >
            {routeOptions(<option value="">Standard route</option>)}
          </Select>,
          defaultMarker("orchestrator"),
        )}
      </div>

      <SectionHead
        title="CONNECTED TOOLS"
        note="Services the orchestrator chat can look things up in. A change waits for your OK in the chat."
      />
      <ConnectedTools
        tools={settings.chatTools ?? []}
        savedTools={saved.chatTools ?? []}
        onChange={(chatTools) => update({ chatTools })}
      />

      <SectionHead
        title="SAFETY & LIMITS"
        note="Where agents run, what they may reach, and how much runs at once."
      />
      {row(
        "Sandbox",
        "native isolates each agent in a macOS sandbox; host runs it with your user rights.",
        <Select
          label="Sandbox"
          className="os-select-fixed"
          value={settings.sandbox}
          onChange={(sandbox) =>
            update({ sandbox: sandbox as Settings["sandbox"] })
          }
        >
          <option value="native">native</option>
          <option value="host">host</option>
        </Select>,
        defaultMarker("sandbox"),
      )}
      {row(
        "Codex network access",
        "Let Codex tasks reach the network. Claude tasks follow the allowed domains below.",
        <Toggle
          on={settings.codexNetwork}
          label="Codex network access"
          onChange={(codexNetwork) => update({ codexNetwork })}
        />,
        defaultMarker("codexNetwork"),
      )}
      <div className="os-block">
        <div className="os-row-head">
          <span className="os-row-title">Allowed network domains</span>
          {defaultMarker("allowedDomains")}
        </div>
        <span className="os-row-desc">
          Everything else is blocked inside the sandbox.
        </span>
        <ChipList
          values={settings.allowedDomains}
          label="Add a domain"
          placeholder="Add a domain…"
          onChange={(allowedDomains) => update({ allowedDomains })}
        />
      </div>
      <div className="os-block">
        <div className="os-row-head">
          <span className="os-row-title">Protected paths</span>
          {defaultMarker("protectedPaths")}
        </div>
        <span className="os-row-desc">
          Agents may read these but never change them.
        </span>
        <ChipList
          values={settings.protectedPaths}
          label="Add a glob"
          placeholder="Add a glob…"
          onChange={(protectedPaths) => update({ protectedPaths })}
        />
      </div>
      <div className="os-limits">
        <div className="os-limit">
          <span>Parallel tasks</span>
          <Stepper
            label="Parallel tasks"
            min={1}
            value={settings.parallel}
            onChange={(parallel) => update({ parallel })}
          />
          {defaultMarker("parallel")}
        </div>
        <div className="os-limit">
          <span>Subtasks per task</span>
          <Stepper
            label="Subtasks per task"
            min={1}
            value={settings.childParallel}
            onChange={(childParallel) => update({ childParallel })}
          />
          {defaultMarker("childParallel")}
        </div>
      </div>

      <SectionHead
        title="BUDGET"
        note="A task that would cross a cap stops and comes to you instead."
      />
      <div className="os-budget">
        <div className="os-limit">
          <span>Per task</span>
          <DollarInput
            label="Budget per task"
            value={settings.experiments.maxCostUsd}
            onChange={(maxCostUsd) => updateExperiments({ maxCostUsd })}
          />
          {experimentMarker("Budget per task", "maxCostUsd", dollars)}
        </div>
        <div className="os-limit">
          <span>Per attempt</span>
          <DollarInput
            label="Budget per attempt"
            value={settings.experiments.maxAttemptCostUsd}
            onChange={(maxAttemptCostUsd) =>
              updateExperiments({ maxAttemptCostUsd })
            }
          />
          {experimentMarker("Budget per attempt", "maxAttemptCostUsd", dollars)}
        </div>
        <div className="os-limit">
          <span>Per day</span>
          <DollarInput
            label="Budget per day"
            value={settings.dailyBudgetUsd}
            onChange={(next) => update({ dailyBudgetUsd: next ?? 0 })}
          />
          {valueMarker(
            "Budget per day",
            (from) => from.dailyBudgetUsd,
            (to, from) => ({ ...to, dailyBudgetUsd: from.dailyBudgetUsd }),
            dollars,
          )}
        </div>
      </div>

      <button
        type="button"
        className="os-advanced"
        aria-expanded={advancedOpen}
        onClick={() => setAdvancedOpen(!advancedOpen)}
      >
        <ChevronRight size={13} className={advancedOpen ? "open" : ""} />
        Advanced — experiments and route strength
      </button>
      {advancedOpen && (
        <div className="os-advanced-body">
          {row(
            "Review with evidence",
            "The reviewer gets the screenshots the attempt saved and longer verify output.",
            <Toggle
              on={settings.experiments.reviewEvidence}
              label="Review with evidence"
              onChange={(reviewEvidence) =>
                updateExperiments({ reviewEvidence })
              }
            />,
            experimentMarker("Review with evidence", "reviewEvidence"),
          )}
          {row(
            "Advisor after a failed attempt",
            "One read-only call on the planner's route diagnoses the failure for the next attempt.",
            <Toggle
              on={settings.experiments.advisor}
              label="Advisor after a failed attempt"
              onChange={(advisor) => updateExperiments({ advisor })}
            />,
            experimentMarker("Advisor after a failed attempt", "advisor"),
          )}
          {row(
            "Loop detection",
            "Stop an attempt that repeats itself.",
            <Toggle
              on={settings.experiments.loopDetect}
              label="Loop detection"
              onChange={(loopDetect) => updateExperiments({ loopDetect })}
            />,
            experimentMarker("Loop detection", "loopDetect"),
          )}
          {row(
            "Stall timeout (seconds)",
            "Stop an attempt that prints nothing for this long. 0 turns it off.",
            <label className="os-input os-input-narrow">
              <input
                type="number"
                min={0}
                aria-label="Stall timeout in seconds"
                value={settings.experiments.stallTimeoutSecs}
                onChange={(event) =>
                  updateExperiments({
                    stallTimeoutSecs: Math.max(
                      0,
                      Number(event.target.value) || 0,
                    ),
                  })
                }
              />
            </label>,
            experimentMarker("Stall timeout", "stallTimeoutSecs"),
          )}
          {settings.routes.map((route) =>
            row(
              `Strength of ${route.label}`,
              "1 to 3. Review never picks a route weaker than the implementer's.",
              <label className="os-input os-input-narrow">
                <input
                  type="number"
                  min={1}
                  max={3}
                  aria-label={`Strength of ${route.label}`}
                  value={route.strength ?? ""}
                  placeholder={String(defaultStrength(route.model))}
                  onChange={(event) =>
                    update({
                      routes: settings.routes.map((r) =>
                        r.id === route.id
                          ? {
                              ...r,
                              strength: event.target.value
                                ? Number(event.target.value)
                                : undefined,
                            }
                          : r,
                      ),
                    })
                  }
                />
              </label>,
            ),
          )}
        </div>
      )}

      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      {changes > 0 && (
        <div className="os-savebar" role="region" aria-label="Unsaved changes">
          <i className="os-savebar-dot" />
          <span className="os-savebar-count">
            {changes} unsaved {changes === 1 ? "change" : "changes"}
          </span>
          <span className="os-spacer" />
          <button
            type="button"
            className="os-ghost"
            disabled={saving}
            onClick={() => setSettings(saved)}
          >
            Discard
          </button>
          <button
            type="button"
            className="os-primary"
            disabled={saving}
            onClick={save}
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      )}
    </div>
  );
}

/** The orchestration settings of one host's daemon: Local, or a remote host
 * whose routes are marked unavailable when its git/claude/codex says so. */
export function OrchestratorSettings() {
  const { hosts } = useOrchestratorHosts();
  const [host, setHost] = useState(LOCAL_HOST);
  const info = hosts.find((item) => item.id === host);
  return (
    <OrchestratorHostProvider value={host}>
      {hosts.length > 1 && (
        <label className="orch-settings-host">
          <span>Host</span>
          <select
            aria-label="Settings host"
            value={host}
            onChange={(event) => setHost(event.target.value)}
          >
            {hosts.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
              </option>
            ))}
          </select>
        </label>
      )}
      <OrchestratorSettingsBody
        key={host}
        preflight={info?.preflight ?? null}
      />
    </OrchestratorHostProvider>
  );
}
