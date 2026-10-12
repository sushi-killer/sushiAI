import { useEffect, useState } from "react";
import { tokenStatus, tokenSet, tokenClear } from "./client";
import "./vault.css";

/** The Settings tab of the Vault module: the read-only token, written here
 * and never read back. */
export function VaultSettingsView() {
  const [saved, setSaved] = useState(false);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    if (window.bridge)
      tokenStatus()
        .then((status) => live && setSaved(status.saved))
        .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  async function run(action: () => Promise<{ saved: boolean }>) {
    setBusy(true);
    setError("");
    try {
      setSaved((await action()).saved);
      setToken("");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <form
      className="vault-token"
      onSubmit={(event) => {
        event.preventDefault();
        if (token.trim()) void run(() => tokenSet(token.trim()));
      }}
    >
      <div className="vault-token-head">
        <strong>Vault token</strong>
        {saved && <span className="vault-token-state">Saved</span>}
      </div>
      <div className="vault-token-row">
        <input
          type="password"
          aria-label="Vault token"
          autoComplete="off"
          spellCheck={false}
          placeholder={saved ? "Replace the saved token" : "Paste a token"}
          value={token}
          onChange={(event) => setToken(event.target.value)}
        />
        <button
          className="primary"
          type="submit"
          disabled={busy || !token.trim()}
        >
          Save
        </button>
        {saved && (
          <button
            className="secondary"
            type="button"
            disabled={busy}
            onClick={() => void run(() => tokenClear())}
          >
            Remove
          </button>
        )}
      </div>
      <p className="muted">Read-only token, stored encrypted on this Mac.</p>
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}
