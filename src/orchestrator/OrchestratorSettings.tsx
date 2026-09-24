import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import "./orchestrator.css";
import { orchestratorClient } from "./client";
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
      <button
        className="icon-button orch-route-delete"
        title={`Remove route ${route.label}`}
        onClick={onDelete}
        type="button"
      >
        <Trash2 size={13} />
      </button>
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
            <label key={tier} className="orch-tier-label">
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
          ))}
        </div>
        <div className="orch-review-planner-row">
          <label className="orch-tier-label">
            <span>Review</span>
            <select
              value={settings.review}
              onChange={(event) => update({ review: event.target.value })}
            >
              <option value="">Off</option>
              <option value="auto">Other vendor (auto)</option>
              {settings.routes.map((route) => (
                <option key={route.id} value={route.id}>
                  {route.label}
                </option>
              ))}
            </select>
          </label>
          <label className="orch-tier-label">
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
          <label className="orch-tier-label">
            <span>Orchestrator</span>
            <select
              value={settings.orchestrator}
              onChange={(event) => update({ orchestrator: event.target.value })}
            >
              <option value="">Standard route</option>
              {settings.routes.map((route) => (
                <option key={route.id} value={route.id}>
                  {route.label}
                </option>
              ))}
            </select>
          </label>
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
        <label>
          Allowed network domains (Claude's sandbox only, one per line)
          <textarea
            value={domainsText}
            onChange={(event) => setDomainsText(event.target.value)}
            onBlur={() => update({ allowedDomains: linesToList(domainsText) })}
            rows={3}
          />
        </label>
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
        <div className="form-row">
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
