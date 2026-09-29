import { useEffect, useState } from "react";
import { Plus, RotateCcw, Trash2 } from "lucide-react";
import "./orchestrator.css";
import { orchestratorClient } from "./client";
import {
  type DefaultableSetting,
  resetSettingToDefault,
  settingValue,
  settingsDifferingFromDefaults,
} from "./helpers";
import type { ChatModels, ModelProfile, ModelProvider } from "../types";
import type {
  ClassifierBackend,
  Harness,
  Route,
  Settings,
  Tier,
} from "./types";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const TIERS: Tier[] = ["mechanical", "standard", "hard"];
const HARNESSES: Harness[] = ["claude", "codex"];
const CLASSIFIER_BACKENDS: { value: ClassifierBackend; label: string }[] = [
  { value: "none", label: "Off (rules only)" },
  { value: "openrouter", label: "OpenRouter" },
  { value: "typesafe", label: "TypeSafe" },
  { value: "openai", label: "OpenAI-compatible (Ollama / LM Studio)" },
];

const SETTING_LABELS: Record<DefaultableSetting, string> = {
  "tiers.mechanical": "Mechanical tier",
  "tiers.standard": "Standard tier",
  "tiers.hard": "Hard tier",
  review: "Review",
  planner: "Planner",
  orchestrator: "Orchestrator",
  autoAnswer: "Answer stuck questions",
  "classifier.backend": "Classifier backend",
  "classifier.model": "Classifier model",
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
  if (field === "classifier.backend")
    return (
      CLASSIFIER_BACKENDS.find((backend) => backend.value === value)?.label ||
      String(value)
    );
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

function linesToList(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

/** Mirrors the engine's built-in strength for a route's model. */
function defaultStrength(model?: string): number {
  const m = (model || "").toLowerCase();
  if (/haiku|luna|mini/.test(m)) return 1;
  return m.includes("opus") ? 3 : 2;
}

function RouteRow({
  route,
  chatModels,
  profiles,
  onChange,
  onDelete,
}: {
  route: Route;
  chatModels: ChatModels;
  profiles: ModelProfile[];
  onChange(next: Route): void;
  onDelete(): void;
}) {
  const models = chatModels[route.harness]?.models || [];
  const efforts = chatModels[route.harness]?.efforts || [];
  return (
    <div className="orch-route-row">
      <input
        className="orch-route-label"
        value={route.label}
        onChange={(event) => onChange({ ...route, label: event.target.value })}
        aria-label="Route label"
        placeholder="Label"
      />
      <select
        className="orch-route-harness"
        value={route.harness}
        onChange={(event) =>
          onChange({
            ...route,
            harness: event.target.value as Harness,
            model: undefined,
            profileId: undefined,
          })
        }
        aria-label="Harness"
      >
        {HARNESSES.map((harness) => (
          <option key={harness} value={harness}>
            {harness}
          </option>
        ))}
      </select>
      <select
        className="orch-route-model"
        value={route.model || ""}
        onChange={(event) =>
          onChange({ ...route, model: event.target.value || undefined })
        }
        aria-label="Model"
      >
        <option value="">Default model</option>
        {models.map((model) => (
          <option key={model.id} value={model.id}>
            {model.label}
          </option>
        ))}
      </select>
      <select
        className="orch-route-effort"
        value={route.effort || ""}
        onChange={(event) =>
          onChange({ ...route, effort: event.target.value || undefined })
        }
        aria-label="Effort"
      >
        <option value="">Default effort</option>
        {efforts.map((effort) => (
          <option key={effort} value={effort}>
            {effort}
          </option>
        ))}
      </select>
      {route.harness === "claude" && (
        <select
          className="orch-route-profile"
          value={route.profileId || ""}
          onChange={(event) =>
            onChange({ ...route, profileId: event.target.value || undefined })
          }
          aria-label="Model profile"
        >
          <option value="">Anthropic (no profile)</option>
          {profiles.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {profile.label}
            </option>
          ))}
        </select>
      )}
      <div className="orch-route-tail">
        <input
          className="orch-route-strength"
          type="number"
          min={1}
          max={3}
          value={route.strength ?? ""}
          placeholder={String(defaultStrength(route.model))}
          onChange={(event) =>
            onChange({
              ...route,
              strength: event.target.value
                ? Number(event.target.value)
                : undefined,
            })
          }
          aria-label="Strength"
        />
        <button
          className="icon-button orch-route-delete"
          title={`Remove route ${route.label}`}
          onClick={onDelete}
          type="button"
        >
          <Trash2 size={13} />
        </button>
      </div>
    </div>
  );
}

export function OrchestratorSettings() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [providers, setProviders] = useState<ModelProvider[]>([]);
  const [profiles, setProfiles] = useState<ModelProfile[]>([]);
  const [chatModels, setChatModels] = useState<ChatModels>({});
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [notBuilt, setNotBuilt] = useState(false);
  // Raw textarea text, decoupled from `settings.allowedDomains`/
  // `protectedPaths`: parsing into a list on every keystroke rebuilds the
  // controlled value from the filtered array immediately after, which drops
  // a trailing newline before Enter can ever start a new line. Parsed back
  // into `settings` on blur and again at save time.
  const [domainsText, setDomainsText] = useState("");
  const [pathsText, setPathsText] = useState("");
  // orchd's built-in defaults; stays null on an older orchd without
  // `settings.defaults`, which just means no default markers.
  const [defaults, setDefaults] = useState<Settings | null>(null);

  useEffect(() => {
    Promise.all([
      orchestratorClient.settingsGet(),
      window.bridge?.providersList() ?? Promise.resolve([]),
      window.bridge?.modelProfilesList() ?? Promise.resolve([]),
      window.bridge?.chatModels() ?? Promise.resolve({}),
    ])
      .then(([s, p, m, c]) => {
        setSettings(s);
        setDomainsText(s.allowedDomains.join("\n"));
        setPathsText(s.protectedPaths.join("\n"));
        setProviders(p);
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
  }, []);

  if (notBuilt)
    return (
      <p className="text-muted">
        The orchestrator isn't built yet. Run <code>npm run build:orchd</code>{" "}
        and reopen Settings.
      </p>
    );
  if (!settings)
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

  const differing = defaults
    ? settingsDifferingFromDefaults(settings, defaults)
    : [];

  function resetToDefault(field: DefaultableSetting) {
    if (!defaults) return;
    setSettings((old) =>
      old ? resetSettingToDefault(old, defaults, field) : old,
    );
    if (field === "allowedDomains")
      setDomainsText(defaults.allowedDomains.join("\n"));
    if (field === "protectedPaths")
      setPathsText(defaults.protectedPaths.join("\n"));
  }

  /** The marker and reset button beside a control whose saved value differs
   * from orchd's default. Reset only stages the value; Save still writes it,
   * like every other edit here. */
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
      <span className="orch-default-marker">
        {missing
          ? `Differs from the default (${text}).`
          : `Differs from the default (${text}). Reset restores it.`}
        <button
          type="button"
          className="icon-button"
          aria-label={`Reset ${label} to default`}
          title={
            missing
              ? `Route ${text} is not configured`
              : `Reset ${label} to default`
          }
          disabled={missing}
          onClick={() => resetToDefault(field)}
        >
          <RotateCcw size={12} />
        </button>
      </span>
    );
  }

  async function save() {
    if (!settings) return;
    setSaving(true);
    setError("");
    try {
      // Parsed here too, not just on blur: a click straight from a focused
      // textarea to Save must not lose whatever hasn't blurred yet.
      const toSave: Settings = {
        ...settings,
        allowedDomains: linesToList(domainsText),
        protectedPaths: linesToList(pathsText),
      };
      const saved = await orchestratorClient.settingsSet(toSave);
      setSettings(saved);
      setDomainsText(saved.allowedDomains.join("\n"));
      setPathsText(saved.protectedPaths.join("\n"));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="orchestrator-settings">
      <div className="setting-block">
        <p className="dialog-eyebrow">ROUTES</p>
        {settings.routes.map((route) => (
          <RouteRow
            key={route.id}
            route={route}
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
          onClick={() =>
            update({ routes: [...settings.routes, newRoute("claude")] })
          }
        >
          <Plus size={13} /> Add route
        </button>
      </div>

      <div className="setting-block">
        <p className="dialog-eyebrow">TIER → ROUTE</p>
        <div className="orch-tier-row">
          {TIERS.map((tier) => (
            <div key={tier} className="orch-tier-label">
              <label>
                <span>{tier}</span>
                <select
                  value={settings.tiers[tier] || ""}
                  onChange={(event) =>
                    update({
                      tiers: { ...settings.tiers, [tier]: event.target.value },
                    })
                  }
                >
                  {settings.routes.map((route) => (
                    <option key={route.id} value={route.id}>
                      {route.label}
                    </option>
                  ))}
                </select>
              </label>
              {defaultMarker(`tiers.${tier}`)}
            </div>
          ))}
        </div>
        <div className="orch-review-planner-row">
          <div className="orch-tier-label">
            <label>
              <span>Review</span>
              <select
                value={settings.review}
                onChange={(event) => update({ review: event.target.value })}
              >
                <option value="">Off</option>
                <option value="auto">Auto (never weaker)</option>
                {settings.routes.map((route) => (
                  <option key={route.id} value={route.id}>
                    {route.label}
                  </option>
                ))}
              </select>
            </label>
            {defaultMarker("review")}
          </div>
          <div className="orch-tier-label">
            <label>
              <span>Planner</span>
              <select
                value={settings.planner}
                onChange={(event) => update({ planner: event.target.value })}
              >
                <option value="">Off</option>
                {settings.routes.map((route) => (
                  <option key={route.id} value={route.id}>
                    {route.label}
                  </option>
                ))}
              </select>
              {settings.planner && settings.planner !== settings.tiers.hard && (
                <span className="tone-yellow">
                  Differs from the hard tier's route
                </span>
              )}
            </label>
            {defaultMarker("planner")}
          </div>
          <div className="orch-tier-label">
            <label>
              <span>Orchestrator</span>
              <select
                value={settings.orchestrator}
                onChange={(event) =>
                  update({ orchestrator: event.target.value })
                }
              >
                <option value="">Standard route</option>
                {settings.routes.map((route) => (
                  <option key={route.id} value={route.id}>
                    {route.label}
                  </option>
                ))}
              </select>
            </label>
            {defaultMarker("orchestrator")}
          </div>
        </div>
        <label className="setting-check">
          <input
            type="checkbox"
            checked={settings.autoAnswer}
            onChange={(event) => update({ autoAnswer: event.target.checked })}
          />
          <span>
            Answer stuck questions for me
            <em>
              The orchestrator answers an agent's question when the repository
              or your earlier decisions settle it, and asks you otherwise. It
              never approves protected paths, stops a task or adds attempts.
            </em>
          </span>
        </label>
        {defaultMarker("autoAnswer")}
      </div>

      <div className="setting-block">
        <p className="dialog-eyebrow">CLASSIFIER</p>
        <label className="form-row">
          Backend
          <select
            value={settings.classifier.backend}
            onChange={(event) =>
              update({
                classifier: {
                  ...settings.classifier,
                  backend: event.target.value as ClassifierBackend,
                },
              })
            }
          >
            {CLASSIFIER_BACKENDS.map((backend) => (
              <option key={backend.value} value={backend.value}>
                {backend.label}
              </option>
            ))}
          </select>
        </label>
        {defaultMarker("classifier.backend")}
        {settings.classifier.backend !== "none" && (
          <>
            <label className="form-row">
              Model
              <input
                value={settings.classifier.model}
                onChange={(event) =>
                  update({
                    classifier: {
                      ...settings.classifier,
                      model: event.target.value,
                    },
                  })
                }
                placeholder="typesafe/jev-1.13"
              />
            </label>
            {defaultMarker("classifier.model")}
            <label className="form-row">
              Provider (API key)
              <select
                value={settings.classifier.providerId}
                onChange={(event) =>
                  update({
                    classifier: {
                      ...settings.classifier,
                      providerId: event.target.value,
                    },
                  })
                }
              >
                <option value="">Choose a provider…</option>
                {providers.map((provider) => (
                  <option key={provider.id} value={provider.id}>
                    {provider.label} {provider.hasKey ? "✓" : "(no key)"}
                  </option>
                ))}
              </select>
            </label>
            {settings.classifier.backend === "openai" && (
              <p className="text-muted">
                Uses the selected provider's own base URL - there is no separate
                endpoint field.
              </p>
            )}
          </>
        )}
      </div>

      <div className="setting-block">
        <p className="dialog-eyebrow">SANDBOX AND LIMITS</p>
        <div
          className="workspace-control-tabs"
          role="radiogroup"
          aria-label="Sandbox"
        >
          {(["native", "host"] as const).map((value) => (
            <button
              key={value}
              role="radio"
              aria-checked={settings.sandbox === value}
              className={settings.sandbox === value ? "selected" : ""}
              onClick={() => update({ sandbox: value })}
            >
              {value}
            </button>
          ))}
        </div>
        {defaultMarker("sandbox")}
        <label className="setting-check">
          <input
            type="checkbox"
            checked={settings.codexNetwork}
            onChange={(event) => update({ codexNetwork: event.target.checked })}
          />
          <span>
            Codex network access
            <em>
              All or nothing - Codex has no per-domain filter, unlike Claude's
              sandbox below.
            </em>
          </span>
        </label>
        {defaultMarker("codexNetwork")}
        <label>
          Allowed network domains (Claude's sandbox only, one per line)
          <textarea
            value={domainsText}
            onChange={(event) => setDomainsText(event.target.value)}
            onBlur={() => update({ allowedDomains: linesToList(domainsText) })}
            rows={3}
          />
        </label>
        {defaultMarker("allowedDomains")}
        <label>
          Protected paths (globs, one per line)
          <textarea
            value={pathsText}
            onChange={(event) => setPathsText(event.target.value)}
            onBlur={() => update({ protectedPaths: linesToList(pathsText) })}
            rows={2}
            placeholder="src/app/**"
          />
        </label>
        {defaultMarker("protectedPaths")}
        <div className="form-row">
          <div className="orch-default-field">
            <label>
              Max attempts
              <input
                type="number"
                min={1}
                value={settings.maxAttempts}
                onChange={(event) =>
                  update({ maxAttempts: Number(event.target.value) || 1 })
                }
              />
            </label>
            {defaultMarker("maxAttempts")}
          </div>
          <div className="orch-default-field">
            <label>
              Parallel tasks
              <input
                type="number"
                min={1}
                value={settings.parallel}
                onChange={(event) =>
                  update({ parallel: Number(event.target.value) || 1 })
                }
              />
            </label>
            {defaultMarker("parallel")}
          </div>
          <div className="orch-default-field">
            <label>
              Parallel subtasks per task
              <input
                type="number"
                min={1}
                value={settings.childParallel}
                onChange={(event) =>
                  update({ childParallel: Number(event.target.value) || 1 })
                }
              />
            </label>
            {defaultMarker("childParallel")}
          </div>
        </div>
      </div>

      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      <div className="form-row">
        <button className="primary" disabled={saving} onClick={save}>
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}
