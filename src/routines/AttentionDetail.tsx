import type { AttentionItem } from "../extensions/modules.ts";
import { MetaLine } from "../ui/index.ts";

/** The Inbox detail of a routines row: what happened, and where to look. */
export function AttentionDetail({ item }: { item: AttentionItem }) {
  return (
    <div className="routines-attention">
      <h3>{item.title}</h3>
      {item.meta && <p>{item.meta}</p>}
      <MetaLine>
        Open Routines in the sidebar for the result and the log.
      </MetaLine>
    </div>
  );
}
