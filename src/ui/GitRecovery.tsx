import { useState } from "react";
import type { ProjectGitFailure } from "../types";

export function GitRecovery({
  projectId,
  endpoint,
  failure,
  onRetry,
  disabled = false,
}: {
  projectId: string;
  endpoint: string;
  failure: ProjectGitFailure;
  onRetry(url?: string): void;
  disabled?: boolean;
}) {
  const [url, setUrl] = useState(
    failure.transport === "ssh" ? failure.url : failure.sshUrl || "",
  );
  const [key, setKey] = useState<{
    publicKey: string;
    fingerprint: string;
  } | null>(null);
  const [scan, setScan] = useState<{
    scanId: string;
    host: string;
    fingerprints: string[];
    changed?: boolean;
  } | null>(null);
  const [pending, setPending] = useState(false);
  const busy = pending || disabled;
  const [error, setError] = useState("");
  const [copied, setCopied] = useState(false);
  const [trusted, setTrusted] = useState(false);

  async function act(work: () => Promise<void>) {
    setPending(true);
    setError("");
    try {
      await work();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="orch-git-recovery">
      <div className="orch-prep-caption">Repository access over SSH</div>
      <p>
        {failure.transport === "ssh"
          ? "The SSH checkout failed. You can finish repository access here, then try again."
          : "Use the repository’s SSH clone URL if HTTPS is unavailable."}
      </p>
      <label className="orch-git-url">
        SSH clone URL
        <input
          value={url}
          placeholder="git@example.test:team/repo.git"
          disabled={busy}
          spellCheck={false}
          onChange={(event) => {
            setUrl(event.target.value);
            setScan(null);
            setTrusted(false);
          }}
        />
      </label>
      <p>For a custom port, use ssh://git@host:2222/team/repo.git.</p>
      {failure.changed && (
        <p role="alert">
          The Git server’s key changed. Verify the change with its administrator
          before trusting a replacement. Your existing SSH settings are kept;
          this choice applies to sushiAI repository access on this host.
        </p>
      )}
      <>
        <div className="orch-prep-actions">
          <button
            type="button"
            className="ui-button secondary"
            disabled={busy}
            onClick={() =>
              void act(async () => {
                setKey(
                  await window.bridge!.projectHostGitKey(projectId, endpoint),
                );
                setCopied(false);
              })
            }
          >
            {busy ? "Working…" : "Get public SSH key"}
          </button>
          <button
            type="button"
            className="ui-button ghost"
            disabled={busy || !url.trim()}
            onClick={() =>
              void act(async () => {
                setScan(
                  await window.bridge!.projectHostGitScan(
                    projectId,
                    endpoint,
                    url.trim(),
                  ),
                );
                setTrusted(false);
              })
            }
          >
            Check Git server key
          </button>
        </div>
        {key && (
          <div className="orch-git-key">
            <p>
              Add this public key to your GitLab or GitHub account’s SSH keys,
              or as a deploy key with read access to this repository. It belongs
              to the remote host. An existing setup key is reused.
            </p>
            <textarea
              aria-label="Public SSH key"
              value={key.publicKey}
              readOnly
              rows={3}
            />
            <code>{key.fingerprint}</code>
            <button
              type="button"
              className="ui-button secondary"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  setKey(
                    await window.bridge!.projectHostGitCopyKey(
                      projectId,
                      endpoint,
                    ),
                  );
                  setCopied(true);
                })
              }
            >
              {copied ? "Copied public key" : "Copy public key"}
            </button>
          </div>
        )}
        {scan && (
          <div className="orch-git-key">
            <p>
              {`Verify these fingerprints with ${scan.host}’s administrator or published SSH fingerprints, then trust the server.`}
            </p>
            {scan.fingerprints.map((fingerprint) => (
              <code key={fingerprint}>{fingerprint}</code>
            ))}
            <button
              type="button"
              className="ui-button secondary"
              disabled={busy || trusted}
              onClick={() =>
                void act(async () => {
                  await window.bridge!.projectHostGitTrust(
                    projectId,
                    endpoint,
                    scan.scanId,
                  );
                  setTrusted(true);
                })
              }
            >
              {trusted
                ? "Server key trusted"
                : scan.changed
                  ? "Trust verified replacement key"
                  : "Trust verified server key"}
            </button>
          </div>
        )}
      </>
      {error && (
        <p className="orch-git-error" role="alert">
          {error}
        </p>
      )}
      <button
        type="button"
        className="ui-button primary"
        disabled={busy || !url.trim()}
        onClick={() =>
          void act(async () => {
            await window.bridge!.projectHostGitValidate(
              projectId,
              endpoint,
              url.trim(),
            );
            onRetry(url.trim());
          })
        }
      >
        Try again over SSH
      </button>
    </div>
  );
}
