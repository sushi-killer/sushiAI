import { useEffect, useRef, useState } from "react";
import { insideProject, kindOf } from "./artifact.ts";
import { usePreviewFile } from "./usePreviewFile.ts";

/** True once the file a hidden Preview showed has new content. Mount it only
 * while the Preview is hidden: it reads the file like the open view does. */
export function useChangedWhileHidden(
  args: Record<string, string>,
  cwd: string,
  connection: string | undefined,
): boolean {
  const asked = args.arg || "";
  const path = insideProject(asked, cwd) || "";
  const element = useRef<HTMLElement | null>(null);
  const file = usePreviewFile(path, cwd, kindOf(asked), connection, element);
  const first = useRef("");
  const [changed, setChanged] = useState(false);
  const hash = file.state === "ready" ? file.hash : "";
  useEffect(() => {
    if (!hash) return;
    if (!first.current) first.current = hash;
    else if (hash !== first.current) setChanged(true);
  }, [hash]);
  return changed;
}
