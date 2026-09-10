import { useEffect, useMemo, useRef, useState, type UIEvent } from "react";
import {
  AlertCircle,
  CheckCircle2,
  FileDiff,
  LoaderCircle,
  Maximize2,
  Minimize2,
  Power,
  PowerOff,
  Save,
  Trash2,
  X,
} from "lucide-react";
import { highlightCode } from "./syntax";
import type { SkillCatalogItem, SkillManagementAction } from "./types";

type Target = { root: string; path: string };
type Validation = { errors: string[]; warnings: string[] };

function resolveTarget(file: string, home?: string): Target | null {
  if (!file) return null;
  if (file.startsWith("~/"))
    return home ? { root: home, path: file.slice(2) } : null;
  if (file.startsWith("/")) {
    const slash = file.lastIndexOf("/");
    return slash > 0
      ? { root: file.slice(0, slash), path: file.slice(slash + 1) }
      : null;
  }
  return home ? { root: home, path: file } : null;
}

function decodeText(base64: string) {
  const bytes = Uint8Array.from(atob(base64), (character) =>
    character.charCodeAt(0),
  );
  if (bytes.includes(0)) throw new Error("This skill is a binary file.");
  return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
    bytes,
  );
}

function validateSkill(source: string): Validation {
  const errors: string[] = [];
  const warnings: string[] = [];
  const lines = source.split(/\r?\n/);
  const fences = lines.filter((line) => /^\s*```/.test(line)).length;
  if (fences % 2) errors.push("A Markdown code block is not closed.");

  if (source.trim().length === 0) errors.push("The skill file is empty.");

  if (lines[0]?.trim() === "---") {
    const closing = lines.findIndex(
      (line, index) => index > 0 && line.trim() === "---",
    );
    if (closing < 0) errors.push("Skill frontmatter is not closed with ---.");
    else {
      const frontmatter = lines.slice(1, closing);
      const keys = frontmatter
        .map((line) => line.match(/^([A-Za-z][\w-]*):\s*/)?.[1])
        .filter((key): key is string => Boolean(key));
      const duplicateKeys = keys.filter(
        (key, index) => keys.indexOf(key) !== index,
      );
      if (duplicateKeys.length)
        warnings.push(`Duplicate frontmatter key: ${duplicateKeys[0]}.`);
    }
  }

  return { errors, warnings };
}

function diffStats(before: string, after: string) {
  const oldLines = before.split(/\r?\n/);
  const newLines = after.split(/\r?\n/);
  let prefix = 0;
  while (
    prefix < oldLines.length &&
    prefix < newLines.length &&
    oldLines[prefix] === newLines[prefix]
  )
    prefix += 1;
  let suffix = 0;
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - 1 - suffix] ===
      newLines[newLines.length - 1 - suffix]
  )
    suffix += 1;
  return {
    added: Math.max(0, newLines.length - prefix - suffix),
    removed: Math.max(0, oldLines.length - prefix - suffix),
  };
}

function relativeDate(value?: number) {
  if (!value) return "Never recorded";
  const age = Math.max(0, Date.now() - value);
  if (age < 60 * 1000) return "just now";
  if (age < 24 * 60 * 60 * 1000)
    return `${Math.max(1, Math.floor(age / (60 * 60 * 1000)))}h ago`;
  if (age < 30 * 24 * 60 * 60 * 1000)
    return `${Math.floor(age / (24 * 60 * 60 * 1000))}d ago`;
  return new Date(value).toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
  });
}

function HighlightedSkillEditor({
  value,
  name,
  onChange,
}: {
  value: string;
  name: string;
  onChange(value: string): void;
}) {
  const codeRef = useRef<HTMLPreElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const lines = useMemo(() => highlightCode(value, "SKILL.md"), [value]);

  useEffect(() => {
    inputRef.current?.scrollTo({ top: 0, left: 0 });
    codeRef.current?.scrollTo({ top: 0, left: 0 });
  }, [name]);

  function syncScroll(event: UIEvent<HTMLTextAreaElement>) {
    const code = codeRef.current;
    if (!code) return;
    code.scrollTop = event.currentTarget.scrollTop;
    code.scrollLeft = event.currentTarget.scrollLeft;
  }

  return (
    <div className="skill-editor-code-shell">
      <pre ref={codeRef} className="skill-editor-code" aria-hidden="true">
        {lines.map((line, lineIndex) => (
          <div className="skill-editor-code-line" key={lineIndex}>
            <span className="skill-editor-line-number">{lineIndex + 1}</span>
            <code>
              {line.tokens.map((token, tokenIndex) => (
                <span
                  className={token.kind ? `syntax-${token.kind}` : undefined}
                  key={`${lineIndex}-${tokenIndex}`}
                >
                  {token.text}
                </span>
              ))}
            </code>
          </div>
        ))}
      </pre>
      <textarea
        ref={inputRef}
        className="skill-editor-input"
        aria-label={`Edit ${name}`}
        spellCheck={false}
        value={value}
        onScroll={syncScroll}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

export function SkillEditorPanel({
  item,
  home,
  onClose,
  onManage,
  pluginSkillCount = 1,
}: {
  item: SkillCatalogItem;
  home?: string;
  onClose(): void;
  onManage(action: SkillManagementAction): Promise<void>;
  pluginSkillCount?: number;
}) {
  const [resolvedHome, setResolvedHome] = useState(home);
  const target = useMemo(
    () => (item.path ? resolveTarget(item.path, resolvedHome) : null),
    [item.path, resolvedHome],
  );
  const [source, setSource] = useState("");
  const [draft, setDraft] = useState("");
  const [hash, setHash] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  const [fullscreen, setFullscreen] = useState(false);
  const [managing, setManaging] = useState<SkillManagementAction | null>(null);
  const validation = useMemo(() => validateSkill(draft), [draft]);
  const stats = useMemo(() => diffStats(source, draft), [source, draft]);
  const dirty = draft !== source;
  const isPlugin = item.source === "Plugin";
  const isLocal = item.source === "Local";
  const isSupportedProvider = ["Codex", "Claude", "Agent"].includes(
    item.provider || "",
  );
  const pluginAction = isPlugin && item.disabledBy !== "skill-config";
  const systemProtected =
    isLocal && Boolean(item.path?.split("/").includes(".system"));
  const canManage = isSupportedProvider && (isLocal || isPlugin);
  const canDelete = canManage && (isPlugin || !systemProtected);

  useEffect(() => {
    let cancelled = false;
    setResolvedHome(home);
    setSource("");
    setDraft("");
    setHash("");
    setSaved(false);
    setError("");
    setLoading(true);
    async function load() {
      let nextTarget = item.path ? resolveTarget(item.path, home) : null;
      if (!nextTarget && item.path?.startsWith("~/") && window.bridge) {
        try {
          const system = await window.bridge.system();
          if (cancelled) return;
          setResolvedHome(system.home);
          nextTarget = resolveTarget(item.path, system.home);
        } catch (cause) {
          if (!cancelled)
            setError(cause instanceof Error ? cause.message : String(cause));
        }
      }
      if (!nextTarget || !window.bridge) {
        if (!cancelled) {
          setError("Cannot resolve the local skill path.");
          setLoading(false);
        }
        return;
      }
      try {
        const data = await window.bridge.projectInspect(undefined, {
          operation: "read",
          root: nextTarget.root,
          path: nextTarget.path,
        });
        const text = decodeText(data.base64);
        if (!cancelled) {
          setSource(text);
          setDraft(text);
          setHash(data.hash);
        }
      } catch (cause) {
        if (!cancelled)
          setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    void load();
    return () => {
      cancelled = true;
    };
  }, [home, item.path]);

  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (fullscreen) setFullscreen(false);
        else requestClose();
      }
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [onClose, draft, source, hash, saving, target, fullscreen]);

  function requestClose() {
    if (dirty && !window.confirm("Discard the unsaved skill changes?")) return;
    onClose();
  }

  async function manage(action: SkillManagementAction) {
    if (!canManage || managing) return;
    if (dirty && !window.confirm("Discard the unsaved skill changes first?"))
      return;
    if (action === "delete") {
      const target = isPlugin
        ? `${item.plugin || "this plugin"} and its ${pluginSkillCount} cataloged skill${pluginSkillCount === 1 ? "" : "s"}`
        : `the local skill “${item.name}”`;
      if (
        !window.confirm(
          `Move ${target} to the Trash? This changes the provider configuration and can affect future sessions.`,
        )
      )
        return;
    }
    setManaging(action);
    setError("");
    setSaved(false);
    try {
      await onManage(action);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setManaging(null);
    }
  }

  async function save() {
    if (!target || !dirty || saving || validation.errors.length) return;
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      const result = await window.bridge?.projectInspect(undefined, {
        operation: "write",
        root: target.root,
        path: target.path,
        text: draft,
        expectedHash: hash,
      });
      if (!result) throw new Error("Desktop bridge is unavailable.");
      setSource(draft);
      setHash(result.hash);
      setSaved(true);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  return (
    <aside
      className={`skill-editor-panel${fullscreen ? " fullscreen" : ""}`}
      role="region"
      aria-label={`Edit ${item.name}`}
    >
      <div
        className="skill-editor-tab-strip"
        role="tablist"
        aria-label="Skill editor tabs"
      >
        <span
          className="skill-editor-tab active"
          role="tab"
          aria-selected="true"
        >
          <FileDiff size={12} /> SKILL.md
        </span>
        <div className="skill-editor-tab-actions">
          <button
            className="skill-editor-tab-action"
            aria-pressed={fullscreen}
            aria-label={
              fullscreen ? "Exit fullscreen editor" : "Fullscreen editor"
            }
            title={fullscreen ? "Exit fullscreen" : "Fullscreen editor"}
            onClick={() => setFullscreen((value) => !value)}
          >
            {fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
          </button>
          <button
            className="skill-editor-tab-action"
            aria-label="Close skill editor"
            title="Close"
            onClick={requestClose}
          >
            <X size={15} />
          </button>
        </div>
      </div>
      <header className="skill-editor-header">
        <div>
          <span className="skill-editor-eyebrow">Skill editor</span>
          <h2 title={item.name}>{item.name}</h2>
          <p title={item.path}>{item.path || "Local file"}</p>
          {pluginAction && (
            <span className="skill-editor-scope">
              Plugin action applies to {pluginSkillCount} cataloged skill
              {pluginSkillCount === 1 ? "" : "s"}
            </span>
          )}
        </div>
      </header>
      <div className="skill-editor-meta">
        <span>
          <strong>Agent</strong>
          {item.provider || "Other"} · {item.source || "Local"}
        </span>
        <span>
          <strong>Last used</strong>
          {relativeDate(item.lastUsedAt)}
        </span>
        <span>
          <strong>Calls</strong>
          {item.usageCount ? item.usageCount.toLocaleString() : "Not available"}
        </span>
        <span>
          <strong>Updated</strong>
          {relativeDate(item.updatedAt)}
        </span>
      </div>
      <div className="skill-editor-toolbar">
        <span className="skill-editor-filetype">Markdown · SKILL.md</span>
        <span className="skill-editor-diff" aria-label="Draft changes">
          <b>+{stats.added}</b>
          <i>−{stats.removed}</i>
          <span>lines</span>
        </span>
        {canManage ? (
          <div
            className="skill-editor-management"
            aria-label="Skill management"
          >
            <button
              className="skill-editor-manage"
              disabled={loading || Boolean(managing)}
              title={
                item.availability === "disabled"
                  ? pluginAction
                    ? "Enable plugin"
                    : "Enable skill"
                  : pluginAction
                    ? "Disable plugin"
                    : "Disable skill"
              }
              onClick={() =>
                void manage(
                  item.availability === "disabled" ? "enable" : "disable",
                )
              }
            >
              {item.availability === "disabled" ? (
                <Power size={13} />
              ) : (
                <PowerOff size={13} />
              )}
              {managing === "enable"
                ? "Enabling…"
                : managing === "disable"
                  ? "Disabling…"
                  : item.availability === "disabled"
                    ? pluginAction
                      ? "Enable plugin"
                      : "Enable"
                    : pluginAction
                      ? "Disable plugin"
                      : "Disable"}
            </button>
            {canDelete && (
              <button
                className="skill-editor-manage danger"
                disabled={loading || Boolean(managing)}
                title={isPlugin ? "Uninstall plugin" : "Delete skill"}
                onClick={() => void manage("delete")}
              >
                {managing === "delete" ? (
                  <LoaderCircle className="spin" size={13} />
                ) : (
                  <Trash2 size={13} />
                )}
                {isPlugin ? "Uninstall" : "Delete"}
              </button>
            )}
          </div>
        ) : item.availability === "external" ? (
          <span className="skill-editor-readonly">
            Managed by external harness
          </span>
        ) : null}
        <button
          className="skill-editor-save"
          disabled={loading || saving || !dirty || validation.errors.length > 0}
          onClick={() => void save()}
        >
          {saving ? (
            <LoaderCircle className="spin" size={13} />
          ) : (
            <Save size={13} />
          )}
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
      <div className="skill-editor-status" aria-live="polite">
        {loading ? (
          <span className="skill-editor-muted">
            <LoaderCircle className="spin" size={13} /> Reading local file…
          </span>
        ) : error ? (
          <span className="skill-editor-error">
            <AlertCircle size={13} /> {error}
          </span>
        ) : validation.errors.length ? (
          <span className="skill-editor-error">
            <AlertCircle size={13} /> {validation.errors[0]}
          </span>
        ) : validation.warnings.length ? (
          <span className="skill-editor-warning">
            <AlertCircle size={13} /> {validation.warnings[0]}
          </span>
        ) : (
          <span className="skill-editor-ok">
            <CheckCircle2 size={13} /> Syntax OK
            {dirty ? " · unsaved changes" : ""}
          </span>
        )}
        {saved && <span className="skill-editor-saved">Saved locally</span>}
      </div>
      {loading ? (
        <div className="skill-editor-loading" />
      ) : error && !source ? (
        <div className="skill-editor-empty">The skill could not be opened.</div>
      ) : (
        <HighlightedSkillEditor
          name={item.name}
          value={draft}
          onChange={(value) => {
            setDraft(value);
            setSaved(false);
          }}
        />
      )}
    </aside>
  );
}
