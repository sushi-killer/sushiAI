import { useState } from "react";
import { ArrowUpRight, Check, Download, RefreshCw } from "lucide-react";
import type { UpdateState } from "./types";
export function UpdateSettings({ state }: { state: UpdateState | null }) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const busy =
    pending ||
    state?.phase === "checking" ||
    state?.phase === "downloading" ||
    state?.phase === "installing";
  async function act(action: () => Promise<unknown>) {
    setPending(true);
    setError("");
    try {
      await action();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(false);
    }
  }
  if (!state || !window.bridge?.updatesCheck)
    return (
      <div className="update-settings">
        <h3>Software updates</h3>
        <p className="muted">Open the desktop app to check for updates.</p>
      </div>
    );
  const bridge = window.bridge;
  const title =
    state.phase === "installing"
      ? "Preparing update and restart…"
      : state.phase === "ready"
        ? `${state.release?.version} is ready to install`
        : state.phase === "downloading"
          ? `Downloading ${state.release?.version}…`
          : state.phase === "checking"
            ? "Checking GitHub Releases…"
            : state.release
              ? `${state.release.version} is available`
              : state.phase === "current"
                ? "You’re up to date"
                : "Software updates";
  return (
    <section className="update-settings" aria-label="Software updates">
      <div className="update-heading">
        <div>
          <h3>{title}</h3>
          <p>
            sushiAI {state.currentVersion} · {state.repository}
          </p>
        </div>
        {state.phase === "ready" || state.phase === "current" ? (
          <Check size={20} />
        ) : (
          <Download size={20} />
        )}
      </div>
      {state.phase === "downloading" && (
        <div className="update-progress">
          <progress
            max={100}
            value={state.progress}
            aria-label="Update download progress"
          />
          <span>{state.progress}%</span>
        </div>
      )}
      <div className="update-actions">
        <button
          disabled={busy}
          onClick={() => act(() => bridge.updatesCheck())}
        >
          <RefreshCw size={13} />
          Check for updates
        </button>
        {state.release && state.phase !== "ready" && (
          <button
            className="primary"
            disabled={busy}
            onClick={() => act(() => bridge.updatesDownload())}
          >
            <Download size={13} />
            Download update
          </button>
        )}
        {state.phase === "ready" && (
          <button
            className="primary"
            disabled={busy}
            onClick={() =>
              act(() =>
                state.canInstall
                  ? bridge.updatesInstall()
                  : bridge.updatesOpen(),
              )
            }
          >
            {state.canInstall ? "Install and restart" : "Open installer"}
            <ArrowUpRight size={13} />
          </button>
        )}
        <button
          disabled={pending}
          onClick={() => act(() => bridge.updatesReleasePage())}
        >
          Release notes
          <ArrowUpRight size={13} />
        </button>
      </div>
      {(error || state.error) && (
        <p className="inline-error" role="alert">
          {error || state.error}
        </p>
      )}
      {state.checkedAt && (
        <p className="update-checked">
          Last checked {new Date(state.checkedAt).toLocaleString()}
        </p>
      )}
      <div className="update-preferences">
        {(
          [
            [
              "autoCheck",
              "Check automatically",
              "Check at startup and every six hours.",
            ],
            [
              "autoDownload",
              "Download automatically",
              "Keep the latest compatible DMG ready to install.",
            ],
            [
              "includePrereleases",
              "Include alpha and beta releases",
              "Receive previews as well as stable updates.",
            ],
          ] as const
        ).map(([key, label, help]) => (
          <label key={key}>
            <input
              type="checkbox"
              checked={state.settings[key]}
              disabled={busy}
              onChange={(e) =>
                act(() => bridge.updatesConfigure({ [key]: e.target.checked }))
              }
            />
            <span>
              <strong>{label}</strong>
              <small>{help}</small>
            </span>
          </label>
        ))}
      </div>
      <p className="update-note">
        Save your edits before installing. Installation restarts sushiAI and
        stops local terminals; Herdr sessions keep running. The app verifies the
        package and keeps a recovery copy until the new version opens.
      </p>
      {state.release?.notes && (
        <details className="update-notes">
          <summary>What’s new in {state.release.version}</summary>
          <pre>{state.release.notes}</pre>
        </details>
      )}
    </section>
  );
}
