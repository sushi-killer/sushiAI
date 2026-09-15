import { memo, useRef, useState, type ReactNode } from "react";
import { Maximize2, Minimize2, MoreHorizontal, Plus, X } from "lucide-react";
import type { Layout, Panel } from "./types";
import { TerminalPanel } from "./TerminalPanel";
import { BrowserPanel } from "./BrowserPanel";
import { ChatPanel } from "./ChatPanel";
import { ProjectPanel } from "./ProjectPanel";
import { RenderProfiler } from "./RenderProfiler";
import { Icon } from "./PanelIcon";
import type { ExtensionRegistry } from "./extensions/registry";
import { ExtensionSurface } from "./extensions/SurfaceRenderer";

type PanelHostProps = {
  panel: Panel;
  cwd: string;
  socket: string;
  endpoint?: string;
  /** Pane provenance (AC23): the workspace's host label, set only when it is
   * a member of a merged sidebar row. Only the terminal/agent pane draws it. */
  hostLabel?: string;
  selected: boolean;
  zoomed: boolean;
  dragging: boolean;
  onFocus(panelId: string): void;
  onDrag(panelId: string): void;
  onDragEnd(): void;
  onDrop(panelId: string, edge: string): void;
  onClose(panelId: string): void;
  onZoom(panelId: string): void;
  onAdd(): void;
  onRename(panelId: string, title: string): void;
  onStart(panelId: string): void;
  onNavigate(panelId: string, url: string): void;
  onHTML(root: string, file: string, endpoint?: string): void;
  onSend(panel: Panel, text: string): void;
  onCancel(panelId: string): void;
  onAgent(panelId: string, agent: string): void;
  extensionRegistry: ExtensionRegistry;
};

export const PanelHost = memo(function PanelHost({
  panel,
  cwd,
  socket,
  endpoint,
  hostLabel,
  selected,
  zoomed,
  dragging,
  onFocus,
  onDrag,
  onDragEnd,
  onDrop,
  onClose,
  onZoom,
  onAdd,
  onRename,
  onStart,
  onNavigate,
  onHTML,
  onSend,
  onCancel,
  onAgent,
  extensionRegistry,
}: PanelHostProps) {
  return (
    <RenderProfiler id={`panel:${panel.id}`}>
      <PanelFrame
        panel={panel}
        selected={selected}
        zoomed={zoomed}
        dragging={dragging}
        onFocus={() => onFocus(panel.id)}
        onDrag={() => onDrag(panel.id)}
        onDragEnd={onDragEnd}
        onDrop={(edge) => onDrop(panel.id, edge)}
        onClose={() => onClose(panel.id)}
        onZoom={() => onZoom(panel.id)}
        onAdd={onAdd}
        onRename={(title) => onRename(panel.id, title)}
      >
        {panel.kind === "terminal" || panel.kind === "agent" ? (
          cwd || !window.bridge ? (
            <TerminalPanel
              panel={panel}
              cwd={cwd}
              socket={socket}
              endpoint={endpoint}
              hostLabel={hostLabel}
              onStart={() => onStart(panel.id)}
            />
          ) : (
            <div className="loading">Opening workspace…</div>
          )
        ) : panel.kind === "browser" ? (
          <BrowserPanel
            url={panel.url}
            endpoint={endpoint}
            sourceFile={panel.previewFile}
            onNavigate={(url) => onNavigate(panel.id, url)}
          />
        ) : panel.kind === "files" ? (
          <ProjectPanel
            key={`${panel.id}:${panel.filesTarget?.root || cwd}:${panel.filesTarget?.path || ""}:${panel.filesTarget?.openToken || ""}`}
            cwd={panel.filesTarget?.root || cwd}
            initialFile={panel.filesTarget?.path}
            initialEdit={panel.filesTarget?.edit}
            endpoint={panel.filesTarget ? panel.filesTarget.endpoint : endpoint}
            onHTML={onHTML}
          />
        ) : panel.kind === "extension" ? (
          <ExtensionSurface
            panel={panel}
            cwd={cwd}
            connection={endpoint}
            registry={extensionRegistry}
          />
        ) : (
          <ChatPanel
            panel={panel}
            onSend={(text) => onSend(panel, text)}
            onCancel={() => onCancel(panel.id)}
            onAgent={(agent) => onAgent(panel.id, agent)}
          />
        )}
      </PanelFrame>
    </RenderProfiler>
  );
});

export function LayoutView({
  layout,
  renderPanel,
  onResize,
}: {
  layout: Layout;
  renderPanel(id: string): ReactNode;
  onResize(id: string, ratio: number): void;
}) {
  const container = useRef<HTMLDivElement>(null);
  if (layout.type === "leaf") return renderPanel(layout.id);
  const axis = layout.axis;
  return (
    <div ref={container} className={`split split-${axis}`}>
      <div
        className="split-child"
        style={{ flexGrow: layout.ratio, flexBasis: 0 }}
      >
        <LayoutView
          layout={layout.a}
          renderPanel={renderPanel}
          onResize={onResize}
        />
      </div>
      <div
        className={`split-handle ${axis}`}
        role="separator"
        aria-orientation={axis === "row" ? "vertical" : "horizontal"}
        aria-label="Resize panels"
        tabIndex={0}
        onKeyDown={(event) => {
          if (
            ["ArrowLeft", "ArrowUp", "ArrowRight", "ArrowDown"].includes(
              event.key,
            )
          ) {
            event.preventDefault();
            onResize(
              layout.id,
              Math.max(
                0.15,
                Math.min(
                  0.85,
                  layout.ratio +
                    (event.key === "ArrowLeft" || event.key === "ArrowUp"
                      ? -0.025
                      : 0.025),
                ),
              ),
            );
          }
        }}
        onPointerDown={(event) => {
          event.preventDefault();
          const el = event.currentTarget;
          el.setPointerCapture(event.pointerId);
          const rect = container.current!.getBoundingClientRect();
          document.body.classList.add("resizing");
          const move = (e: PointerEvent) => {
            const value =
              axis === "row"
                ? (e.clientX - rect.left) / rect.width
                : (e.clientY - rect.top) / rect.height;
            onResize(layout.id, Math.max(0.15, Math.min(0.85, value)));
          };
          const end = () => {
            el.removeEventListener("pointermove", move);
            el.removeEventListener("pointerup", end);
            el.removeEventListener("pointercancel", end);
            document.body.classList.remove("resizing");
          };
          el.addEventListener("pointermove", move);
          el.addEventListener("pointerup", end);
          el.addEventListener("pointercancel", end);
        }}
      />
      <div
        className="split-child"
        style={{ flexGrow: 1 - layout.ratio, flexBasis: 0 }}
      >
        <LayoutView
          layout={layout.b}
          renderPanel={renderPanel}
          onResize={onResize}
        />
      </div>
    </div>
  );
}

function PanelFrame({
  panel,
  selected,
  zoomed,
  dragging,
  onFocus,
  onDrag,
  onDragEnd,
  onDrop,
  onClose,
  onZoom,
  onAdd,
  onRename,
  children,
}: {
  panel: Panel;
  selected: boolean;
  zoomed: boolean;
  dragging: boolean;
  onFocus(): void;
  onDrag(): void;
  onDragEnd(): void;
  onDrop(edge: string): void;
  onClose(): void;
  onZoom(): void;
  onAdd(): void;
  onRename(title: string): void;
  children: ReactNode;
}) {
  const [edge, setEdge] = useState(""),
    [menu, setMenu] = useState(false),
    [rename, setRename] = useState(false);
  return (
    <section
      className={`panel ${selected ? "focused" : ""} panel-${panel.kind}`}
      data-panel-id={panel.id}
      onMouseDown={onFocus}
      onDragOver={(event) => {
        if (!dragging) return;
        event.preventDefault();
        const r = event.currentTarget.getBoundingClientRect();
        const x = (event.clientX - r.left) / r.width,
          y = (event.clientY - r.top) / r.height;
        setEdge(
          x < 0.22
            ? "left"
            : x > 0.78
              ? "right"
              : y < 0.22
                ? "top"
                : y > 0.78
                  ? "bottom"
                  : "center",
        );
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node))
          setEdge("");
      }}
      onDrop={(event) => {
        event.preventDefault();
        onDrop(edge || "center");
        setEdge("");
      }}
    >
      <header
        className="panel-header"
        draggable={!rename}
        onDragStart={(event) => {
          event.dataTransfer.effectAllowed = "move";
          event.dataTransfer.setData("text/plain", panel.id);
          onDrag();
        }}
        onDragEnd={() => {
          setEdge("");
          onDragEnd();
        }}
        onDoubleClick={onZoom}
      >
        <span
          className={`status-dot ${panel.status === "working" || (panel.kind === "agent" && panel.started) ? "green" : panel.status === "blocked" ? "yellow" : ""}`}
        />
        <Icon kind={panel.kind} agent={panel.agent} size={12} />
        {rename ? (
          <input
            className="panel-rename"
            defaultValue={panel.title}
            autoFocus
            onBlur={(event) => {
              if (event.target.value.trim())
                onRename(event.target.value.trim());
              setRename(false);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
          />
        ) : (
          <span className="panel-title">{panel.title}</span>
        )}
        <div className="panel-actions">
          <button
            aria-label={`Options for ${panel.title}`}
            onClick={() => setMenu(!menu)}
          >
            <MoreHorizontal />
          </button>
          <button aria-label={`Maximize ${panel.title}`} onClick={onZoom}>
            {zoomed ? <Minimize2 /> : <Maximize2 />}
          </button>
          <button aria-label="Add panel" onClick={onAdd}>
            <Plus />
          </button>
          <button aria-label={`Close ${panel.title}`} onClick={onClose}>
            <X />
          </button>
        </div>
        {menu && (
          <div className="panel-menu">
            <button
              onClick={() => {
                setRename(true);
                setMenu(false);
              }}
            >
              Rename panel
            </button>
            <button
              onClick={() => {
                onZoom();
                setMenu(false);
              }}
            >
              {zoomed ? "Restore layout" : "Focus panel"}
            </button>
            <button onClick={onClose}>
              {panel.herdrId ? "Close / end session…" : "Close panel"}
            </button>
          </div>
        )}
      </header>
      <div className="panel-content">{children}</div>
      {dragging && edge && (
        <div className={`drop-zone edge-${edge}`}>
          <span>{edge === "center" ? "Swap panels" : `Place ${edge}`}</span>
        </div>
      )}
    </section>
  );
}
