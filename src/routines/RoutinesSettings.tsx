import { useEffect, useState } from "react";
import {
  ChoiceGroup,
  Eyebrow,
  MetaLine,
  Segmented,
  SettingRow,
  Toggle,
} from "../ui/index.ts";
import { routinesClient } from "./client.ts";
import { effortsFor, harnessProblem, HARNESSES, modelsFor } from "./model.ts";
import type { BuilderSettings, RoutinesCatalog } from "./types.ts";

type Account = { id: string; label: string };

/** R9, the Settings tab of Routines: which agent builds a routine and where
 * it starts. A change applies to the next build only. */
export function RoutinesSettings() {
  const [settings, setSettings] = useState<BuilderSettings | null>(null);
  const [catalog, setCatalog] = useState<RoutinesCatalog | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [error, setError] = useState("");

  useEffect(() => {
    let live = true;
    routinesClient
      .settingsGet()
      .then((value) => live && setSettings(value))
      .catch((reason) => live && setError(String(reason?.message ?? reason)));
    routinesClient
      .catalog()
      .then((value) => live && setCatalog(value))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  const harness = settings?.harness ?? "claude";
  useEffect(() => {
    let live = true;
    setAccounts([]);
    const list =
      harness === "claude"
        ? window.bridge?.claudeAccountsList()
        : window.bridge?.codexAccountsList();
    void list
      ?.then((items) => {
        if (!live) return;
        setAccounts(
          items.map((item) => ({
            id: String(item.id),
            label: item.label,
          })),
        );
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [harness]);

  if (!settings)
    return error ? (
      <p role="alert" className="rt-error">
        {error}
      </p>
    ) : (
      <div className="loading">Loading settings…</div>
    );

  async function change(patch: Partial<BuilderSettings>) {
    setError("");
    try {
      // A host that does not echo a field keeps what the owner just chose.
      const saved = await routinesClient.settingsSet(patch);
      setSettings({ ...settings!, ...patch, ...saved });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    }
  }

  const sees = settings.sees ?? { projects: true, connections: true };
  // Codex's sandbox cannot reach the daemon socket, so its builder uses MCP only.
  const codex = settings.harness === "codex";
  const access = codex ? "mcp" : (settings.access ?? "cli");
  const models = modelsFor(catalog, settings.host, settings.harness);
  const problem = harnessProblem(catalog, settings.host, settings.harness);
  const hosts = (catalog?.hosts ?? []).filter(
    (host) => host.state === "ready" || host.id === settings.host,
  );
  return (
    <div className="rt-settings">
      <section aria-label="Routine builder">
        <Eyebrow>Routine builder</Eyebrow>
        <MetaLine>
          Turns your words into a routine and runs the dry run. Each routine
          picks its own host.
        </MetaLine>
        <ChoiceGroup
          label="Builder agent"
          value={settings.harness}
          options={HARNESSES.map((item) => ({
            id: item.id,
            title: item.label,
            detail:
              item.id === "claude"
                ? "Anthropic models. Your Claude account or API key."
                : "OpenAI models. Your ChatGPT account or API key.",
          }))}
          onChange={(harness) =>
            void change({ harness, model: "", accountId: "" })
          }
        />
        <div className="rt-fields">
          <label>
            <span className="rt-field-label">Model</span>
            <select
              className="ui-select"
              aria-label="Builder model"
              value={settings.model}
              onChange={(event) => void change({ model: event.target.value })}
            >
              <option value="">Harness default</option>
              {models.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span className="rt-field-label">Effort</span>
            <select
              className="ui-select"
              aria-label="Builder thinking"
              value={settings.effort}
              onChange={(event) => void change({ effort: event.target.value })}
            >
              {effortsFor(settings.harness).map((level) => (
                <option key={level} value={level}>
                  {level}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span className="rt-field-label">Account</span>
            <select
              className="ui-select"
              aria-label="Builder account"
              value={settings.accountId}
              onChange={(event) =>
                void change({ accountId: event.target.value })
              }
            >
              <option value="">Default account</option>
              {accounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span className="rt-field-label">Starts on</span>
            <select
              className="ui-select"
              aria-label="Builder host"
              value={settings.host}
              onChange={(event) => void change({ host: event.target.value })}
            >
              {hosts.map((host) => (
                <option key={host.id} value={host.id}>
                  {host.name}
                </option>
              ))}
            </select>
          </label>
        </div>
        {problem && <MetaLine>{problem}.</MetaLine>}
      </section>
      <section aria-label="What it sees">
        <Eyebrow>What it sees</Eyebrow>
        <MetaLine>
          Read-only while it builds. A routine writes only after you save it and
          choose how it runs.
        </MetaLine>
        <SettingRow
          title="All projects and hosts"
          detail="Names, paths, README, AGENTS.md, recent tasks and routines - so it knows who the routine is for."
        >
          <Toggle
            checked={sees.projects}
            label="All projects and hosts"
            onChange={(value) =>
              void change({ sees: { ...sees, projects: value } })
            }
          />
        </SettingRow>
        <SettingRow
          title="Connections"
          detail="Asana, Slack, GitHub: reads the data a routine will use to check names and fields."
        >
          <Toggle
            checked={sees.connections}
            label="Connections"
            onChange={(value) =>
              void change({ sees: { ...sees, connections: value } })
            }
          />
        </SettingRow>
        <SettingRow
          title="Access through"
          detail={
            codex
              ? "Codex runs in a sandbox that cannot reach the daemon, so it uses the sushiai MCP server."
              : "The sushiai CLI works with any harness. MCP gives the same tools to agents that prefer it."
          }
        >
          <Segmented
            label="Access through"
            value={access}
            options={
              codex
                ? [{ id: "mcp", label: "MCP" }]
                : [
                    { id: "cli", label: "CLI" },
                    { id: "mcp", label: "MCP" },
                    { id: "both", label: "Both" },
                  ]
            }
            onChange={(next) => void change({ access: next })}
          />
        </SettingRow>
      </section>
      {error && (
        <p role="alert" className="rt-error">
          {error}
        </p>
      )}
    </div>
  );
}
