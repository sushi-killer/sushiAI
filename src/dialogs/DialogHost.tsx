import { X } from "lucide-react";
import type { ReactNode } from "react";
import { DIALOG_META, type Dialog } from "./dialog-state.ts";

/** The shell every dialog shares: backdrop, framing and the close button. It
 * holds no business logic and no dialog closes itself — the caller does, after
 * its action succeeds. */
export function DialogHost({
  dialog,
  onClose,
  children,
}: {
  dialog: Dialog | null;
  onClose(): void;
  children: ReactNode;
}) {
  if (!dialog) return null;
  const meta = DIALOG_META[dialog.kind];
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className={`modal ${meta.className}`}
        role="dialog"
        aria-modal="true"
        aria-label={meta.label}
      >
        <button
          className="modal-close icon-button"
          aria-label="Close dialog"
          onClick={onClose}
        >
          <X size={17} />
        </button>
        {children}
      </div>
    </div>
  );
}
