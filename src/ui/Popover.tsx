import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ChevronDown } from "lucide-react";

const EDGE = 8;

/** The part of the window a menu may use: the window, cut down by every
 * ancestor that scrolls or clips (a card with its own scrollbar). */
function visibleArea(from: HTMLElement) {
  const area = {
    left: 0,
    top: 0,
    right: document.documentElement.clientWidth,
    bottom: document.documentElement.clientHeight,
  };
  for (
    let node = from.parentElement;
    node && node !== document.body;
    node = node.parentElement
  ) {
    const style = getComputedStyle(node);
    if (style.overflowX === "visible" && style.overflowY === "visible")
      continue;
    const box = node.getBoundingClientRect();
    area.left = Math.max(area.left, box.left + node.clientLeft);
    area.top = Math.max(area.top, box.top + node.clientTop);
    area.right = Math.min(
      area.right,
      box.left + node.clientLeft + node.clientWidth,
    );
    area.bottom = Math.min(
      area.bottom,
      box.top + node.clientTop + node.clientHeight,
    );
  }
  return area;
}

/** A text button with a chevron that opens a panel under it. A click outside
 * or Escape closes it. The panel is moved inside the window when it would
 * cross an edge, and opens upward when there is no room below. */
export function Popover({
  label,
  text,
  disabled,
  children,
}: {
  /** The accessible name of the button and of the panel. */
  label: string;
  text: string;
  disabled?: boolean;
  children(close: () => void): ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [place, setPlace] = useState<{
    shift: number;
    up: boolean;
    maxHeight?: number;
    maxWidth?: number;
  }>({ shift: 0, up: false });
  const ref = useRef<HTMLSpanElement>(null);
  const panel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (!ref.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open) {
      setPlace({ shift: 0, up: false });
      return;
    }
    const node = panel.current;
    const anchor = ref.current;
    if (!node || !anchor) return;
    const box = node.getBoundingClientRect();
    const from = anchor.getBoundingClientRect();
    const area = visibleArea(anchor);
    let shift = 0;
    if (box.right > area.right - EDGE) shift = area.right - EDGE - box.right;
    if (box.left + shift < area.left + EDGE)
      shift = area.left + EDGE - box.left;
    const room = area.bottom - from.bottom - EDGE;
    const above = from.top - area.top - EDGE;
    const up = box.height > room && above > room;
    setPlace({
      shift,
      up,
      maxHeight: Math.max(120, up ? above : room),
      maxWidth: Math.max(160, area.right - area.left - 2 * EDGE),
    });
  }, [open]);

  return (
    <span className="ui-pop" ref={ref}>
      <button
        type="button"
        className="ui-pop-button"
        aria-label={label}
        aria-haspopup="true"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="ui-pop-text">{text}</span>
        <ChevronDown size={12} aria-hidden />
      </button>
      {open && (
        <div
          ref={panel}
          className={`ui-pop-panel${place.up ? " up" : ""}`}
          role="dialog"
          aria-label={label}
          style={{
            left: place.shift,
            ...(place.maxHeight ? { maxHeight: place.maxHeight } : {}),
            ...(place.maxWidth ? { maxWidth: place.maxWidth } : {}),
          }}
        >
          {children(() => setOpen(false))}
        </div>
      )}
    </span>
  );
}

/** One choice of a menu: a title, an optional detail line and a tick. */
export function MenuItem({
  title,
  detail,
  on = false,
  muted = false,
  onClick,
}: {
  title: string;
  detail?: string;
  on?: boolean;
  /** An action under the choices ("Add a host…"), not a choice. */
  muted?: boolean;
  onClick(): void;
}) {
  return (
    <button
      type="button"
      className={`ui-menu-item${on ? " on" : ""}${muted ? " muted" : ""}`}
      onClick={onClick}
    >
      <span className="ui-menu-text">
        <strong>{title}</strong>
        {detail && <small>{detail}</small>}
      </span>
      {on && (
        <span className="ui-menu-check" aria-label="Selected">
          ✓
        </span>
      )}
    </button>
  );
}

/** A small uppercase heading over a group of a menu. */
export function MenuHead({ children }: { children: ReactNode }) {
  return <p className="ui-menu-head">{children}</p>;
}
