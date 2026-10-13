import { useState } from "react";
import { ChevronDown } from "lucide-react";
import "./settings-scope.css";
import { orchestratorClientFor } from "./client";
import { errorText } from "./helpers";
import { hostName } from "./hosts";
import { copySettings, HOST_SPECIFIC_KEYS } from "./projectSettings";
import type { OrchestratorHost } from "./types";

/** "Copy settings to...": writes the selected host's saved orchd settings to
 * one other host through that host's own `settings.set`, keeping the
 * target's host-specific keys. The panel's unsaved edits are never part of
 * it: the source is read from its daemon at the moment of the copy. */
export function CopySettings({
  source,
  hosts,
}: {
  source: string;
  hosts: OrchestratorHost[];
}) {
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );
  const others = hosts.filter((host) => host.id !== source);
  if (!others.length) return null;
  const from = hostName(hosts, source);
  const to = target ? hostName(hosts, target) : "";

  async function copy() {
    setBusy(true);
    setResult(null);
    try {
      const sourceClient = orchestratorClientFor(source);
      const targetClient = orchestratorClientFor(target);
      const [saved, current] = await Promise.all([
        sourceClient.settingsGet(),
        targetClient.settingsGet(),
      ]);
      await targetClient.settingsSet(copySettings(saved, current));
      setResult({ ok: true, text: `Copied ${from}'s settings to ${to}.` });
      setTarget("");
    } catch (e) {
      setResult({
        ok: false,
        text: `Could not copy to ${to}: ${errorText(e)}`,
      });
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <span className="os-scope-select os-copy-select">
        <select
          aria-label="Copy settings to"
          value={target}
          disabled={busy}
          onChange={(event) => {
            setTarget(event.target.value);
            setResult(null);
          }}
        >
          <option value="">Copy settings to…</option>
          {others.map((host) => (
            <option key={host.id} value={host.id}>
              {host.name}
            </option>
          ))}
        </select>
        <ChevronDown size={12} aria-hidden />
      </span>
      {(target || result) && (
        <div className="os-copy">
          {target && (
            <div
              className="os-copy-confirm"
              role="alertdialog"
              aria-label={`Copy settings to ${to}`}
            >
              <p>
                Copy the saved orchestration settings of <b>{from}</b> to{" "}
                <b>{to}</b>? They replace {to}'s settings. Unsaved changes on
                this page are not copied.
              </p>
              <p className="os-copy-kept">
                Kept on {to}: {HOST_SPECIFIC_KEYS.join(", ")}.
              </p>
              <div className="os-copy-actions">
                <button
                  type="button"
                  className="os-ghost"
                  disabled={busy}
                  onClick={() => setTarget("")}
                >
                  Cancel
                </button>
                <button
                  type="button"
                  className="os-primary"
                  disabled={busy}
                  onClick={() => void copy()}
                >
                  {busy ? "Copying…" : `Copy to ${to}`}
                </button>
              </div>
            </div>
          )}
          {result && (
            <p
              className={result.ok ? "os-scope-ok" : "inline-error"}
              role={result.ok ? "status" : "alert"}
            >
              {result.text}
            </p>
          )}
        </div>
      )}
    </>
  );
}
