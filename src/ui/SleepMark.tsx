import { Moon, Pin } from "lucide-react";

/** The state of an agent session that can sleep, as one small icon right after its
 * name: a moon while it sleeps, a pin while it is kept awake. Nothing otherwise. */
export function SleepMark({
  sleeping,
  keepAwake,
  size = 12,
}: {
  sleeping: boolean;
  keepAwake: boolean;
  size?: number;
}) {
  if (sleeping)
    return (
      <Moon
        className="sleep-mark"
        size={size}
        aria-hidden="true"
        data-testid="sleep-mark"
      />
    );
  if (keepAwake)
    return (
      <Pin
        className="sleep-mark"
        size={size}
        aria-hidden="true"
        data-testid="awake-mark"
      />
    );
  return null;
}
