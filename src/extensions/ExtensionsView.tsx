import { Check, ChevronRight, ListTodo, Power, RefreshCw } from "lucide-react";
import { useState } from "react";
import type {
  ExtensionManifest,
  ExtensionSnapshot,
  ExtensionSource,
} from "./types.ts";

const HOST_LABELS: Record<string, string> = {
  "app.page": "page",
  "workspace.pane": "workspace pane",
  "workspace.tab": "workspace tab",
  "dashboard.section": "Dashboard section",
  "sessions.section": "Sessions section",
  "skills.section": "Skills section",
  "settings.section": "Settings section",
};

const PLACEMENT_LABELS: Record<string, string> = {
  "mode.primary": "top bar",
  "sidebar.primary": "sidebar",
  "dashboard.navigation": "Dashboard header",
  "sessions.navigation": "Sessions header",
  "skills.navigation": "Skills header",
  "panel.picker": "add-panel list",
  "workspace.toolbar.start": "toolbar, first",
  "workspace.toolbar.before-tidy": "toolbar, before Tidy",
  "workspace.toolbar.after-tidy": "toolbar, after Tidy",
  "workspace.toolbar.end": "toolbar, last",
  "workspace.folder.actions": "workspace row",
};

const SCOPE_LABELS: Record<string, string> = {
  instance: "per pane",
  project: "per project",
  global: "one for this Mac",
};

/** What an extension actually contributes, in the words of the UI rather than
 * the contract: this is the answer to "what will this add to my app?". */
function ExtensionBlocks({ manifest }: { manifest: ExtensionManifest }) {
  const { surfaces, navigation, actions } = manifest.contributions;
  return (
    <div className="extension-blocks">
      {surfaces.map((surface) => (
        <div className="extension-block" key={surface.id}>
          <strong>{surface.title}</strong>
          <span>
            {surface.allowedHosts
              .map((host) => HOST_LABELS[host] || host)
              .join(" · ")}
          </span>
          <em>
            {SCOPE_LABELS[surface.stateScope]}
            {surface.instancePolicy === "singleton" ? " · one at a time" : ""}
          </em>
        </div>
      ))}
      {navigation.map((item) => (
        <div className="extension-block" key={`nav:${item.id}`}>
          <strong>{item.label}</strong>
          <span>
            {PLACEMENT_LABELS[item.defaultPlacement] || item.defaultPlacement}
          </span>
          <em>opens {item.targetSurfaceId}</em>
        </div>
      ))}
      {actions.map((action) => (
        <div className="extension-block" key={`action:${action.id}`}>
          <strong>{action.label}</strong>
          <span>
            {PLACEMENT_LABELS[action.defaultPlacement] ||
              action.defaultPlacement}
          </span>
          <em>runs {action.commandId}</em>
        </div>
      ))}
    </div>
  );
}

function extensionSourceLabel(source: ExtensionSource): string {
  if (source.kind === "builtin") return "Built-in";
  if (source.kind === "local")
    return `Local:${source.path.split("/").filter(Boolean).pop() || source.path}`;
  if (source.kind === "npm") return `npm:${source.package}@${source.version}`;
  return `Git:${source.url}#${source.requestedRef}`;
}

export function ExtensionsView({
  snapshot,
  onSetEnabled,
  onRefresh,
}: {
  snapshot: ExtensionSnapshot;
  onSetEnabled(extensionId: string, enabled: boolean): void;
  onRefresh(): void;
}) {
  const [open, setOpen] = useState<string | null>(null);
  return (
    <div className="extensions-view">
      {snapshot.localDir && (
        <section className="extensions-local">
          <div className="extensions-local-head">
            <div>
              <strong>Local extensions</strong>
              <p>
                Drop a folder with a <code>manifest.json</code> here, then
                reload. Nothing in the folder is executed.
              </p>
              <code className="extensions-local-path">{snapshot.localDir}</code>
            </div>
            <button className="extension-toggle" onClick={onRefresh}>
              <RefreshCw size={14} /> Reload
            </button>
          </div>
          {snapshot.problems.map((problem) => (
            <div className="extension-problem" key={problem.path}>
              <strong>{problem.folder}</strong>
              <span>{problem.error}</span>
            </div>
          ))}
        </section>
      )}
      <div className="extensions-grid">
        {snapshot.extensions.map((extension) => {
          const enabled = extension.status === "active";
          return (
            <article
              className={`extension-card ${enabled ? "enabled" : ""}`}
              key={extension.manifest.id}
            >
              <button
                className="extension-card-icon"
                aria-expanded={open === extension.manifest.id}
                aria-label={`What ${extension.manifest.name} adds`}
                onClick={() =>
                  setOpen(
                    open === extension.manifest.id
                      ? null
                      : extension.manifest.id,
                  )
                }
              >
                <ListTodo size={18} />
                <ChevronRight className="chevron" size={12} />
              </button>
              <div className="extension-card-copy">
                <div className="extension-card-title">
                  <strong>{extension.manifest.name}</strong>
                  <span>{extension.manifest.version}</span>
                </div>
                <p>
                  {extension.manifest.description ||
                    "App-wide SushiAI extension"}
                </p>
                {extension.error && (
                  <small className="settings-error">{extension.error}</small>
                )}
                <small>{extensionSourceLabel(extension.manifest.source)}</small>
              </div>
              <div className="extension-card-actions">
                {extension.manifest.source.kind === "builtin" ? (
                  <span className="extension-toggle static">
                    <Check size={14} /> Built-in
                  </span>
                ) : (
                  <button
                    className={`extension-toggle ${enabled ? "active" : ""}`}
                    aria-label={`${enabled ? "Disable" : "Enable"} ${extension.manifest.name}`}
                    onClick={() =>
                      onSetEnabled(extension.manifest.id, !enabled)
                    }
                  >
                    {enabled ? <Check size={14} /> : <Power size={14} />}
                    {enabled ? "Enabled" : "Disabled"}
                  </button>
                )}
              </div>
              {open === extension.manifest.id && (
                <ExtensionBlocks manifest={extension.manifest} />
              )}
            </article>
          );
        })}
      </div>
    </div>
  );
}
