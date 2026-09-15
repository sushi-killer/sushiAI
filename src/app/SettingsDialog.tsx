import { Suspense } from "react";
import { Check, RefreshCw, TerminalSquare } from "lucide-react";
import { RenderProfiler } from "../RenderProfiler.tsx";
import { agentTitle } from "./agent-title.ts";
import { ExtensionSectionSlot } from "../extensions/ExtensionSlots.tsx";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import {
  ConnectionsSettings,
  ProvidersSettings,
  UpdateSettings,
} from "../dialogs/lazy-settings.ts";
import type {
  ConnectionProfile,
  System,
  UpdateState,
  Workspace,
} from "../types";

export type SettingsTab = "general" | "connections" | "providers" | "updates";

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
  return (
    <>
      <div className="dialog-eyebrow">PREFERENCES</div>
      <h2>Your workspace, connected.</h2>
      <div className="workspace-control-tabs" role="tablist">
        {(
          [
            ["general", "General"],
            ["connections", "Connections"],
            ["providers", "Providers"],
            ["updates", "Updates"],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            className={settingsTab === key ? "selected" : ""}
            role="tab"
            aria-selected={settingsTab === key}
            onClick={() => setSettingsTab(key)}
          >
            {label}
          </button>
        ))}
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
                      <i className={`status-dot ${connected ? "green" : ""}`} />
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
                        socket.startsWith("ssh:") ? system?.socketPath : socket
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
              <div className="settings-note">
                <TerminalSquare size={16} />
                <p>
                  Herdr sessions keep running when you close sushiAI. Local
                  terminals live for the duration of the app.
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
    </>
  );
}
