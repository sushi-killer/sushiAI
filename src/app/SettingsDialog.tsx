import { Suspense, useCallback, useEffect, useState } from "react";
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
import {
  OPEN_PROJECT_SETTINGS_EVENT,
  OPEN_SETTINGS_EVENT,
  setPendingProjectTab,
  type ProjectSettingsTab,
} from "./openSettings.ts";
import { useOrchestratorEnabled } from "../orchestrator/enabled.ts";
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
  MascotShortcutStatus,
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
    description: "The machines sushiAI can reach.",
  },
  {
    key: "providers",
    label: "Providers",
    icon: Server,
    description: "Claude and Codex accounts, API keys and model profiles.",
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

function shortcutLabel(accelerator: string) {
  return accelerator.replace("Alt+", "\u2325");
}

const DEFAULT_APP_PREFERENCES: AppPreferences = {
  runInMenuBar: true,
  notifications: true,
  desktopMascot: true,
  mascotShortcut: false,
};

export function SettingsDialog({
  settingsTab,
  setSettingsTab,
  socket,
  setSocket,
  fontScale,
  setFontScale,
  keepAwake,
  setKeepAwake,
  updates,
  system,
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
  fontScale: number;
  setFontScale(value: number): void;
  keepAwake: boolean;
  setKeepAwake(on: boolean): void;
  updates: UpdateState | null;
  system: System | null;
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

  const [shortcutStatus, setShortcutStatus] =
    useState<MascotShortcutStatus | null>(null);

  useEffect(() => {
    let cancelled = false;
    window.bridge
      ?.mascotShortcutStatus()
      .then((value) => {
        if (!cancelled) setShortcutStatus(value);
      })
      .catch(() => {});
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

  function refreshShortcutStatus() {
    return window.bridge
      ?.mascotShortcutStatus()
      .then(setShortcutStatus)
      .catch(() => {});
  }

  function setAppPreference(key: keyof AppPreferences, value: boolean) {
    setAppPreferences((prev) => ({ ...prev, [key]: value }));
    window.bridge
      ?.appPreferencesSet({ [key]: value })
      .then(() => refreshShortcutStatus())
      .catch((error) => notify(errorText(error)));
  }

  // Off: no Orchestration tab, and a saved one falls back to General.
  const orchestrator = useOrchestratorEnabled();
  const tabs = SETTINGS_NAV.filter(
    (item) => orchestrator || item.key !== "orchestration",
  );
  const tab = tabs.some((item) => item.key === settingsTab)
    ? settingsTab
    : "general";
  const current = tabs.find((item) => item.key === tab) || tabs[0];

  return (
    <div className="settings-shell">
      <nav className="settings-nav" role="tablist" aria-label="Settings">
        <div className="settings-nav-head">
          <p className="settings-nav-eyebrow">PREFERENCES</p>
          <p className="settings-nav-title">Settings</p>
        </div>
        {tabs.map(({ key, label, icon: Icon }) => (
          <button
            key={key}
            className={`settings-nav-item${tab === key ? " current" : ""}`}
            role="tab"
            aria-selected={tab === key}
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
            {tab === "connections" ? (
              <ConnectionsSettings
                endpoint={socket}
                localSocket={system?.socketPath || ""}
                onSelect={(value) => setSocket(value)}
                profiles={connectionProfiles}
                onRefresh={refreshConnectionProfiles}
                notify={notify}
              />
            ) : tab === "providers" ? (
              <ProvidersSettings />
            ) : tab === "orchestration" ? (
              <OrchestratorSettings />
            ) : tab === "updates" ? (
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
                  {orchestrator && (
                    <label className="setting-check">
                      <input
                        type="checkbox"
                        checked={appPreferences.desktopMascot}
                        disabled={!appPreferences.notifications}
                        onChange={(event) =>
                          setAppPreference(
                            "desktopMascot",
                            event.target.checked,
                          )
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
                  )}
                  <label className="setting-check">
                    <input
                      type="checkbox"
                      checked={appPreferences.mascotShortcut}
                      onChange={(event) =>
                        setAppPreference("mascotShortcut", event.target.checked)
                      }
                    />
                    <span>
                      Global shortcut to open the mascot
                      <em>
                        {appPreferences.mascotShortcut && shortcutStatus?.failed
                          ? `${shortcutLabel(shortcutStatus.accelerator)} is already taken by another app, so the shortcut is not active.`
                          : `Press ${shortcutLabel(shortcutStatus?.accelerator ?? "Alt+Space")} from any app. Off by default so it does not clash with launchers.`}
                      </em>
                    </span>
                  </label>
                </div>
                <div className="settings-note">
                  <TerminalSquare size={16} />
                  <p>
                    Sessions keep running in the sushiai daemon when you close
                    sushiAI.
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

/** The Settings dialog's tab, plus an opener that lands on Connections -
 * the same function every render (as long as `setDialog` is), so a memoized
 * panel that receives it does not re-render. */
export function useSettingsTab(
  setDialog: (
    dialog:
      | { kind: "settings" }
      | {
          kind: "workspace-actions";
          workspaceId: string;
          cwd: string;
          connection?: string;
        },
  ) => void,
) {
  const [tab, setTab] = useState<SettingsTab>("general");
  const openConnections = useCallback(() => {
    setTab("connections");
    setDialog({ kind: "settings" });
  }, [setDialog]);
  useEffect(() => {
    const open = (event: Event) => {
      setTab((event as CustomEvent<SettingsTab>).detail);
      setDialog({ kind: "settings" });
    };
    window.addEventListener(OPEN_SETTINGS_EVENT, open);
    return () => window.removeEventListener(OPEN_SETTINGS_EVENT, open);
  }, [setDialog]);
  useEffect(() => {
    const open = (event: Event) => {
      const {
        cwd,
        tab: target,
        connection,
      } = (
        event as CustomEvent<{
          cwd: string;
          tab: ProjectSettingsTab;
          connection?: string;
        }>
      ).detail;
      setPendingProjectTab(target, cwd, connection);
      setDialog({
        kind: "workspace-actions",
        workspaceId: "",
        cwd,
        connection,
      });
    };
    window.addEventListener(OPEN_PROJECT_SETTINGS_EVENT, open);
    return () => window.removeEventListener(OPEN_PROJECT_SETTINGS_EVENT, open);
  }, [setDialog]);
  return [tab, setTab, openConnections] as const;
}
