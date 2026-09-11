import { useEffect, useMemo, useRef, useState } from "react";
import {
  GitBranch,
  GitCommitVertical,
  GitMerge,
  RefreshCw,
} from "lucide-react";
import { SyntaxHighlightedCode } from "./SyntaxHighlightedCode";
import {
  classifyRefs,
  computeGitGraph,
  laneColor,
  type GitLogCommit,
} from "./gitGraph";

type Branch = {
  name: string;
  ref: string;
  current: boolean;
  upstream: string;
  track: string;
  date: number;
  subject: string;
  local: boolean;
  origin: string;
  remoteOnly?: boolean;
};
type CommitFile = { status: string; path: string; previous?: string };
type CommitDetail = {
  hash: string;
  parents: string[];
  author: string;
  email: string;
  date: number;
  refs: string[];
  subject: string;
  body: string;
  files: CommitFile[];
};

const ROW_HEIGHT = 50;
const LANE_WIDTH = 14;
const NODE_RADIUS = 4;

function formatDate(seconds: number) {
  if (!seconds) return "";
  return new Date(seconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// Fork-style relative stamp: the row only needs "how long ago", the exact
// timestamp lives in the tooltip.
function relativeDate(seconds: number) {
  if (!seconds) return "";
  const minutes = Math.floor((Date.now() / 1000 - seconds) / 60);
  if (minutes < 1) return "now";
  const units: [number, string][] = [
    [525600, "y"],
    [43200, "mo"],
    [10080, "w"],
    [1440, "d"],
    [60, "h"],
  ];
  for (const [size, label] of units) {
    const count = Math.floor(minutes / size);
    if (count >= 1) return `${count}${label}`;
  }
  return `${minutes}m`;
}

// git reports ahead/behind against the upstream as >, < or <>.
function trackLabel(track: string) {
  if (track === ">") return "↑";
  if (track === "<") return "↓";
  if (track === "<>") return "↑↓";
  return "";
}

function displayError(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  return message
    .replace(/^Error invoking remote method '[^']+':\s*Error:\s*/, "")
    .replace(/^Error:\s*/, "");
}

export function GitHistoryPanel({
  root,
  endpoint,
}: {
  root: string;
  endpoint?: string;
}) {
  const [scope, setScope] = useState<"all" | "current">("all"),
    [reloadToken, setReloadToken] = useState(0),
    [commits, setCommits] = useState<GitLogCommit[]>([]),
    [head, setHead] = useState<string | null>(null),
    [truncated, setTruncated] = useState(false),
    [loading, setLoading] = useState(false),
    [error, setError] = useState(""),
    [selected, setSelected] = useState(""),
    [detail, setDetail] = useState<CommitDetail | null>(null),
    [detailError, setDetailError] = useState(""),
    [detailBusy, setDetailBusy] = useState(false),
    [selectedFile, setSelectedFile] = useState(""),
    [diffText, setDiffText] = useState(""),
    [diffBusy, setDiffBusy] = useState(false),
    [branches, setBranches] = useState<Branch[]>([]),
    [historyRef, setHistoryRef] = useState("");

  const listVersion = useRef(0),
    detailVersion = useRef(0),
    diffVersion = useRef(0);

  const inspect = (operation: string, extra: Record<string, unknown> = {}) => {
    if (!window.bridge)
      return Promise.reject(
        new Error("Open the desktop app to browse project history."),
      );
    return window.bridge.projectInspect(endpoint, { operation, root, ...extra });
  };

  useEffect(() => {
    const revision = ++listVersion.current;
    setLoading(true);
    setError("");
    setSelected("");
    setDetail(null);
    setSelectedFile("");
    setDiffText("");
    inspect("log", {
      refs: historyRef ? "current" : scope,
      ...(historyRef ? { branch: historyRef } : {}),
    })
      .then((result) => {
        if (revision !== listVersion.current) return;
        setCommits(result.commits || []);
        setHead(result.head || null);
        setTruncated(!!result.truncated);
      })
      .catch((e) => {
        if (revision === listVersion.current) setError(displayError(e));
      })
      .finally(() => {
        if (revision === listVersion.current) setLoading(false);
      });
  }, [root, endpoint, scope, historyRef, reloadToken]);

  const branchVersion = useRef(0);
  useEffect(() => {
    const revision = ++branchVersion.current;
    inspect("branches")
      .then((result) => {
        if (revision === branchVersion.current)
          setBranches(result.branches || []);
      })
      .catch(() => {
        // Branch chrome is optional; the graph below still works without it.
        if (revision === branchVersion.current) setBranches([]);
      });
  }, [root, endpoint, reloadToken]);

  const currentBranch = useMemo(
    () => branches.find((branch) => branch.current),
    [branches],
  );

  const layout = useMemo(() => computeGitGraph(commits), [commits]);
  const laneOf = useMemo(() => {
    const map = new Map<string, number>();
    for (const node of layout.nodes) map.set(node.hash, node.lane);
    return map;
  }, [layout]);
  const edgesByRow = useMemo(() => {
    const map = new Map<number, typeof layout.edges>();
    for (const edge of layout.edges) {
      const list = map.get(edge.row) || [];
      list.push(edge);
      map.set(edge.row, list);
    }
    return map;
  }, [layout]);
  const graphWidth = Math.max(1, layout.laneCount) * LANE_WIDTH + LANE_WIDTH;
  const diffStats = useMemo(() => {
    const lines = diffText.split("\n");
    return {
      additions: lines.filter(
        (line) => line.startsWith("+") && !line.startsWith("+++"),
      ).length,
      deletions: lines.filter(
        (line) => line.startsWith("-") && !line.startsWith("---"),
      ).length,
    };
  }, [diffText]);

  function openCommit(hash: string) {
    setSelected(hash);
    setSelectedFile("");
    setDiffText("");
    setDetailError("");
    setDetailBusy(true);
    const revision = ++detailVersion.current;
    inspect("commit", { commit: hash })
      .then((result: CommitDetail) => {
        if (revision !== detailVersion.current) return;
        setDetail(result);
        if (result.files?.length) openCommitFile(hash, result.files[0].path);
      })
      .catch((e) => {
        if (revision === detailVersion.current) setDetailError(String(e));
      })
      .finally(() => {
        if (revision === detailVersion.current) setDetailBusy(false);
      });
  }

  function openCommitFile(hash: string, path: string) {
    setSelectedFile(path);
    setDiffBusy(true);
    setDiffText("");
    const revision = ++diffVersion.current;
    inspect("diff", { commit: hash, path })
      .then((result) => {
        if (revision === diffVersion.current)
          setDiffText(result.text || "No changes for this file.");
      })
      .catch((e) => {
        if (revision === diffVersion.current) setDiffText(String(e));
      })
      .finally(() => {
        if (revision === diffVersion.current) setDiffBusy(false);
      });
  }

  function renderEdge(edge: { fromLane: number; toLane: number }, top: boolean) {
    const x1 = LANE_WIDTH / 2 + edge.fromLane * LANE_WIDTH + LANE_WIDTH / 2;
    const x2 = LANE_WIDTH / 2 + edge.toLane * LANE_WIDTH + LANE_WIDTH / 2;
    const [xa, ya, xb, yb] = top
      ? [x2, 0, x1, ROW_HEIGHT / 2]
      : [x1, ROW_HEIGHT / 2, x2, ROW_HEIGHT];
    const color = laneColor(top ? edge.toLane : edge.fromLane);
    if (xa === xb)
      return (
        <line
          key={`${top ? "t" : "b"}-${edge.fromLane}-${edge.toLane}`}
          x1={xa}
          y1={ya}
          x2={xb}
          y2={yb}
          stroke={color}
          strokeWidth={1.6}
        />
      );
    const midY = (ya + yb) / 2;
    return (
      <path
        key={`${top ? "t" : "b"}-${edge.fromLane}-${edge.toLane}`}
        d={`M${xa},${ya} C${xa},${midY} ${xb},${midY} ${xb},${yb}`}
        fill="none"
        stroke={color}
        strokeWidth={1.6}
      />
    );
  }

  const detailBadges = detail ? classifyRefs(detail.refs) : [];
  const selectedBranch = branches.find((branch) => branch.ref === historyRef);
  const scopeLabel = historyRef
    ? selectedBranch?.remoteOnly
      ? "Origin branch"
      : "Branch history"
    : scope === "all"
      ? "All branches"
      : "Current branch";

  return (
    <div className="project-body git-history">
      <aside className="git-history-list">
        <div className="history-toolbar">
          <div className="history-branch-control">
            <GitBranch size={12} />
            <select
              aria-label="View branch history"
              value={historyRef}
              disabled={!branches.length}
              onChange={(e) => {
                const ref = e.target.value;
                setHistoryRef(ref);
                setScope(ref ? "current" : "all");
              }}
            >
              {!branches.length ? (
                <option value="">No branches</option>
              ) : (
                <>
                  <option value="">All branches</option>
                  <optgroup label="Local branches">
                    {branches
                      .filter((branch) => branch.local)
                      .map((branch) => (
                        <option key={branch.ref} value={branch.ref}>
                          {branch.name} · {branch.origin ? "local · origin" : "local only"}
                        </option>
                      ))}
                  </optgroup>
                  {branches.some((branch) => branch.remoteOnly) && (
                    <optgroup label="Origin only">
                      {branches
                        .filter((branch) => branch.remoteOnly)
                        .map((branch) => (
                          <option key={branch.ref} value={branch.ref}>
                            {branch.name} · origin only
                          </option>
                        ))}
                    </optgroup>
                  )}
                </>
              )}
            </select>
            {currentBranch && trackLabel(currentBranch.track) && (
              <span className="history-track" title="Ahead/behind upstream">
                {trackLabel(currentBranch.track)}
              </span>
            )}
          </div>
          <button
            className="history-refresh"
            title="Refresh history"
            onClick={() => setReloadToken((token) => token + 1)}
          >
            <RefreshCw size={12} />
          </button>
        </div>
        <div className="history-toolbar-sub">
          <div className="history-scope" role="group" aria-label="History scope">
            <button
              className={scope === "all" ? "active" : ""}
              aria-pressed={scope === "all"}
              onClick={() => {
                setHistoryRef("");
                setScope("all");
              }}
            >
              All
            </button>
            <button
              className={scope === "current" ? "active" : ""}
              aria-pressed={scope === "current"}
              onClick={() => {
                setHistoryRef(currentBranch?.ref || "");
                setScope("current");
              }}
            >
              Current
            </button>
          </div>
          <span className="history-count">
            {loading ? "Loading…" : `${commits.length} commits`}
          </span>
        </div>
        <div className="history-context">
          <span className="history-context-label">{scopeLabel}</span>
          <span
            className="history-head"
            title={historyRef || head || "Current HEAD"}
          >
            {historyRef || head || "—"}
          </span>
        </div>
        {error && (
          <div className="inline-error history-error" role="alert">
            {error}
          </div>
        )}
        <div className="history-rows">
          {loading ? (
            <p className="muted clean-tree">Loading history…</p>
          ) : !commits.length ? (
            <p className="muted clean-tree">No commits yet.</p>
          ) : (
            commits.map((commit, row) => {
              const lane = laneOf.get(commit.hash) ?? 0;
              const incoming = edgesByRow.get(row - 1) || [];
              const outgoing = edgesByRow.get(row) || [];
              const badges = classifyRefs(commit.refs);
              const isMerge = commit.parents.length > 1;
              return (
                <button
                  key={commit.hash}
                  className={`history-commit ${selected === commit.hash ? "active" : ""}`}
                  title={`${commit.hash}\n${commit.subject}`}
                  aria-label={`${commit.subject}, ${commit.hash}`}
                  onClick={() => openCommit(commit.hash)}
                >
                  <svg
                    className="history-graph"
                    width={graphWidth}
                    height={ROW_HEIGHT}
                    viewBox={`0 0 ${graphWidth} ${ROW_HEIGHT}`}
                    aria-hidden="true"
                  >
                    {incoming.map((edge) => renderEdge(edge, true))}
                    {outgoing.map((edge) => renderEdge(edge, false))}
                    <circle
                      cx={LANE_WIDTH / 2 + lane * LANE_WIDTH + LANE_WIDTH / 2}
                      cy={ROW_HEIGHT / 2}
                      r={NODE_RADIUS}
                      fill={laneColor(lane)}
                      stroke={isMerge ? "#0c0c0c" : "none"}
                      strokeWidth={isMerge ? 1.8 : 0}
                    />
                  </svg>
                  <span className="history-commit-copy">
                    <span className="history-commit-title">
                      {commit.subject}
                    </span>
                    <span className="history-commit-meta">
                      <code>{commit.hash.slice(0, 7)}</code>
                      {isMerge && <GitMerge size={11} className="history-merge" />}
                      {badges.slice(0, 2).map((badge) => (
                        <code
                          key={badge.label}
                          className={`ref-badge ref-${badge.kind}`}
                        >
                          {badge.label}
                        </code>
                      ))}
                    </span>
                  </span>
                  <span className="history-commit-time" title={formatDate(commit.date)}>
                    {relativeDate(commit.date)}
                  </span>
                </button>
              );
            })
          )}
          {truncated && (
            <p className="muted clean-tree">
              Showing the most recent {commits.length} commits.
            </p>
          )}
        </div>
      </aside>
      <section className="git-history-detail">
        {!selected ? (
          <div className="history-empty">
            <GitCommitVertical size={24} />
            <strong>Commit details</strong>
            <span>Select a commit to inspect its files and diff.</span>
          </div>
        ) : (
          <>
            <header className="history-inspector-head">
              <div className="history-inspector-title">
                <span className="history-kicker">COMMIT</span>
                <h2 title={detail?.hash}>{detail?.subject || "Loading…"}</h2>
              </div>
              {detail && (
                <div className="history-inspector-refs">
                  {detailBadges.map((badge) => (
                    <code key={badge.label} className={`ref-badge ref-${badge.kind}`}>
                      {badge.label}
                    </code>
                  ))}
                </div>
              )}
            </header>
            {detailError && (
              <div className="inline-error history-error" role="alert">
                {detailError}
              </div>
            )}
            {detailBusy && !detail ? (
              <div className="history-empty">Loading commit…</div>
            ) : detail ? (
              <>
                <div className="history-commit-meta-bar">
                  <code className="commit-sha" title={detail.hash}>
                    {detail.hash.slice(0, 9)}
                  </code>
                  <span className="commit-author" title={detail.email}>
                    {detail.author}
                  </span>
                  <span className="commit-date" title={formatDate(detail.date)}>
                    {relativeDate(detail.date)}
                  </span>
                  {detail.parents.length > 1 && (
                    <span className="commit-parents">
                      merge · {detail.parents.map((p) => p.slice(0, 7)).join(" + ")}
                    </span>
                  )}
                </div>
                {detail.body && <p className="commit-body">{detail.body}</p>}
                <div className="history-diff-layout">
                  <aside className="history-files-pane">
                    <div className="history-pane-label">
                      <span>Changed files</span>
                      <span>{detail.files.length}</span>
                    </div>
                    <div className="history-files-list">
                      {detail.files.length ? (
                        detail.files.map((file) => (
                          <button
                            key={file.path}
                            className={`history-file ${selectedFile === file.path ? "active" : ""}`}
                            title={
                              file.previous
                                ? `${file.previous} → ${file.path}`
                                : file.path
                            }
                            onClick={() => openCommitFile(selected, file.path)}
                          >
                            <code className={`git-status status-${file.status.trim()[0]}`}>
                              {file.status}
                            </code>
                            <span>{file.path}</span>
                          </button>
                        ))
                      ) : (
                        <p className="muted clean-tree">No file changes.</p>
                      )}
                    </div>
                  </aside>
                  <div className="history-diff-pane">
                    <div className="history-pane-label">
                      <span className="history-diff-path" title={selectedFile}>
                        {selectedFile || "Diff"}
                      </span>
                      {diffText && (
                        <span className="history-diff-stats">
                          <b>+{diffStats.additions}</b>
                          <i>−{diffStats.deletions}</i>
                        </span>
                      )}
                    </div>
                    <div className="commit-diff">
                      {diffBusy ? (
                        <div className="history-empty">Loading diff…</div>
                      ) : diffText ? (
                        <SyntaxHighlightedCode text={diffText} path={selectedFile} diff />
                      ) : (
                        <div className="history-empty">
                          <span>Select a file to view its diff.</span>
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              </>
            ) : null}
          </>
        )}
      </section>
    </div>
  );
}
