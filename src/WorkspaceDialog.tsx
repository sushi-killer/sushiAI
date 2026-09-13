import { useEffect, useState } from "react";
import { ArrowUpRight, FolderOpen } from "lucide-react";
import type { ClaudePlugin } from "./types";

export function WorkspaceDialog({
  defaultCwd,
  endpoint,
  connected,
  onCreate,
}: {
  defaultCwd: string;
  endpoint: string;
  connected: boolean;
  onCreate(
    name: string,
    cwd: string,
    backend: string,
    starter: string,
    pluginChanges: Array<{ name: string; disabled: boolean }>,
  ): Promise<void>;
}) {
  const [cwd, setCwd] = useState(defaultCwd),
    [busy, setBusy] = useState(false),
    [backend, setBackend] = useState(connected ? "herdr" : "local");
  const [plugins, setPlugins] = useState<ClaudePlugin[]>([]);
  const [disabledPlugins, setDisabledPlugins] = useState<string[]>([]);
  const [pluginsLoading, setPluginsLoading] = useState(false);
  const remote = backend === "herdr" && endpoint.startsWith("ssh:");
  const targetEndpoint = remote ? endpoint : undefined;
  useEffect(() => {
    let cancelled = false;
    if (remote)
      window.bridge
        ?.projectInspect(endpoint, { operation: "home" })
        .then((info) => {
          if (!cancelled) setCwd(info.home);
        })
        .catch(() => {});
    else setCwd(defaultCwd);
    return () => {
      cancelled = true;
    };
  }, [remote, endpoint, defaultCwd]);
  useEffect(() => {
    let cancelled = false;
    setPluginsLoading(true);
    setPlugins([]);
    setDisabledPlugins([]);
    const request = window.bridge?.claudePluginsList(cwd, targetEndpoint);
    if (!request) {
      setPluginsLoading(false);
      return () => {
        cancelled = true;
      };
    }
    request
      .then((result) => {
        if (cancelled) return;
        setPlugins(result.plugins);
        setDisabledPlugins(
          result.plugins
            .filter((plugin) => plugin.disabled)
            .map((plugin) => plugin.name),
        );
      })
      .catch(() => {
        if (!cancelled) {
          setPlugins([]);
          setDisabledPlugins([]);
        }
      })
      .finally(() => {
        if (!cancelled) setPluginsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [cwd, targetEndpoint]);
  return (
    <>
      <div className="dialog-eyebrow">A PLACE TO BUILD</div>
      <h2>New workspace</h2>
      <p>Keep your agents and project together.</p>
      <form
        onSubmit={async (event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          setBusy(true);
          try {
            await onCreate(
              String(data.get("name")),
              cwd,
              backend,
              String(data.get("starter")),
              plugins.flatMap((plugin) => {
                const disabled = disabledPlugins.includes(plugin.name);
                return disabled === plugin.disabled
                  ? []
                  : [{ name: plugin.name, disabled }];
              }),
            );
          } finally {
            setBusy(false);
          }
        }}
      >
        <label>
          Name
          <input
            name="name"
            autoFocus
            placeholder="my-next-project"
            required
            maxLength={80}
          />
        </label>
        <label>
          Project folder
          <div className="folder-field">
            <input
              value={cwd}
              onChange={(event) => setCwd(event.target.value)}
              required
              placeholder="/Users/you/projects/app"
            />
            <button
              type="button"
              title="Choose project folder"
              disabled={remote}
              onClick={async () => {
                const selected = await window.bridge?.chooseDirectory();
                if (selected) setCwd(selected);
              }}
            >
              <FolderOpen size={16} />
            </button>
          </div>
        </label>
        <label>
          Session backend
          <select
            value={backend}
            onChange={(event) => setBackend(event.target.value)}
          >
            <option value="herdr" disabled={!connected}>
              Herdr · persistent sessions
            </option>
            <option value="local">Local · built-in PTY</option>
          </select>
          <small className="workspace-plugin-hint">
            {connected
              ? "Herdr sessions survive closing the app; local shells do not."
              : "Herdr is not connected, so this workspace runs on the built-in shell. Connect it in Settings to choose."}
          </small>
        </label>
        <label>
          Agent / harness
          <select name="starter">
            <option value="shell">One terminal</option>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
            <option value="gemini">Gemini CLI</option>
          </select>
        </label>
        {plugins.length > 0 ? (
          <label>
            Claude Code plugins to disable
            <select
              className="workspace-plugin-picker"
              multiple
              size={Math.min(5, Math.max(3, plugins.length))}
              value={disabledPlugins}
              onChange={(event) =>
                setDisabledPlugins(
                  Array.from(
                    event.currentTarget.selectedOptions,
                    (option) => option.value,
                  ),
                )
              }
            >
              {plugins.map((plugin) => (
                <option value={plugin.name} key={plugin.name}>
                  {plugin.name} · {plugin.disabled ? "off" : "on"}
                </option>
              ))}
            </select>
            <small className="workspace-plugin-hint">
              Select plugins that should be off in this workspace. Existing
              selections reflect their current state.
            </small>
          </label>
        ) : null}
        <button
          type="submit"
          className="primary"
          disabled={busy || pluginsLoading}
        >
          {busy
            ? "Creating…"
            : pluginsLoading
              ? "Finding plugins…"
              : "Create workspace"}
          <ArrowUpRight size={14} />
        </button>
      </form>
    </>
  );
}
