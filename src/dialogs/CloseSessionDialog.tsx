import type { Panel, Workspace } from "../types";

export function CloseSessionDialog({
  workspace,
  panel,
  onHide,
  onEnd,
}: {
  workspace: Workspace;
  panel: Panel;
  onHide(): void;
  onEnd(): void;
}) {
  return (
    <>
      <div className="dialog-eyebrow">CLOSE SESSION</div>
      <h2>{panel.title}</h2>
      <p>
        {workspace.name} · {panel.herdrId}
      </p>
      <p>
        Hide this panel to keep its process running, or end the actual session.
      </p>
      <div className="dialog-actions">
        <button className="secondary" onClick={onHide}>
          Hide only
        </button>
        <button className="danger" onClick={onEnd}>
          End session
        </button>
      </div>
    </>
  );
}
