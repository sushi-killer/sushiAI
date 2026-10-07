/** Orch/GroupLabel: an uppercase group heading with a count badge. */
export function GroupLabel({ label, count }: { label: string; count: number }) {
  return (
    <div className="ui-group-label">
      <span className="ui-group-label-text">{label}</span>
      <span className="ui-count">{count}</span>
    </div>
  );
}
