import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Code2,
  ExternalLink,
  File,
  Folder,
  GitBranch,
  Image,
  RefreshCw,
  Search,
  Pencil,
  Save,
} from "lucide-react";
type Entry = { name: string; path: string; directory: boolean; size: number };
type Change = { path: string; status: string; previous?: string };
type Draft = { file: string; text: string; original: string; hash: string };
const drafts = new Map<string, Draft>();
export function ProjectPanel({
  cwd,
  endpoint,
  onHTML,
}: {
  cwd: string;
  endpoint?: string;
  onHTML(root: string, path: string): void;
}) {
  const [root, setRoot] = useState(cwd),
    [draft, setDraft] = useState(cwd),
    [directory, setDirectory] = useState("."),
    [entries, setEntries] = useState<Entry[]>([]),
    [tab, setTab] = useState("files"),
    [hidden, setHidden] = useState(false),
    [filter, setFilter] = useState(""),
    [selected, setSelected] = useState(""),
    [changes, setChanges] = useState<Change[]>([]),
    [branch, setBranch] = useState(""),
    [diffMode, setDiffMode] = useState("working"),
    [text, setText] = useState(""),
    [image, setImage] = useState(""),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const [editable, setEditable] = useState(false),
    [editing, setEditing] = useState(false),
    [draftText, setDraftText] = useState(""),
    [fileHash, setFileHash] = useState(""),
    [saving, setSaving] = useState(false);
  const draftKey = `${endpoint || "local"}:${root}`;
  const dirty = editing && draftText !== text;
  const leaveEditor = () => {
    if (dirty && !window.confirm("Discard the unsaved changes to this file?"))
      return false;
    drafts.delete(draftKey);
    setEditing(false);
    return true;
  };
  const version = useRef(0),
    listVersion = useRef(0);
  const inspect = (operation: string, extra = {}) => {
    if (!window.bridge)
      return Promise.reject(
        new Error("Open the desktop app to browse project files."),
      );
    return window.bridge.projectInspect(endpoint, {
      operation,
      root,
      ...extra,
    });
  };
  async function refresh() {
    const revision = ++listVersion.current;
    setError("");
    try {
      if (tab === "files") {
        const result = await inspect("list", { path: directory, hidden });
        if (revision === listVersion.current) setEntries(result.entries);
      } else {
        const result = await inspect("git");
        if (revision === listVersion.current) {
          setChanges(result.changes);
          setBranch(result.branch);
        }
      }
    } catch (e) {
      if (revision === listVersion.current) setError(String(e));
    }
  }
  useEffect(() => {
    const retained = tab === "files" ? drafts.get(draftKey) : undefined;
    setSelected(retained?.file || "");
    setText(retained?.original || "");
    setDraftText(retained?.text || "");
    setFileHash(retained?.hash || "");
    setEditing(!!retained);
    setEditable(!!retained);
    setImage("");
    version.current++;
    refresh();
    return () => {
      listVersion.current++;
      version.current++;
    };
  }, [root, directory, tab, hidden, endpoint]);
  async function openFile(file: string, mode = diffMode) {
    if (!leaveEditor()) return;
    const revision = ++version.current;
    setSelected(file);
    setImage("");
    setText("");
    setError("");
    setBusy(true);
    setEditable(false);
    try {
      if (
        tab === "git" &&
        changes.find((c) => c.path === file)?.status !== "??"
      ) {
        const result = await inspect("diff", { path: file, mode });
        if (revision === version.current)
          setText(
            result.text || "No changes in this view. Try the other diff mode.",
          );
      } else {
        const data = await inspect("read", { path: file });
        if (revision !== version.current) return;
        if (data.mime.startsWith("image/"))
          setImage(`data:${data.mime};base64,${data.base64}`);
        else {
          const bytes = Uint8Array.from(atob(data.base64), (c) =>
            c.charCodeAt(0),
          );
          if (bytes.includes(0))
            setText(`Binary file · ${data.size.toLocaleString()} bytes`);
          else {
            const content = new TextDecoder().decode(
              bytes.subarray(0, 2 * 1024 * 1024),
            );
            setText(
              tab === "git"
                ? content
                    .split("\n")
                    .map((line) => "+" + line)
                    .join("\n")
                : content +
                    (bytes.length > 2 * 1024 * 1024
                      ? "\n\n[Preview limited to 2 MB]"
                      : ""),
            );
            if (tab === "files" && data.size <= 2 * 1024 * 1024) {
              try {
                const exact = new TextDecoder("utf-8", {
                  fatal: true,
                  ignoreBOM: true,
                }).decode(bytes);
                setText(exact);
                setFileHash(data.hash);
                setEditable(true);
              } catch {
                /* Non-UTF-8 files remain read-only. */
              }
            }
          }
        }
      }
    } catch (e) {
      if (revision === version.current) setError(String(e));
    } finally {
      if (revision === version.current) setBusy(false);
    }
  }
  async function saveFile() {
    if (!dirty || saving) return;
    setSaving(true);
    setError("");
    try {
      const result = await inspect("write", {
        path: selected,
        text: draftText,
        expectedHash: fileHash,
      });
      setText(draftText);
      setFileHash(result.hash);
      drafts.delete(draftKey);
      setEditing(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }
  useEffect(() => {
    if (!editing) return;
    const key = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        saveFile();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [editing, dirty, saving, draftText, fileHash, selected, root, endpoint]);
  return (
    <div className="project-panel">
      <form
        className="project-root"
        onSubmit={(event) => {
          event.preventDefault();
          if (!leaveEditor()) return;
          setRoot(draft);
          setDirectory(".");
        }}
      >
        <Folder size={13} />
        <input
          aria-label="Project root"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
        />
        <button title="Open folder" type="submit">
          <ArrowLeft className="turn-around" size={13} />
        </button>
        <button type="button" aria-label="Refresh project" onClick={refresh}>
          <RefreshCw size={13} />
        </button>
      </form>
      <div className="project-tabs">
        <button
          className={tab === "files" ? "active" : ""}
          onClick={() => {
            if (leaveEditor()) setTab("files");
          }}
        >
          <Folder size={12} /> Files
        </button>
        <button
          className={tab === "git" ? "active" : ""}
          onClick={() => {
            if (leaveEditor()) setTab("git");
          }}
        >
          <GitBranch size={12} /> Git changes
        </button>
        {endpoint?.startsWith("ssh:") && <span>SSH</span>}
      </div>
      <div className="project-body">
        <aside className="file-list">
          <label className="file-filter">
            <Search size={12} />
            <input
              aria-label="Filter files"
              placeholder="Filter files…"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
          </label>
          {tab === "files" ? (
            <>
              <div className="file-breadcrumb">
                <button
                  title="Parent folder"
                  disabled={directory === "."}
                  onClick={() =>
                    setDirectory(
                      directory.split("/").slice(0, -1).join("/") || ".",
                    )
                  }
                >
                  <ArrowLeft size={12} />
                </button>
                <span>
                  {directory === "." ? "Project" : directory.split("/").at(-1)}
                </span>
                <label title="Show hidden files">
                  <input
                    type="checkbox"
                    checked={hidden}
                    onChange={(e) => setHidden(e.target.checked)}
                  />
                </label>
              </div>
              {entries
                .filter((e) =>
                  e.name.toLowerCase().includes(filter.toLowerCase()),
                )
                .map((entry) => (
                  <button
                    className={`file-row ${selected === entry.path ? "active" : ""}`}
                    key={entry.path}
                    title={entry.path}
                    onClick={() =>
                      entry.directory
                        ? leaveEditor() && setDirectory(entry.path)
                        : openFile(entry.path)
                    }
                  >
                    {entry.directory ? (
                      <Folder size={13} />
                    ) : /\.(png|jpe?g|gif|webp|svg)$/i.test(entry.name) ? (
                      <Image size={13} />
                    ) : (
                      <File size={13} />
                    )}
                    <span>{entry.name}</span>
                  </button>
                ))}
            </>
          ) : (
            <>
              <div className="git-branch">
                <GitBranch size={12} />
                {branch || "Repository"}
                <span>{changes.length}</span>
              </div>
              {changes
                .filter((c) =>
                  c.path.toLowerCase().includes(filter.toLowerCase()),
                )
                .map((change) => (
                  <button
                    key={change.path}
                    className={`file-row ${selected === change.path ? "active" : ""}`}
                    title={
                      change.previous
                        ? `${change.previous} → ${change.path}`
                        : change.path
                    }
                    onClick={() => openFile(change.path)}
                  >
                    <code
                      className={`git-status status-${change.status.trim()[0]}`}
                    >
                      {change.status}
                    </code>
                    <span>{change.path}</span>
                  </button>
                ))}
              {!changes.length && !error && (
                <p className="muted clean-tree">Working tree is clean.</p>
              )}
            </>
          )}
        </aside>
        <div className="file-preview">
          <div className="file-preview-toolbar">
            <span title={selected}>{selected || "Select a file"}</span>
            {editable && tab === "files" && !editing && (
              <button
                title="Edit text file"
                onClick={() => {
                  setDraftText(text);
                  setEditing(true);
                }}
              >
                <Pencil size={12} /> Edit
              </button>
            )}
            {editing && (
              <>
                <small>{dirty ? "Unsaved" : "Editing"}</small>
                <button disabled={saving} onClick={leaveEditor}>
                  Cancel
                </button>
                <button
                  title="Save file · ⌘S"
                  disabled={!dirty || saving}
                  onClick={saveFile}
                >
                  <Save size={12} /> {saving ? "Saving…" : "Save"}
                </button>
              </>
            )}
            {tab === "git" && (
              <select
                aria-label="Diff mode"
                value={diffMode}
                onChange={(e) => {
                  setDiffMode(e.target.value);
                  if (selected) openFile(selected, e.target.value);
                }}
              >
                <option value="working">Working tree</option>
                <option value="staged">Staged</option>
              </select>
            )}
            {/\.(html?|pdf)$/i.test(selected) && (
              <button
                title="Open file in browser"
                onClick={() => onHTML(root, selected)}
              >
                <ExternalLink size={13} /> Open in browser
              </button>
            )}
          </div>
          {error && (
            <div className="inline-error" role="alert">
              {error}
            </div>
          )}
          {busy ? (
            <div className="file-empty">Loading…</div>
          ) : editing ? (
            <textarea
              className="file-editor"
              aria-label={`Edit ${selected}`}
              spellCheck={false}
              value={draftText}
              onChange={(e) => {
                setDraftText(e.target.value);
                drafts.set(draftKey, {
                  file: selected,
                  text: e.target.value,
                  original: text,
                  hash: fileHash,
                });
              }}
            />
          ) : image ? (
            <div className="image-preview">
              <img src={image} alt={selected} />
            </div>
          ) : text ? (
            <pre className={`file-code ${tab === "git" ? "diff-code" : ""}`}>
              {text.split("\n").map((line, i) => (
                <div
                  key={i}
                  className={
                    tab === "git"
                      ? line.startsWith("+")
                        ? "diff-add"
                        : line.startsWith("-")
                          ? "diff-remove"
                          : line.startsWith("@@")
                            ? "diff-hunk"
                            : ""
                      : ""
                  }
                >
                  <span className="line-number">{i + 1}</span>
                  <code>{line || " "}</code>
                </div>
              ))}
            </pre>
          ) : (
            <div className="file-empty">
              <Code2 size={27} />
              <p>Explore your project.</p>
              <small>
                Code, images, HTML and Git changes — here or over SSH.
              </small>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
