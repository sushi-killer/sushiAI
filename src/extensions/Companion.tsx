import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { X, type LucideIcon } from "lucide-react";
import type { Companion, Panel } from "../types.ts";
import { companionRatio } from "../workspace/workspace-actions.ts";
import {
  coreViews,
  type CoreViewEntry,
  type CoreViewProps,
  type LaunchAgentRequest,
} from "./coreViews.ts";
import { daemonHost } from "../daemonSessions.ts";
import type { ExtensionRegistry } from "./registry.ts";

export type CompanionPatch = {
  args?: Record<string, string>;
  open?: boolean;
  ratio?: number;
};

/** The core view and the name of the surface a companion shows, or null when
 * its extension is off or has no such view. */
export function companionTarget(
  registry: ExtensionRegistry,
  companion: Companion | undefined,
) {
  if (!companion) return null;
  const surface = registry
    .availableSurfaces()
    .find(
      (item) =>
        item.extensionId === companion.extensionId &&
        item.id === companion.surfaceId,
    );
  const entry =
    surface?.view.kind === "core" ? coreViews[surface.view.viewId] : undefined;
  return surface && entry ? { title: surface.title, ...entry } : null;
}

/** `shown` drives the CSS transition and `mounted` keeps the view alive while
 * it plays out, so hiding slides out before the view goes away. */
function usePresence(open: boolean) {
  const [mounted, setMounted] = useState(open);
  const [shown, setShown] = useState(open);
  useEffect(() => {
    if (open) {
      setMounted(true);
      const frame = requestAnimationFrame(() =>
        requestAnimationFrame(() => setShown(true)),
      );
      return () => cancelAnimationFrame(frame);
    }
    setShown(false);
    const timer = window.setTimeout(() => setMounted(false), 200);
    return () => window.clearTimeout(timer);
  }, [open]);
  return { mounted, shown };
}

/** The agent pane's body: the pane itself, then its companion half when it has
 * one. The pane is always wrapped the same way so it is never remounted when a
 * companion appears. */
export function CompanionSplit({
  panel,
  registry,
  cwd,
  endpoint,
  onLaunchAgent,
  worktrees,
  onCompanion,
  onZoom,
  children,
}: {
  panel: Panel;
  registry: ExtensionRegistry;
  cwd: string;
  endpoint?: string;
  onLaunchAgent?(
    panelId: string,
    request: LaunchAgentRequest,
  ): Promise<boolean>;
  worktrees?: CoreViewProps["worktrees"];
  onCompanion(panelId: string, patch: CompanionPatch): void;
  /** A double click on the half's header zooms the whole pane, like the
   * agent's header. */
  onZoom(): void;
  children: ReactNode;
}) {
  const companion = panel.companion;
  const target = companionTarget(registry, companion);
  const { mounted, shown } = usePresence(Boolean(companion?.open && target));
  const [dragging, setDragging] = useState(false);
  const split = useRef<HTMLDivElement>(null);
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const panelId = panel.id;
  const launchAgent = useCallback<NonNullable<CoreViewProps["launchAgent"]>>(
    (request) =>
      onLaunchAgent ? onLaunchAgent(panelId, request) : Promise.resolve(false),
    [onLaunchAgent, panelId],
  );
  const ratio = companionRatio(companion?.ratio);
  const resize = (value: number) => onCompanion(panel.id, { ratio: value });
  return (
    <div ref={split} className="pane-split">
      <div className="pane-main">{children}</div>
      {companion && target && mounted && (
        <>
          <div
            className={`pane-divider ${shown ? "shown" : ""} ${dragging ? "dragging" : ""}`}
            role="separator"
            aria-orientation="vertical"
            aria-label={`Resize ${target.title}`}
            aria-valuenow={Math.round(ratio * 100)}
            tabIndex={0}
            onKeyDown={(event) => {
              if (event.key !== "ArrowLeft" && event.key !== "ArrowRight")
                return;
              event.preventDefault();
              resize(ratio + (event.key === "ArrowLeft" ? 0.025 : -0.025));
            }}
            onPointerDown={(event) => {
              event.preventDefault();
              event.stopPropagation();
              const el = event.currentTarget;
              el.setPointerCapture(event.pointerId);
              const rect = split.current!.getBoundingClientRect();
              document.body.classList.add("resizing");
              setDragging(true);
              const move = (e: PointerEvent) =>
                resize((rect.right - e.clientX) / rect.width);
              const end = () => {
                el.removeEventListener("pointermove", move);
                el.removeEventListener("pointerup", end);
                el.removeEventListener("pointercancel", end);
                document.body.classList.remove("resizing");
                setDragging(false);
              };
              el.addEventListener("pointermove", move);
              el.addEventListener("pointerup", end);
              el.addEventListener("pointercancel", end);
            }}
          />
          <div
            className={`pane-companion ${shown ? "shown" : ""} ${dragging ? "dragging" : ""}`}
            style={{ flexBasis: shown ? `${ratio * 100}%` : 0 }}
          >
            <div className="pane-companion-half">
              <div
                className="pane-companion-head"
                onDoubleClick={(event) => {
                  const target = event.target as HTMLElement;
                  if (!target.closest("button, select, input")) onZoom();
                }}
              >
                <div className="pane-companion-slot" ref={setSlot} />
                <button
                  className="pane-companion-hide"
                  aria-label={`Hide ${target.title.toLowerCase()}`}
                  title={`Hide ${target.title.toLowerCase()}`}
                  onClick={() => onCompanion(panel.id, { open: false })}
                >
                  <X aria-hidden="true" />
                </button>
              </div>
              <div className="pane-companion-body">
                <target.View
                  args={companion.args}
                  cwd={cwd}
                  paneCwd={panel.paneCwd}
                  connection={endpoint}
                  agentSessionId={panel.sessionId}
                  agentHost={daemonHost(endpoint)}
                  agentLabel={panel.title}
                  launchAgent={onLaunchAgent ? launchAgent : undefined}
                  worktrees={worktrees}
                  headerSlot={slot}
                  onArgs={(args) => onCompanion(panel.id, { args })}
                />
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  );
}

const neverChanged = () => false;

/** The header icon that brings the companion back. It exists only while the
 * companion is hidden, and shows a dot once the file changed in the meantime. */
export function CompanionToggle({
  title,
  args,
  cwd,
  connection,
  icon: ToggleIcon,
  useChanged = neverChanged,
  onShow,
}: {
  title: string;
  args: Record<string, string>;
  cwd: string;
  connection?: string;
  icon: LucideIcon;
  useChanged?: CoreViewEntry["useChanged"];
  onShow(): void;
}) {
  const changed = useChanged(args, cwd, connection);
  const label = `Show ${title.toLowerCase()}`;
  return (
    <button
      className="pane-companion-toggle"
      aria-label={label}
      title={changed ? `${label} (file changed)` : label}
      onClick={onShow}
    >
      <ToggleIcon aria-hidden="true" />
      {changed && <span className="pane-companion-dot" aria-hidden="true" />}
    </button>
  );
}
