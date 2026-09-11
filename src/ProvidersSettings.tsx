import { useEffect, useState } from "react";
import { Check, Globe, Pencil, Plus, Server, Trash2, X } from "lucide-react";
import type {
  ModelProfile,
  ModelProvider,
  ModelProviderKind,
  ProviderModel,
} from "./types";

const KIND_LABEL: Record<ModelProviderKind, string> = {
  openrouter: "OpenRouter",
  "opencode-go": "OpenCode Go",
  custom: "Custom",
};
const EFFORTS = ["", "low", "medium", "high", "xhigh", "max"];
const CONTEXT_PRESETS = [
  { value: 0, label: "Model default" },
  { value: 128000, label: "128K" },
  { value: 200000, label: "200K" },
  { value: 256000, label: "256K" },
  { value: 512000, label: "512K" },
  { value: 1000000, label: "1M+" },
];

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

/** Compact context-window label for the profile table. */
function contextLabel(tokens?: number) {
  if (!tokens) return "default";
  if (tokens >= 1_000_000) return `${tokens / 1_000_000}M`;
  return `${Math.round(tokens / 1000)}K`;
}

/** Add/edit form for one model profile under a provider. */
function ProfileForm({
  provider,
  profile,
  profiles,
  onDone,
}: {
  provider: ModelProvider;
  profile?: ModelProfile;
  /** All profiles, so a duplicate model under the same provider is caught. */
  profiles: ModelProfile[];
  onDone(): void;
}) {
  const [models, setModels] = useState<ProviderModel[]>([]);
  const [modelId, setModelId] = useState(profile?.modelId || "");
  const [label, setLabel] = useState(profile?.label || "");
  const [effort, setEffort] = useState(profile?.effort || "");
  const [context, setContext] = useState(
    CONTEXT_PRESETS.some((c) => c.value === profile?.contextWindow)
      ? profile?.contextWindow || 0
      : profile?.contextWindow
        ? -1
        : 0,
  );
  const [customContext, setCustomContext] = useState(
    String(profile?.contextWindow || ""),
  );
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => {
    window.bridge?.providersModels(provider.id).then(setModels);
  }, [provider.id]);
  return (
    <form
      className="model-profile-form"
      onSubmit={async (event) => {
        event.preventDefault();
        if (
          profiles.some(
            (p) =>
              p.providerId === provider.id &&
              p.modelId === modelId &&
              p.id !== profile?.id,
          )
        ) {
          setError("This profile already exists for the provider.");
          return;
        }
        setSaving(true);
        setError("");
        try {
          await window.bridge!.modelProfilesUpsert({
            id: profile?.id,
            providerId: provider.id,
            modelId,
            label: label || modelId,
            effort: effort || undefined,
            contextWindow:
              context === -1
                ? Number(customContext) || undefined
                : context || undefined,
          });
          onDone();
        } catch (e) {
          setError(errorText(e));
        } finally {
          setSaving(false);
        }
      }}
    >
      <label>
        Model
        {models.length ? (
          <select
            value={modelId}
            onChange={(event) => setModelId(event.target.value)}
            required
          >
            <option value="">Choose a model…</option>
            {/* A saved profile can name a model the catalog no longer lists;
                keep it selectable so editing never shows a blank field. */}
            {modelId && !models.some((m) => m.id === modelId) && (
              <option value={modelId}>{modelId}</option>
            )}
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label}
              </option>
            ))}
          </select>
        ) : (
          <input
            value={modelId}
            onChange={(event) => setModelId(event.target.value)}
            placeholder="e.g. anthropic/claude-sonnet-4.5"
            required
          />
        )}
      </label>
      <div className="form-row">
        <label>
          Display name
          <input
            value={label}
            onChange={(event) => setLabel(event.target.value)}
            placeholder={modelId || "Shown on the agent panel"}
          />
        </label>
        <label>
          Reasoning effort
          <select
            value={effort}
            onChange={(event) => setEffort(event.target.value)}
          >
            {EFFORTS.map((e) => (
              <option key={e} value={e}>
                {e || "Default"}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label>
        Context window
        <select
          value={context}
          onChange={(event) => setContext(Number(event.target.value))}
        >
          {CONTEXT_PRESETS.map((c) => (
            <option key={c.value} value={c.value}>
              {c.label}
            </option>
          ))}
          <option value={-1}>Custom…</option>
        </select>
      </label>
      {context === -1 && (
        <input
          type="number"
          min="1"
          value={customContext}
          onChange={(event) => setCustomContext(event.target.value)}
          placeholder="Tokens, e.g. 300000"
          required
        />
      )}
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      <div className="form-row">
        <button className="primary" disabled={saving} type="submit">
          {saving ? "Saving…" : "Save model"}
        </button>
        <button type="button" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}

export function ProvidersSettings() {
  const [providers, setProviders] = useState<ModelProvider[]>([]);
  const [profiles, setProfiles] = useState<ModelProfile[]>([]);
  const [addingKind, setAddingKind] = useState<ModelProviderKind | "">("");
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [status, setStatus] = useState<Record<string, string>>({});
  const [editingProfile, setEditingProfile] = useState<{
    providerId: string;
    profile?: ModelProfile;
  } | null>(null);

  const refresh = () =>
    Promise.all([
      window.bridge?.providersList(),
      window.bridge?.modelProfilesList(),
    ])
      .then(([p, m]) => {
        setProviders(p || []);
        setProfiles(m || []);
      })
      .catch((e) => setError(errorText(e)));
  useEffect(() => {
    refresh();
  }, []);

  const addProviderForm = addingKind && (
    <form
      className="ssh-form"
      onSubmit={async (event) => {
        event.preventDefault();
        const form = new FormData(event.currentTarget);
        setError("");
        if (
          addingKind !== "custom" &&
          providers.some((p) => p.kind === addingKind)
        ) {
          setError(`${KIND_LABEL[addingKind]} is already added.`);
          return;
        }
        try {
          await window.bridge!.providersUpsert({
            kind: addingKind as ModelProviderKind,
            label: String(form.get("label") || ""),
            baseUrl: String(form.get("baseUrl") || ""),
          });
          setAddingKind("");
          await refresh();
        } catch (e) {
          setError(errorText(e));
        }
      }}
    >
      <label>
        Provider
        <select
          value={addingKind}
          onChange={(event) =>
            setAddingKind(event.target.value as ModelProviderKind)
          }
        >
          <option value="openrouter">OpenRouter</option>
          <option value="opencode-go">OpenCode Go</option>
          <option value="custom">Custom (OpenAI-compatible URL)</option>
        </select>
      </label>
      {addingKind === "custom" && (
        <>
          <label>
            Label
            <input name="label" placeholder="My local server" required />
          </label>
          <label>
            Base URL
            <input
              name="baseUrl"
              placeholder="https://your-gateway.example.com"
              required
            />
          </label>
        </>
      )}
      <div className="dialog-actions">
        <button
          className="secondary"
          type="button"
          onClick={() => setAddingKind("")}
        >
          Cancel
        </button>
        <button className="primary" type="submit">
          Add
        </button>
      </div>
    </form>
  );

  return (
    <div className="providers-settings">
      <p className="muted">
        Run Claude Code against another Anthropic-compatible backend. The API
        key is stored locally, encrypted with your macOS Keychain when
        available. OpenRouter only routes Claude models (billed through
        OpenRouter instead of Anthropic directly); for other open models, use
        OpenCode Go.
      </p>
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      {providers.map((provider) => {
        const mine = profiles.filter((p) => p.providerId === provider.id);
        return (
          <div className="provider-card" key={provider.id}>
            <div className="provider-card-head">
              {provider.kind === "custom" ? (
                <Server size={15} />
              ) : (
                <Globe size={15} />
              )}
              <strong>{provider.label}</strong>
              {provider.label !== KIND_LABEL[provider.kind] && (
                <span className="provider-kind">
                  {KIND_LABEL[provider.kind]}
                </span>
              )}
              {provider.kind === "custom" && (
                <small title={provider.baseUrl}>{provider.baseUrl}</small>
              )}
              <button
                title={`Remove ${provider.label}`}
                disabled={busy === provider.id}
                onClick={async () => {
                  if (
                    !window.confirm(
                      `Remove ${provider.label}?\n\nThe provider and its API key are removed. This cannot be undone.`,
                    )
                  )
                    return;
                  setBusy(provider.id);
                  try {
                    await window.bridge!.providersDelete(provider.id);
                    await refresh();
                  } catch (e) {
                    setError(errorText(e));
                  } finally {
                    setBusy("");
                  }
                }}
              >
                <Trash2 size={13} />
              </button>
            </div>
            <form
              className="form-row provider-key-form"
              onSubmit={async (event) => {
                event.preventDefault();
                const formEl = event.currentTarget;
                const form = new FormData(formEl);
                const key = String(form.get("key") || "");
                if (!key) return;
                setBusy(provider.id);
                try {
                  await window.bridge!.providersSetKey(provider.id, key);
                  formEl.reset();
                  await refresh();
                } catch (e) {
                  setError(errorText(e));
                } finally {
                  setBusy("");
                }
              }}
            >
              <label>
                API key
                <input
                  name="key"
                  type="password"
                  placeholder={provider.hasKey ? "Replace saved key" : "sk-…"}
                />
              </label>
              <button disabled={busy === provider.id} type="submit">
                {busy === provider.id ? "Saving…" : "Save key"}
              </button>
              {provider.hasKey && (
                <button
                  type="button"
                  disabled={busy === provider.id}
                  onClick={async () => {
                    if (
                      !window.confirm(
                        `Clear the API key for ${provider.label}?\n\nThe key is removed from the Keychain. The provider and its model profiles stay.`,
                      )
                    )
                      return;
                    setBusy(provider.id);
                    try {
                      await window.bridge!.providersClearKey(provider.id);
                      await refresh();
                    } catch (e) {
                      setError(errorText(e));
                    } finally {
                      setBusy("");
                    }
                  }}
                >
                  Clear
                </button>
              )}
            </form>
            <div className="provider-card-status">
              {provider.hasKey ? (
                <span className="ok">
                  <Check size={12} /> Key saved
                  {provider.keyPlaintext
                    ? " (unencrypted — no Keychain on this machine)"
                    : ""}
                </span>
              ) : (
                <span>No key saved</span>
              )}
              <button
                type="button"
                disabled={busy === `test:${provider.id}` || !provider.hasKey}
                title={provider.hasKey ? undefined : "Save an API key first"}
                onClick={async () => {
                  setBusy(`test:${provider.id}`);
                  try {
                    const result = await window.bridge!.providersTest(
                      provider.id,
                    );
                    setStatus((s) => ({ ...s, [provider.id]: result.message }));
                  } catch (e) {
                    setStatus((s) => ({ ...s, [provider.id]: errorText(e) }));
                  } finally {
                    setBusy("");
                  }
                }}
              >
                {busy === `test:${provider.id}`
                  ? "Testing…"
                  : "Test connection"}
              </button>
              {status[provider.id] && (
                <span className="provider-card-result">
                  {status[provider.id]}
                </span>
              )}
            </div>
            <div className="provider-profiles">
              {mine.length > 0 && (
                <table className="profile-table">
                  <thead>
                    <tr>
                      <th>Model</th>
                      <th>Effort</th>
                      <th>Context</th>
                      <th />
                    </tr>
                  </thead>
                  <tbody>
                    {mine.map((profile) => (
                      <tr
                        className="profile-row"
                        key={profile.id}
                        title={`Edit ${profile.label}`}
                        onClick={() =>
                          setEditingProfile({
                            providerId: provider.id,
                            profile,
                          })
                        }
                      >
                        <td>{profile.label}</td>
                        <td className="p-meta">
                          {profile.effort || "default"}
                        </td>
                        <td className="p-meta">
                          {contextLabel(profile.contextWindow)}
                        </td>
                        <td>
                          <span className="row-actions">
                            <button
                              className="p-edit"
                              title={`Edit ${profile.label}`}
                              onClick={(event) => {
                                event.stopPropagation();
                                setEditingProfile({
                                  providerId: provider.id,
                                  profile,
                                });
                              }}
                            >
                              <Pencil size={12} />
                            </button>
                            <button
                              title={`Remove ${profile.label}`}
                              disabled={busy === profile.id}
                              onClick={async (event) => {
                                event.stopPropagation();
                                setBusy(profile.id);
                                try {
                                  await window.bridge!.modelProfilesDelete(
                                    profile.id,
                                  );
                                  refresh();
                                } catch (e) {
                                  setError(errorText(e));
                                } finally {
                                  setBusy("");
                                }
                              }}
                            >
                              <X size={12} />
                            </button>
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              {editingProfile?.providerId === provider.id ? (
                <ProfileForm
                  provider={provider}
                  profile={editingProfile.profile}
                  profiles={profiles}
                  onDone={() => {
                    setEditingProfile(null);
                    refresh();
                  }}
                />
              ) : (
                <button
                  className="add-profile"
                  onClick={() => setEditingProfile({ providerId: provider.id })}
                >
                  <Plus size={12} /> Add model profile
                </button>
              )}
            </div>
          </div>
        );
      })}
      {addProviderForm || (
        <button className="add-row" onClick={() => setAddingKind("openrouter")}>
          <Plus size={12} /> Add provider
        </button>
      )}
    </div>
  );
}
