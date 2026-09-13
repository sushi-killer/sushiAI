import { useCallback, useEffect, useState } from "react";

/** One transient message at a time; a new one replaces the old and restarts the
 * six-second timer. */
export function useToast() {
  const [toast, setToast] = useState("");
  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(""), 6000);
    return () => clearTimeout(timer);
  }, [toast]);
  return {
    toast,
    setToast,
    notify: useCallback((text: string) => setToast(text), []),
  };
}
