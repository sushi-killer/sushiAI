import {
  Check,
  ChevronDown,
  Copy,
  ExternalLink,
  MessageSquare,
  Pencil,
  Play,
} from "lucide-react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { errorText } from "../../app/errors.ts";
import { orchestratorClientFor } from "../../orchestrator/client.ts";
import { useOrchestratorEnabled } from "../../orchestrator/enabled.ts";
import { Tag } from "../../orchestrator/ui/index.ts";
import type { CoreViewProps } from "../coreViews.ts";
import {
  acceptAnnotation,
  agoText,
  clampBoxX,
  commentMessage,
  editedMessage,
  goalPrompt,
  insideProject,
  paneRoot,
  agentEnded,
  parseComments,
  kindOf,
  parseFrontmatter,
  parseRecent,
  parseStarts,
  pasteOf,
  pushRecent,
  shownPath,
  splitPath,
  startOrchestratorTask,
  withComments,
  type Comment,
  type StartRecord,
} from "./artifact.ts";
import {
  CommentBox,
  useAnchorHighlight,
  CommentButton,
  CommentTray,
  useDocSelection,
  type Selection,
} from "./Comments.tsx";
import { MarkdownDoc } from "./MarkdownDoc.tsx";
import {
  hostOfEndpoint,
  RunStatus,
  StartTaskCard,
  useTaskStatus,
  VerifiedTag,
  type Runner,
} from "./StartTask.tsx";
import { usePreviewFile } from "./usePreviewFile.ts";

/** "updated N s ago", ticking on its own so the document is not redrawn. */
function Updated({ since }: { since: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return <span className="pv-updated">updated {agoText(now - since)}</span>;
}

/** The `artifacts.preview` core view: one file an agent wrote, drawn in
 * the companion half of the agent's pane, with comments and Start task going back to it. */
export function PreviewView(props: CoreViewProps) {
  const { args, connection, cwd, paneCwd, headerSlot, onArgs } = props;
  const asked = args.arg || "";
  // Only a file inside the project or the pane's own folder is read or served.
  const path = insideProject(asked, cwd, paneRoot(cwd, paneCwd)) || "";
  const outside = asked && !path ? asked : "";
  const kind = kindOf(asked);
  const pane = useRef<HTMLDivElement>(null);
  const file = usePreviewFile(path, kind, connection, pane);
  const argsRef = useRef(args);
  argsRef.current = args;

  // Recent files of this pane, newest first, kept in the pane's args.
  const recent = parseRecent(args.recent);
  useEffect(() => {
    if (path && recent[0] !== path)
      onArgs({ recent: JSON.stringify(pushRecent(recent, path)) });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only a new path records itself
  }, [path]);

  const parsed = useMemo(
    () =>
      file.state === "ready" && kind === "markdown"
        ? parseFrontmatter(file.text)
        : { meta: {} as Record<string, string>, body: "" },
    [file, kind],
  );
  const isPlan = kind === "markdown" && parsed.meta.kind === "plan";

  const [previewUrl, setPreviewUrl] = useState("");
  useEffect(() => {
    setPreviewUrl("");
    if (!path || (kind !== "html" && kind !== "pdf") || !window.bridge) return;
    let disposed = false;
    const { dir, base } = splitPath(path);
    window.bridge
      .projectPreview(
        connection,
        dir,
        base,
        kind === "html" ? { annotate: true } : undefined,
      )
      .then((url) => !disposed && setPreviewUrl(url))
      .catch(() => {});
    return () => {
      disposed = true;
    };
  }, [path, kind, connection]);

  // Keep the reading position when the file changes under the reader.
  const scroller = useRef<HTMLDivElement>(null);
  const top = useRef(0);
  const hash = file.state === "ready" ? file.hash : "";
  useLayoutEffect(() => {
    if (scroller.current) scroller.current.scrollTop = top.current;
  }, [hash]);

  const wrap = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useDocSelection(wrap);
  const [boxFor, setBoxFor] = useState<Selection | null>(null);
  useAnchorHighlight(boxFor?.range);
  // Unsent comments are kept per file in the companion's args, so hiding the
  // Preview keeps them; the tray shows the current file's.
  const comments = parseComments(args.comments)[path] || [];
  const setComments = (list: Comment[]) =>
    onArgs({
      comments: JSON.stringify(
        withComments(parseComments(argsRef.current.comments), path, list),
      ),
    });
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState("");
  useEffect(() => {
    setBoxFor(null);
    setSendError("");
  }, [path]);
  const [commentMode, setCommentMode] = useState(false);
  useEffect(() => setCommentMode(false), [path]);
  const frame = useRef<HTMLIFrameElement>(null);
  const pickMode = () =>
    frame.current?.contentWindow?.postMessage(
      { sushiai: "mode", pick: commentMode },
      "*",
    );
  useEffect(pickMode, [commentMode, previewUrl, hash]);
  // What the annotate script in an HTML page tells us. Every field is
  // untrusted: clipped, and only ever shown as text.
  useEffect(() => {
    if (kind !== "html") return;
    const onMessage = (event: MessageEvent) => {
      const note = acceptAnnotation(
        event.data,
        !!frame.current && event.source === frame.current.contentWindow,
      );
      if (!note || !frame.current) return;
      if (note.kind === "cancel") return setBoxFor(null);
      const area = wrap.current?.getBoundingClientRect();
      const inner = frame.current.getBoundingClientRect();
      setBoxFor({
        quote: note.quote,
        where: note.where,
        x: Math.max(
          0,
          Math.min(
            inner.left - (area?.left || 0) + note.rect.x,
            (area?.width || 400) - 310,
          ),
        ),
        y: Math.max(
          0,
          inner.top - (area?.top || 0) + note.rect.y + note.rect.h + 6,
        ),
      });
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [kind]);
  const pinImage = (event: React.MouseEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const area = wrap.current?.getBoundingClientRect();
    const px = Math.round(((event.clientX - box.left) / box.width) * 100);
    const py = Math.round(((event.clientY - box.top) / box.height) * 100);
    setBoxFor({
      quote: "",
      where: `point at ${px}%, ${py}%`,
      pin: { x: px, y: py },
      x: Math.max(
        0,
        Math.min(event.clientX - (area?.left || 0), (area?.width || 400) - 310),
      ),
      y: event.clientY - (area?.top || 0) + 10,
    });
  };
  // One bracketed paste keeps the newlines inside a single message; the
  // Enter that submits it is a second, separate input. A closed agent pane
  // means nothing is sent.
  const sendToAgent = async (message: string) => {
    if (!window.bridge || !props.agentHerdrPaneId || !props.herdrEndpoint)
      return false;
    const target = { pane_id: props.agentHerdrPaneId };
    await window.bridge.herdr(props.herdrEndpoint, "pane.send_input", {
      ...target,
      raw: pasteOf(message),
    });
    // The agent TUI needs a moment to take the paste before Enter.
    await new Promise((resolve) => window.setTimeout(resolve, 150));
    await window.bridge.herdr(props.herdrEndpoint, "pane.send_input", {
      ...target,
      raw: "\r",
    });
    return true;
  };
  const send = async (items: Comment[]) => {
    const sentPath = path;
    setSending(true);
    setSendError("");
    try {
      if (!(await sendToAgent(commentMessage(path, items)))) return;
      onArgs({
        comments: JSON.stringify(
          withComments(parseComments(argsRef.current.comments), sentPath, []),
        ),
      });
    } catch (error) {
      setSendError(errorText(error));
    } finally {
      setSending(false);
    }
  };

  // Edit mode (Markdown): the raw text in a textarea, saved with the hash it
  // was read with, so a change made meanwhile is refused instead of lost.
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [editError, setEditError] = useState("");
  const editHash = useRef("");
  useEffect(() => {
    setEditing(false);
    setEditError("");
  }, [path]);
  const startEdit = () => {
    if (file.state !== "ready") return;
    editHash.current = file.hash;
    setDraft(file.text);
    setEditError("");
    setCard(false);
    setBoxFor(null);
    setSelection(null);
    setEditing(true);
  };
  const saveEdit = async () => {
    if (!window.bridge || saving) return;
    setSaving(true);
    setEditError("");
    try {
      const { dir, base } = splitPath(path);
      await window.bridge.projectInspect(connection, {
        operation: "write",
        root: dir,
        path: base,
        text: draft,
        expectedHash: editHash.current,
      });
      setEditing(false);
      try {
        await sendToAgent(editedMessage(path));
      } catch (error) {
        setSendError(errorText(error));
      }
    } catch (error) {
      setEditError(errorText(error));
    } finally {
      setSaving(false);
    }
  };

  const orchestratorOn = useOrchestratorEnabled();
  const [card, setCard] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState("");
  const [failedPath, setFailedPath] = useState("");
  const record: StartRecord | undefined = parseStarts(args.starts)[path];
  const taskStatus = useTaskStatus(record);
  const ended = agentEnded(record, props.runs?.branches, Date.now());
  const failed = failedPath === path;
  const start = async (runner: Runner, branch: string, base: string) => {
    setCard(false);
    setStarting(true);
    setStartError("");
    setFailedPath("");
    try {
      let next: StartRecord;
      if (runner === "orchestrator") {
        const host = hostOfEndpoint(connection);
        const taskId = await startOrchestratorTask(
          orchestratorClientFor(host),
          cwd,
          parsed.body,
          path,
        );
        next = { kind: "orchestrator", host, repo: cwd, taskId };
      } else {
        if (!props.launchAgent)
          throw new Error("Starting an agent is not available here.");
        const ok = await props.launchAgent({
          agent: runner,
          branch,
          base: `refs/heads/${base}`,
          prompt: goalPrompt(parsed.body, path),
        });
        if (!ok) {
          setFailedPath(path);
          return;
        }
        next = { kind: "agent", agent: runner, branch, at: Date.now() };
      }
      onArgs({
        starts: JSON.stringify({
          ...parseStarts(argsRef.current.starts),
          [path]: next,
        }),
      });
    } catch (error) {
      setStartError(errorText(error));
      setFailedPath(path);
    } finally {
      setStarting(false);
    }
  };

  const present = async () => {
    if (previewUrl) await window.bridge?.agentOpenExternal(previewUrl);
  };

  const [menu, setMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (event: Event) => {
      if (
        event instanceof KeyboardEvent
          ? event.key === "Escape"
          : !menuRef.current?.contains(event.target as Node)
      )
        setMenu(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", close);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", close);
    };
  }, [menu]);

  const [copied, setCopied] = useState(false);
  // Nothing started yet: the tag row offers Start task; otherwise it shows the run.
  const idle = !starting && !record && !failed;
  const agentLabel = props.agentLabel || "agent";
  const noAgent = props.agentHerdrPaneId
    ? ""
    : "The agent pane is closed; there is nowhere to send this.";
  const title = parsed.meta.title || "";

  // The controls live in the companion's header row, beside its close button.
  const bar = (
    <>
      <div className="pv-select" ref={menuRef}>
        <button
          type="button"
          className="pv-file"
          aria-haspopup="menu"
          aria-expanded={menu}
          title={path}
          disabled={!path}
          onClick={() => setMenu(!menu)}
        >
          <span>{path ? shownPath(path, cwd) : "No file"}</span>
          <ChevronDown size={12} aria-hidden />
        </button>
        {menu && (
          <div className="pv-menu" role="menu">
            {recent.map((item) => (
              <button
                key={item}
                type="button"
                role="menuitemradio"
                aria-checked={item === path}
                title={item}
                onClick={() => {
                  setMenu(false);
                  if (item !== path) onArgs({ arg: item });
                }}
              >
                {shownPath(item, cwd)}
              </button>
            ))}
          </div>
        )}
      </div>
      {file.state === "ready" && <Updated since={file.changedAt} />}
      <span className="pv-spacer" />
      {(kind === "html" ||
        kind === "image" ||
        kind === "svg" ||
        kind === "pdf") &&
        file.state === "ready" && (
          <button
            type="button"
            className="pv-icon pv-toggle"
            aria-label="Comment"
            title="Comment"
            aria-pressed={kind === "pdf" ? undefined : commentMode}
            onClick={() =>
              kind === "pdf"
                ? // Nothing to point at in a PDF: the box opens without a quote.
                  setBoxFor({ quote: "", where: "PDF", x: 12, y: 12 })
                : setCommentMode(!commentMode)
            }
          >
            <MessageSquare aria-hidden />
          </button>
        )}
      {kind === "html" && previewUrl && (
        <button
          type="button"
          className="pv-icon pv-present"
          aria-label="Present"
          title="Present"
          onClick={() => void present()}
        >
          <ExternalLink aria-hidden />
        </button>
      )}
    </>
  );

  return (
    <div className="pv" ref={pane}>
      {headerSlot ? createPortal(bar, headerSlot) : null}
      {(startError || editError) && (
        <div className="pd-alert pv-alert" role="alert">
          {editError || startError}
        </div>
      )}
      <div className="pv-wrap" ref={wrap}>
        {outside ? (
          <div className="pd-alert pv-alert" role="alert">
            Outside this project: {outside}
          </div>
        ) : !path ? (
          <div className="pv-empty">No file is open.</div>
        ) : file.state === "loading" ? (
          <div className="loading">Loading…</div>
        ) : file.state === "error" ? (
          <div className="pd-alert pv-alert" role="alert">
            {file.message}
          </div>
        ) : kind === "markdown" ? (
          <div
            className={editing ? "pv-doc editing" : "pv-doc"}
            ref={scroller}
            onScroll={(event) => {
              top.current = event.currentTarget.scrollTop;
              setSelection(null);
              setBoxFor(null);
            }}
          >
            <article className="pv-md">
              <div className="pv-head">
                {parsed.meta.kind && (
                  <Tag tone="neutral" dot={false}>
                    {parsed.meta.kind.toUpperCase()}
                  </Tag>
                )}
                {isPlan && <VerifiedTag meta={parsed.meta} />}
                <span className="pv-spacer" />
                {editing ? (
                  <>
                    <span className="pv-hint">⌘S save · Esc cancel</span>
                    <button
                      type="button"
                      className="ui-button ghost pv-head-btn"
                      onClick={() => setEditing(false)}
                    >
                      Cancel
                    </button>
                    <button
                      type="button"
                      className="ui-button primary pv-head-btn"
                      disabled={saving}
                      onClick={() => void saveEdit()}
                    >
                      Save
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      type="button"
                      className="pv-icon"
                      aria-label="Copy text"
                      title="Copy text"
                      onClick={() =>
                        void navigator.clipboard
                          ?.writeText(file.text)
                          .then(() => {
                            setCopied(true);
                            window.setTimeout(() => setCopied(false), 1500);
                          })
                      }
                    >
                      {copied ? <Check aria-hidden /> : <Copy aria-hidden />}
                    </button>
                    <button
                      type="button"
                      className="pv-icon"
                      aria-label="Edit"
                      title="Edit"
                      onClick={startEdit}
                    >
                      <Pencil aria-hidden />
                    </button>
                    {isPlan &&
                      (idle ? (
                        <button
                          type="button"
                          className="pv-start-btn"
                          onClick={() => setCard(true)}
                        >
                          <Play aria-hidden />
                          Start task
                        </button>
                      ) : (
                        <RunStatus
                          record={record}
                          ended={ended}
                          starting={starting}
                          status={taskStatus}
                          failed={failed}
                          onOpen={() => setCard(true)}
                          onGo={props.runs?.open}
                        />
                      ))}
                  </>
                )}
              </div>
              {editing ? (
                <textarea
                  className="pv-edit"
                  aria-label="Edit the file"
                  autoFocus
                  spellCheck={false}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") setEditing(false);
                    else if (
                      (event.metaKey || event.ctrlKey) &&
                      event.key.toLowerCase() === "s"
                    ) {
                      event.preventDefault();
                      void saveEdit();
                    }
                  }}
                />
              ) : (
                <>
                  {title && !/^#\s/m.test(parsed.body) && <h1>{title}</h1>}
                  <MarkdownDoc body={parsed.body} plan={isPlan} />
                </>
              )}
            </article>
          </div>
        ) : kind === "html" || kind === "pdf" ? (
          previewUrl ? (
            <iframe
              key={`${previewUrl}:${hash}`}
              ref={frame}
              onLoad={pickMode}
              className="pv-frame"
              title={path}
              src={previewUrl}
              sandbox={kind === "html" ? "allow-scripts" : undefined}
            />
          ) : (
            <div className="loading">Loading…</div>
          )
        ) : kind === "image" || kind === "svg" ? (
          <div className="pv-image">
            <div className="pv-pinbox">
              <img src={file.dataUrl} alt={path} />
              {commentMode && (
                <div
                  className="pv-overlay"
                  role="button"
                  aria-label="Click the image to place a comment"
                  onClick={pinImage}
                />
              )}
              {comments.map(
                (comment, index) =>
                  comment.pin && (
                    <span
                      key={index}
                      className="pv-pin"
                      style={{
                        left: `${comment.pin.x}%`,
                        top: `${comment.pin.y}%`,
                      }}
                    >
                      {index + 1}
                    </span>
                  ),
              )}
            </div>
          </div>
        ) : (
          <pre className="pv-text">{file.text}</pre>
        )}
        {kind === "markdown" &&
          file.state === "ready" &&
          !editing &&
          selection &&
          !boxFor && (
            <CommentButton
              selection={selection}
              onOpen={() => setBoxFor(selection)}
            />
          )}
        {boxFor && (
          <CommentBox
            selection={{
              ...boxFor,
              x: clampBoxX(boxFor.x, wrap.current?.clientWidth || 0),
            }}
            onAdd={(comment) => {
              setComments([...comments, comment]);
              setBoxFor(null);
              setSelection(null);
              window.getSelection()?.removeAllRanges();
            }}
            onCancel={() => setBoxFor(null)}
          />
        )}
        {card && !editing && (
          <StartTaskCard
            path={path}
            body={parsed.body}
            meta={parsed.meta}
            cwd={cwd}
            connection={connection}
            orchestratorOn={orchestratorOn}
            takenBranch={record?.kind === "agent" ? record.branch : undefined}
            onClose={() => setCard(false)}
            onStart={(runner, branch, base) => void start(runner, branch, base)}
          />
        )}
      </div>
      {file.state === "ready" && (
        <CommentTray
          comments={comments}
          agent={agentLabel}
          disabledReason={noAgent}
          sending={sending}
          error={sendError}
          onClear={() => setComments([])}
          onSend={(items) => void send(items)}
        />
      )}
    </div>
  );
}
