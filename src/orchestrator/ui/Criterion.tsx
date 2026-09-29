import { Check, Square, X } from "lucide-react";

export type CriterionState = "met" | "pending" | "failing";

/** Orch/Criterion: one acceptance criterion with its met/pending/failing mark. */
export function Criterion({
  state,
  children,
}: {
  state: CriterionState;
  children: string;
}) {
  const Icon = state === "met" ? Check : state === "failing" ? X : Square;
  return (
    <p className={`ui-criterion ${state}`}>
      <Icon size={14} aria-label={state} />
      <span>{children}</span>
    </p>
  );
}
