import { useEffect, useState } from "react";
import {
  addressDismiss,
  addressGet,
  addressSet,
  tokenStatus,
  tokenSet,
  tokenClear,
} from "./client";
import "./vault.css";

function VaultAddress() {
  const [stored, setStored] = useState("");
  const [conflict, setConflict] = useState("");
  const [address, setAddress] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let live = true;
    if (window.bridge)
      addressGet()
        .then((value) => {
          if (!live) return;
          setStored(value.address);
          setAddress(value.address);
          setConflict(value.conflict);
        })
        .catch(() => {});
    return () => {
      live = false;
    };
  }, []);
  async function save() {
    setBusy(true);
    setError("");
    try {
      const value = await addressSet(address.trim());
      setStored(value.address);
      setAddress(value.address);
      setConflict(value.conflict);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  async function dismiss() {
    setError("");
    try {
      setConflict((await addressDismiss()).conflict);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }
  return (
    <form
      className="vault-address"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div className="vault-token-head">
        <strong>Vault address</strong>
      </div>
      <div className="vault-token-row">
        <input
          className="pd-mono"
          aria-label="Vault address"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://vault.example.com"
          value={address}
          onChange={(event) => setAddress(event.target.value)}
        />
        <button
          className="primary"
          type="submit"
          disabled={busy || address.trim() === stored}
        >
          Save
        </button>
      </div>
      <p className="muted">One address for every project on This Mac.</p>
      {conflict && (
        <div className="vault-conflict" role="status">
          <p>{conflict}</p>
          <button
            className="ui-button ghost"
            type="button"
            onClick={() => void dismiss()}
          >
            Dismiss
          </button>
        </div>
      )}
      {error && (
        <p className="inline-error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

function VaultToken() {
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

/** The Settings tab of the Vault module: the address, then the read-only
 * token, written here and never read back. */
export function VaultSettingsView() {
  return (
    <>
      <VaultAddress />
      <VaultToken />
    </>
  );
}
