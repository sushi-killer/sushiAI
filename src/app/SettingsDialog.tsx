import { Suspense, useEffect, useState } from "react";
import {
  Check,
  Globe,
  ListTodo,
  RefreshCw,
  Server,
  Settings as SettingsIcon,
  TerminalSquare,
  type LucideIcon,
} from "lucide-react";
import "./settings-dialog.css";
import { RenderProfiler } from "../RenderProfiler.tsx";
import { agentTitle } from "./agent-title.ts";
import { errorText } from "./errors.ts";
import { ExtensionSectionSlot } from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import {
  ConnectionsSettings,
  OrchestratorSettings,
  ProvidersSettings,
  UpdateSettings,
} from "../dialogs/lazy-settings.ts";
import type {
  AppPreferences,
  ConnectionProfile,
  System,
  UpdateState,
  Workspace,
} from "../types";

export type SettingsTab =
  "general" | "connections" | "providers" | "orchestration" | "updates";

const SETTINGS_NAV: {
  key: SettingsTab;
  label: string;
  icon: LucideIcon;
  description: string;
}[] = [
  {
    key: "general",
    label: "General",
    icon: SettingsIcon,
    description: "Interface size, sleep and background behaviour.",
  },
  {
    key: "connections",
    label: "Connections",
    icon: Globe,
    description: "The Herdr socket and the machines sushiAI can reach.",
  },
  {
    key: "providers",
    label: "Providers",
    icon: Server,
    description: "API keys and model profiles.",
  },
  {
    key: "orchestration",
    label: "Orchestration",
    icon: ListTodo,
    description:
      "How tasks are planned, run, checked and landed. Saved to orchd for every project on this Mac.",
  },
  {
    key: "updates",
    label: "Updates",
    icon: RefreshCw,
    description: "Check for and install new versions of sushiAI.",
  },
];

const DEFAULT_APP_PREFERENCES: AppPreferences = {
  runInMenuBar: true,
  notifications: true,
  desktopMascot: true,
};

export function SettingsDialog({
  settingsTab,
  setSettingsTab,
  socket,
  setSocket,
  connected,
  refreshHerdr,
  fontScale,
  setFontScale,
  keepAwake,
  setKeepAwake,
  updates,
  system,
  connectionError,
  registry,
  cwd,
  connection,
  workspaces,
  connectionProfiles,
  refreshConnectionProfiles,
  notify,
}: {
  settingsTab: SettingsTab;
  setSettingsTab(tab: SettingsTab): void;
  socket: string;
  setSocket(value: string): void;
  connected: boolean;
  refreshHerdr(path: string): Promise<void>;
  fontScale: number;
  setFontScale(value: number): void;
  keepAwake: boolean;
  setKeepAwake(on: boolean): void;
  updates: UpdateState | null;
  system: System | null;
  connectionError: string;
  registry: ExtensionRegistry;
  cwd: string;
  connection?: string;
  workspaces: Workspace[];
  connectionProfiles: ConnectionProfile[];
  refreshConnectionProfiles(): Promise<void>;
  notify(text: string): void;
}) {
  const [appPreferences, setAppPreferences] = useState<AppPreferences>(
    DEFAULT_APP_PREFERENCES,
  );

  useEffect(() => {
    let cancelled = false;
    window.bridge
      ?.appPreferences()
      .then((value) => {
        if (!cancelled) setAppPreferences(value);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  function setAppPreference(key: keyof AppPreferences, value: boolean) {
    setAppPreferences((prev) => ({ ...prev, [key]: value }));
    window.bridge
      ?.appPreferencesSet({ [key]: value })
      .catch((error) => notify(errorText(error)));
  }

  const current =
    SETTINGS_NAV.find((item) => item.key === settingsTab) || SETTINGS_NAV[0];

  return (
    <div className="settings-shell">
      <nav className="settings-nav" role="tablist" aria-label="Settings">
        <div className="settings-nav-head">
          <p className="settings-nav-eyebrow">PREFERENCES</p>
          <p className="settings-nav-title">Settings</p>
        </div>
        {SETTINGS_NAV.map(({ key, label, icon: Icon }) => (
          <button
            key={key}
            className={`settings-nav-item${settingsTab === key ? " current" : ""}`}
            role="tab"
            aria-selected={settingsTab === key}
            onClick={() => setSettingsTab(key)}
          >
            <Icon size={15} />
            {label}
          </button>
        ))}
      </nav>
      <div className="settings-main" role="tabpanel">
        <div className="settings-title">
          <h2>{current.label}</h2>
          <p>{current.description}</p>
        </div>
        <Suspense fallback={<div className="loading">Loading settings…</div>}>
          <RenderProfiler id="settings">
            {settingsTab === "connections" ? (
              <ConnectionsSettings
                endpoint={socket}
                localSocket={system?.socketPath || ""}
                onSelect={(value) => setSocket(value)}
                profiles={connectionProfiles}
                onRefresh={refreshConnectionProfiles}
                notify={notify}
                socketForm={
                  <form
                    className="socket-form"
                    onSubmit={(event) => {
                      event.preventDefault();
                      const value = String(
                        new FormData(event.currentTarget).get("socket"),
                      );
                      setSocket(value);
                      refreshHerdr(value);
                    }}
                  >
                    <div className="socket-head">
                      <label htmlFor="settings-socket">Herdr socket</label>
                      <div className="connection-detail">
                        <i
                          className={`status-dot ${connected ? "green" : ""}`}
                        />
                        {connected
                          ? "Connected · workspaces sync automatically"
                          : connectionError || "Connecting…"}
                      </div>
                    </div>
                    <div className="socket-controls">
                      <input
                        id="settings-socket"
                        name="socket"
                        key={socket}
                        defaultValue={
                          socket.startsWith("ssh:")
                            ? system?.socketPath
                            : socket
                        }
                        placeholder="/Users/you/.config/herdr/herdr.sock"
                        required
                      />
                      <button className="primary" type="submit">
                        <RefreshCw size={14} /> Reconnect
                      </button>
                    </div>
                  </form>
                }
              />
            ) : settingsTab === "providers" ? (
              <ProvidersSettings />
            ) : settingsTab === "orchestration" ? (
              <OrchestratorSettings />
            ) : settingsTab === "updates" ? (
              <UpdateSettings state={updates} />
            ) : (
              <>
                <ExtensionSectionSlot
                  registry={registry}
                  host="settings.section"
                  cwd={cwd}
                  connection={connection}
                  workspaces={workspaces}
                />
                <div className="setting-block">
                  <h4>Interface size</h4>
                  <div
                    className="workspace-control-tabs"
                    role="group"
                    aria-label="Interface size"
                  >
                    {(
                      [
                        [0.95, "Compact"],
                        [1, "Default"],
                        [1.1, "Large"],
                      ] as const
                    ).map(([value, label]) => (
                      <button
                        key={label}
                        role="radio"
                        aria-checked={fontScale === value}
                        className={fontScale === value ? "selected" : ""}
                        onClick={() => setFontScale(value)}
                      >
                        {label}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="setting-block">
                  <h4>Sleep</h4>
                  <label className="setting-check">
                    <input
                      type="checkbox"
                      checked={keepAwake}
                      onChange={(event) => setKeepAwake(event.target.checked)}
                    />
                    <span>
                      Keep this Mac awake while sushiAI is open
                      <em>
                        Stops idle sleep cutting a long agent turn short. The
                        display still sleeps normally.
                      </em>
                    </span>
                  </label>
                </div>
                <div className="setting-block">
                  <h4>Background</h4>
                  <label className="setting-check">
                    <input
                      type="checkbox"
                      checked={appPreferences.runInMenuBar}
                      onChange={(event) =>
                        setAppPreference("runInMenuBar", event.target.checked)
                      }
                    />
                    <span>
                      Keep sushiAI in the menu bar when the window closes
                      <em>
                        Agents keep reporting while the window is closed. Quit
                        from the menu bar icon or ⌘Q.
                      </em>
                    </span>
                  </label>
                  <label className="setting-check">
                    <input
                      type="checkbox"
                      checked={appPreferences.notifications}
                      onChange={(event) =>
                        setAppPreference("notifications", event.target.checked)
                      }
                    />
                    <span>
                      Notify me when an agent needs input or finishes
                      <em>
                        If you don't answer, reminders follow at 5, 10 and 20
                        minutes.
                      </em>
                    </span>
                  </label>
                  <label className="setting-check">
                    <input
                      type="checkbox"
                      checked={appPreferences.desktopMascot}
                      disabled={!appPreferences.notifications}
                      onChange={(event) =>
                        setAppPreference("desktopMascot", event.target.checked)
                      }
                    />
                    <span>
                      Desktop mascot
                      <em>
                        Shows orchestrator task notices as a mascot in the
                        corner of your screen. Off sends a native notification
                        instead.
                      </em>
                    </span>
                  </label>
                </div>
                <div className="settings-note">
                  <TerminalSquare size={16} />
                  <p>
                    Herdr sessions keep running when you close sushiAI. Local
                    terminals live for as long as the app does.
                  </p>
                </div>
                <div className="cli-status">
                  {system?.agents.map((a) => (
                    <div key={a.name}>
                      <span>{agentTitle(a.name)}</span>
                      <span>
                        {a.path ? (
                          <>
                            <Check size={12} /> Installed
                          </>
                        ) : (
                          "Not found"
                        )}
                      </span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </RenderProfiler>
        </Suspense>
      </div>
    </div>
  );
}
