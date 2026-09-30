import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight, LoaderCircle, Trash2 } from "lucide-react";
import { useOrchestratorClient } from "./hostContext";
import { errorText } from "./helpers";
import { toolFromServer, toolStatusLine, withOverride } from "./toolsModel";
import type { ChatToolConfig, ChatToolRow, ChatToolServer } from "./types";

/** The CONNECTED TOOLS section of Settings: a row per tool with a toggle and
 * a line that says what it gives the chat or why it gives nothing, and a
 * menu that adds a server from the owner's own MCP config. */
export function ConnectedTools({
  tools,
  savedTools,
  onChange,
}: {
  tools: ChatToolConfig[];
  /** The list orchd holds: its rows are looked up again when it changes. */
  savedTools: ChatToolConfig[];
  onChange(next: ChatToolConfig[]): void;
}) {
  const client = useOrchestratorClient();
  const [rows, setRows] = useState<ChatToolRow[]>([]);
  const [servers, setServers] = useState<ChatToolServer[]>([]);
  const [checking, setChecking] = useState<string[]>([]);
  const [open, setOpen] = useState("");
  const [error, setError] = useState("");
  const savedKey = JSON.stringify(savedTools);

  useEffect(() => {
    let live = true;
    // What orchd already knows shows at once; each enabled tool is then
    // looked at on its own, so a slow server never holds the others back.
    client
      .chatTools({ cached: true })
      .then((result) => live && setRows(result.tools))
      .catch((e) => live && setError(errorText(e)));
    const ids = JSON.parse(savedKey).flatMap((t: ChatToolConfig) =>
      t.enabled ? [t.id] : [],
    ) as string[];
    setChecking(ids);
    for (const id of ids) {
      client
        .chatTools({ id })
        .then(
          (result) =>
            live &&
            setRows((old) => [
              ...old.filter((r) => r.id !== id),
              ...result.tools,
            ]),
        )
        .catch((e) => live && setError(errorText(e)))
        .finally(
          () => live && setChecking((old) => old.filter((i) => i !== id)),
        );
    }
    client
      .chatToolServers()
      .then((result) => live && setServers(result.servers))
      .catch(() => {
        // An older orchd: nothing to add from.
      });
    return () => {
      live = false;
    };
  }, [client, savedKey]);

  const offered = servers.filter(
    (server) =>
      !tools.some(
        (tool) =>
          (tool.server as { ref?: string }).ref === server.ref ||
          tool.label === server.label,
      ),
  );
  const change = (
    id: string,
    patch: (tool: ChatToolConfig) => ChatToolConfig,
  ) => onChange(tools.map((tool) => (tool.id === id ? patch(tool) : tool)));

  return (
    <div className="os-tools" aria-label="Connected tools">
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
      {tools.map((tool) => {
        const row = rows.find((r) => r.id === tool.id);
        const expanded = open === tool.id;
        const saved = savedTools.some((s) => s.id === tool.id);
        const busy = saved && tool.enabled && checking.includes(tool.id);
        return (
          <div key={tool.id} className="os-tool">
            <div className="os-tool-row">
              <button
                type="button"
                className="os-tool-main"
                aria-expanded={expanded}
                aria-label={`${tool.label} tools`}
                disabled={!row || row.tools.length === 0}
                onClick={() => setOpen(expanded ? "" : tool.id)}
              >
                {expanded ? (
                  <ChevronDown size={12} aria-hidden />
                ) : (
                  <ChevronRight size={12} aria-hidden />
                )}
                <span className="os-tool-text">
                  <span className="os-row-title">{tool.label}</span>
                  <span
                    className={`os-row-desc${
                      tool.enabled && row && row.status !== "ok" && saved
                        ? " os-tool-problem"
                        : ""
                    }`}
                  >
                    {busy && (
                      <LoaderCircle
                        className="spin os-tool-spin"
                        size={11}
                        aria-hidden
                      />
                    )}
                    {busy
                      ? "Checking…"
                      : saved
                        ? toolStatusLine(tool, row)
                        : tool.enabled
                          ? "Save to connect"
                          : "Off"}
                  </span>
                </span>
              </button>
              <button
                type="button"
                className="icon-button"
                aria-label={`Remove ${tool.label}`}
                onClick={() => onChange(tools.filter((t) => t.id !== tool.id))}
              >
                <Trash2 size={14} />
              </button>
              <button
                type="button"
                role="switch"
                aria-checked={tool.enabled}
                aria-label={`Use ${tool.label} in chat`}
                className={`os-toggle${tool.enabled ? " on" : ""}`}
                onClick={() =>
                  change(tool.id, (t) => ({ ...t, enabled: !t.enabled }))
                }
              >
                <span className="os-knob" />
              </button>
            </div>
            {expanded && row && (
              <ul className="os-tool-list">
                {row.tools.map((t) => (
                  <li key={t.name} className="os-tool-item">
                    <span className="os-tool-name">{t.name}</span>
                    <span className="os-select os-select-fixed">
                      <select
                        aria-label={`${t.name} is`}
                        value={tool.overrides?.[t.name] ?? t.kind}
                        onChange={(event) =>
                          change(tool.id, (old) =>
                            withOverride(
                              old,
                              t.name,
                              event.target.value as "read" | "write",
                              t.guess,
                            ),
                          )
                        }
                      >
                        <option value="read">reads</option>
                        <option value="write">asks first</option>
                      </select>
                      <ChevronDown size={12} aria-hidden />
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        );
      })}
      <span className="os-select os-tool-add">
        <select
          aria-label="Add a server from your MCP config"
          value=""
          disabled={offered.length === 0}
          onChange={(event) => {
            const server = offered.find((s) => s.ref === event.target.value);
            if (server)
              onChange([
                ...tools,
                toolFromServer(
                  server,
                  tools.map((t) => t.id),
                ),
              ]);
          }}
        >
          <option value="">Add a server from your MCP config…</option>
          {offered.map((server) => (
            <option key={server.ref} value={server.ref}>
              {server.label} · {server.source}
            </option>
          ))}
        </select>
        <ChevronDown size={12} aria-hidden />
      </span>
      <p className="os-row-desc">
        Reads never ask. A tool that changes something waits for your OK in the
        chat first.
      </p>
    </div>
  );
}
