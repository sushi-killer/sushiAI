import { useEffect, useRef, useState } from "react";

/** Tracks whether the workspace surface is too small for split panels. The flag
 * lives here because the `compact` class sits on the app root, not the canvas. */
export function useCompact() {
  const ref = useRef<HTMLElement>(null);
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) =>
      setCompact(
        entry.contentRect.width < 720 || entry.contentRect.height < 460,
      ),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [compact, ref] as const;
}
