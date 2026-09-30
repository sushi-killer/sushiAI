import { useEffect, useState } from "react";
import { Check, RefreshCw, X } from "lucide-react";
import {
  preflightItems,
  routesFallbackNote,
  setupSteps,
  type SetupSeen,
  type SetupStep,
} from "./hosts";
import type { OrchestratorHost, Preflight } from "./types";

function PreflightRow({ preflight }: { preflight: Preflight }) {
  return (
    <>
      {preflightItems(preflight).map((item) => (
        <span
          key={item.name}
          className={`orch-preflight-item${item.ok ? " ok" : " missing"}`}
        >
          {item.ok ? (
            <Check size={12} aria-hidden />
          ) : (
            <X size={12} aria-hidden />
          )}
          <code>{item.name}</code>
          {item.note && (
            <span className="orch-preflight-note">{item.note}</span>
          )}
        </span>
      ))}
    </>
  );
}

/** Home's one-line preflight strip on a remote host (Figma "Home · remote
 * host"): git, claude and codex, and which routes that turns off there. */
export function PreflightStrip({
  hostName,
  preflight,
}: {
  hostName: string;
  preflight: Preflight;
}) {
  const off = routesFallbackNote(preflight, hostName);
  return (
    <div className="orch-preflight-strip" aria-label={`On ${hostName}`}>
      <PreflightRow preflight={preflight} />
      <span className="orch-preflight-spacer" />
      {off && <span className="orch-preflight-off">{off}</span>}
    </div>
  );
}

function StepMark({ state }: { state: SetupStep["state"] }) {
  return (
    <span className={`orch-setup-mark ${state}`} aria-hidden>
      {state === "done" && <Check size={10} strokeWidth={3} />}
      {state === "failed" && <X size={10} strokeWidth={2.5} />}
    </span>
  );
}

function CopyCommand({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="orch-setup-command">
      <code>{command}</code>
      <button
        type="button"
        className="orch-setup-copy"
        onClick={() => {
          void navigator.clipboard?.writeText(command).then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

/** The time since the host entered its current state, ticking each second
 * while it is being set up. */
function useElapsed(since: number, ticking: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);
  return Math.max(0, now - since);
}

/** Figma "Home · remote setup": the five steps that bring orchd up on an
 * SSH host, what the host offers, and Try again / Cancel. */
export function RemoteSetup({
  host,
  seen,
  address,
  onRetry,
  onCancel,
}: {
  host: OrchestratorHost;
  /** States this panel saw the host pass through, and when the last began. */
  seen: { states: OrchestratorHost["state"][]; since: number };
  address?: string;
  onRetry(): void;
  onCancel(): void;
}) {
  const failed = host.state === "error";
  const elapsedMs = useElapsed(seen.since, !failed);
  const setup: SetupSeen = { states: seen.states, elapsedMs, address };
  const steps = setupSteps(host, setup);
  const preflight = host.preflight;
  const off = preflight ? routesFallbackNote(preflight, host.name) : "";
  return (
    <div className="orch-view-scroll orch-setup">
      <div className="orch-setup-head">
        <h2>Set up the orchestrator on {host.name}</h2>
        <p>
          orchd runs on the host where the repo lives, so tasks keep going when
          you quit sushiAI or your Mac sleeps. Nothing new is opened: it talks
          over your SSH connection.
        </p>
      </div>
      <ol className="orch-setup-steps" aria-label="Setup steps">
        {steps.map((step) => (
          <li
            key={step.title}
            className={`orch-setup-step ${step.state}`}
            aria-current={step.state === "active" ? "step" : undefined}
          >
            <StepMark state={step.state} />
            <div className="orch-setup-text">
              <span className="orch-setup-title">{step.title}</span>
              <span className="orch-setup-detail">{step.detail}</span>
              {step.command && <CopyCommand command={step.command} />}
            </div>
          </li>
        ))}
      </ol>
      {preflight && (
        <div className="orch-setup-preflight">
          <span className="orch-eyebrow">ON {host.name.toUpperCase()}</span>
          <div className="orch-setup-preflight-row">
            <PreflightRow preflight={preflight} />
          </div>
          {off && <p className="orch-setup-preflight-note">{off}</p>}
        </div>
      )}
      <div className="orch-setup-actions">
        {failed && (
          <button type="button" className="ui-button primary" onClick={onRetry}>
            <RefreshCw size={14} aria-hidden /> Try again
          </button>
        )}
        <button type="button" className="ui-button ghost" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/** A remote host with no repo picked yet: ask for its path. */
export function RepoPrompt({
  hostName,
  suggestions,
  onRepo,
}: {
  hostName: string;
  suggestions: string[];
  onRepo(repo: string): void;
}) {
  const [draft, setDraft] = useState(suggestions[0] ?? "");
  const commit = () => draft.trim() && onRepo(draft.trim());
  return (
    <div className="orch-view-scroll orch-setup">
      <div className="orch-setup-head">
        <h2>Which repo on {hostName}?</h2>
        <p>Tasks run in a repo on the host. Enter its path there.</p>
      </div>
      <form
        className="orch-repo-prompt"
        onSubmit={(event) => {
          event.preventDefault();
          commit();
        }}
      >
        <input
          aria-label={`Repo path on ${hostName}`}
          list="orch-repo-prompt-suggestions"
          placeholder="/path/to/repo"
          value={draft}
          autoFocus
          onChange={(event) => setDraft(event.target.value)}
        />
        <datalist id="orch-repo-prompt-suggestions">
          {suggestions.map((path) => (
            <option key={path} value={path} />
          ))}
        </datalist>
        <button
          type="submit"
          className="ui-button primary"
          disabled={!draft.trim()}
        >
          Open
        </button>
      </form>
    </div>
  );
}
