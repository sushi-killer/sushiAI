/// <reference types="vite/client" />

import { Profiler, type ReactNode } from "react";

const enabled =
  import.meta.env.DEV &&
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("profile") === "renders";

function reportRender(
  id: string,
  phase: "mount" | "update" | "nested-update",
  actualDuration: number,
  baseDuration: number,
) {
  if (!enabled || typeof window === "undefined") return;
  window.dispatchEvent(
    new CustomEvent("sushiai:render-profile", {
      detail: {
        id,
        phase,
        actualDuration,
        baseDuration,
        at: performance.now(),
      },
    }),
  );
}

export function RenderProfiler({
  id,
  children,
}: {
  id: string;
  children: ReactNode;
}) {
  if (!enabled) return <>{children}</>;
  return (
    <Profiler id={id} onRender={reportRender}>
      {children}
    </Profiler>
  );
}
