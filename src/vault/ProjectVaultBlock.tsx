import { useEffect, useState } from "react";
import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";
import { addressGet, projectGet, projectSet, type Config } from "./client";
import "./vault.css";

type Draft = { mount: string; paths: string[] };

const PLACEHOLDERS = ["team/shared", "team/app"];

function draftOf(vault: Config | null): Draft {
  return {
    mount: vault?.mount ?? "",
    paths: vault?.paths.length ? [...vault.paths] : [""],
  };
}

function same(a: Draft, b: Draft): boolean {
  return (
    a.mount === b.mount &&
    a.paths.length === b.paths.length &&
    a.paths.every((path, index) => path === b.paths[index])
  );
}

/** Where This Mac reads the project's values from in Vault (KV v2). The
 * token lives in Settings → Vault; values are never shown here. */
export function ProjectVaultBlock({ projectId }: { projectId: string }) {
  const [saved, setSaved] = useState<Config | null>(null);
  const [draft, setDraft] = useState<Draft>(() => draftOf(null));
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [address, setAddress] = useState<string | null>(null);
  const dirty = !same(draft, draftOf(saved));

  // Show what is stored for this project.
  useEffect(() => {
    let live = true;
    setSaved(null);
    setDraft(draftOf(null));
    if (window.bridge) {
      addressGet()
        .then((value) => live && setAddress(value.address))
        .catch(() => {});
      projectGet(projectId)
        .then((config) => {
          if (!live) return;
          setSaved(config);
          setDraft(draftOf(config));
        })
        .catch(() => {});
    }
    return () => {
      live = false;
    };
  }, [projectId]);

  function edit(next: Partial<Draft>) {
    setDraft((current) => ({ ...current, ...next }));
  }
  function editPath(index: number, value: string) {
    edit({
      paths: draft.paths.map((path, at) => (at === index ? value : path)),
    });
  }
  function move(index: number, by: -1 | 1) {
    const paths = [...draft.paths];
    [paths[index], paths[index + by]] = [paths[index + by], paths[index]];
    edit({ paths });
  }

  async function save() {
    if (!window.bridge) return;
    setBusy(true);
    setError("");
    try {
      const mount = draft.mount.trim();
      const paths = draft.paths.map((path) => path.trim()).filter(Boolean);
      const config = await projectSet(
        projectId,
        !paths.length ? null : { ...(mount ? { mount } : {}), paths },
      );
      setSaved(config);
      setDraft(draftOf(config));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="pd-vault" aria-label="Vault">
      <h3 className="pd-vault-title">Vault</h3>
      <p className="pd-vault-hint">
        Vault names show in the table above with a Vault tag. A variable you add
        here wins over Vault. Set the address and token in Settings → Vault.
      </p>
      {address === "" && (
        <p className="pd-vault-hint">No Vault address is set yet.</p>
      )}
      <div className="pd-vault-fields">
        <label className="pd-vault-field">
          <span>Mount</span>
          <input
            className="pd-input pd-mono"
            placeholder="secret"
            value={draft.mount}
            onChange={(event) => edit({ mount: event.target.value })}
          />
        </label>
      </div>
      <div className="pd-vault-paths" role="list" aria-label="Vault paths">
        <span className="pd-vault-label">Paths</span>
        {draft.paths.map((path, index) => (
          <div className="pd-vault-path" role="listitem" key={index}>
            <input
              className="pd-input pd-mono"
              aria-label={`Path ${index + 1}`}
              placeholder={PLACEHOLDERS[index % PLACEHOLDERS.length]}
              value={path}
              onChange={(event) => editPath(index, event.target.value)}
            />
            <button
              type="button"
              className="pd-vault-icon"
              aria-label={`Move path ${index + 1} up`}
              disabled={index === 0}
              onClick={() => move(index, -1)}
            >
              <ArrowUp size={13} aria-hidden />
            </button>
            <button
              type="button"
              className="pd-vault-icon"
              aria-label={`Move path ${index + 1} down`}
              disabled={index === draft.paths.length - 1}
              onClick={() => move(index, 1)}
            >
              <ArrowDown size={13} aria-hidden />
            </button>
            <button
              type="button"
              className="pd-vault-icon"
              aria-label={`Remove path ${index + 1}`}
              onClick={() =>
                edit({
                  paths:
                    draft.paths.length > 1
                      ? draft.paths.filter((_, at) => at !== index)
                      : [""],
                })
              }
            >
              <X size={13} aria-hidden />
            </button>
          </div>
        ))}
        <p className="pd-vault-hint">A later path overrides an earlier one.</p>
        <button
          type="button"
          className="ui-button ghost pd-vault-add"
          onClick={() => edit({ paths: [...draft.paths, ""] })}
        >
          <Plus size={13} aria-hidden /> Add path
        </button>
      </div>
      {error && (
        <p role="alert" className="pd-alert">
          {error}
        </p>
      )}
      <div className="pd-vault-actions">
        <button
          type="button"
          className="ui-button primary"
          disabled={busy || !dirty}
          onClick={() => void save()}
        >
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
    </section>
  );
}
