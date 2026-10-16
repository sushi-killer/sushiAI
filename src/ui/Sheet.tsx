import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

/** A confirm sheet over a page. The scrim covers the nearest ancestor marked
 * `data-sheet-host` (a positioned box): the page, never the window frame or
 * the navigation. Escape and a click on the scrim close it. */
export function Sheet({
  title,
  hint,
  children,
  footer,
  onClose,
}: {
  title: string;
  hint?: string;
  children: ReactNode;
  footer: ReactNode;
  onClose(): void;
}) {
  const anchor = useRef<HTMLSpanElement>(null);
  const [host, setHost] = useState<Element | null>(null);
  useLayoutEffect(() => {
    const node = anchor.current;
    setHost(node?.closest("[data-sheet-host]") ?? node?.parentElement ?? null);
  }, []);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  const dialog = useRef<HTMLDivElement>(null);
  // A host taller than the window (a stacked page) may open the sheet below
  // the fold: bring it into view.
  useEffect(() => {
    if (host) dialog.current?.scrollIntoView?.({ block: "nearest" });
  }, [host]);
  const sheet = (
    <div
      className="ui-sheet-scrim"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialog}
        className="ui-sheet"
        role="dialog"
        aria-modal
        aria-label={title}
      >
        <h2>{title}</h2>
        {hint && <p className="ui-sheet-hint">{hint}</p>}
        {children}
        <div className="ui-sheet-footer">{footer}</div>
      </div>
    </div>
  );
  return (
    <>
      <span ref={anchor} hidden />
      {host && createPortal(sheet, host)}
    </>
  );
}
