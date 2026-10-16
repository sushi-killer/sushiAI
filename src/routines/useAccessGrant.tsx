import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Notice } from "../ui/index.ts";
import { AccessSheet } from "./AccessSheet.tsx";
import { routinesClient } from "./client.ts";
import { ungrantedAccess } from "./model.ts";
import type { AccessEntry, Routine } from "./types.ts";

/** The access flow of one routine, shared by the detail page and the builder:
 * the tools that still wait, the notice with its "Review access" button and
 * the sheet that grants them. The sheet opens by itself once for each list of
 * tools that waits. `act` runs a change with the caller's busy and error
 * handling and refreshes afterwards. */
export function useAccessGrant({
  host,
  id,
  routine,
  fallback,
  busy,
  act,
}: {
  host: string;
  id: string;
  routine: Routine | null | undefined;
  fallback?: AccessEntry[];
  busy: boolean;
  act(work: () => Promise<unknown>): Promise<void>;
}): { waiting: AccessEntry[]; notice: ReactNode; sheet: ReactNode } {
  const [open, setOpen] = useState(false);
  const waiting = useMemo(
    () => ungrantedAccess(routine, fallback),
    [routine, fallback],
  );
  const waitingKey = waiting.map((item) => item.tool).join("\n");
  useEffect(() => setOpen(waitingKey !== ""), [host, id, waitingKey]);
  const notice = waiting.length > 0 && (
    <Notice
      title={`Needs access to ${waiting.length} ${waiting.length === 1 ? "tool" : "tools"}`}
      detail={`${waiting.map((item) => item.label).join(", ")}. Granted to this routine only.`}
      action={
        <button
          type="button"
          className="ui-button secondary"
          onClick={() => setOpen(true)}
        >
          Review access
        </button>
      }
    />
  );
  const sheet = open && waiting.length > 0 && (
    <AccessSheet
      name={routine?.name ?? id}
      items={waiting}
      busy={busy}
      onCancel={() => setOpen(false)}
      onAllow={() =>
        void act(async () => {
          await routinesClient.grant(
            host,
            id,
            waiting.map((item) => item.tool),
          );
          setOpen(false);
        })
      }
    />
  );
  return { waiting, notice, sheet };
}
