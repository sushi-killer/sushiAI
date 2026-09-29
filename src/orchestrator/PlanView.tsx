/** The Plan view: drafts waiting to run. A placeholder until lane L8 builds
 * the Figma frame. */
export function PlanView({ onBrainstorm }: { onBrainstorm: () => void }) {
  return (
    <div className="orch-view-scroll">
      <h2 className="orch-view-title">Plan</h2>
      <p className="orch-view-lede">Drafts waiting to run.</p>
      <button type="button" className="ui-button ghost" onClick={onBrainstorm}>
        Brainstorm
      </button>
    </div>
  );
}
