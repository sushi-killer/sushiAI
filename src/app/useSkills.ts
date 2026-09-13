import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "./errors.ts";
import type { SkillCatalogItem, SkillManagementAction } from "../types";

const catalogFingerprint = (items: SkillCatalogItem[]) =>
  items
    .map((item) =>
      [
        item.path,
        item.provider,
        item.harness,
        item.source,
        item.availability,
        item.disabledBy,
        item.plugin,
        item.name,
        item.description,
        item.size,
        item.updatedAt,
        item.lastUsedAt,
        item.lastUsedSource,
        item.usageCount,
        item.isRecent,
        item.isUnused,
        item.isStale,
        item.changed,
        item.isDuplicate,
        item.duplicateKind,
        item.duplicateCount,
        [...(item.duplicateWith || [])].sort().join(","),
        JSON.stringify(item.recentUses || []),
      ].join("\u001f"),
    )
    .join("\u001e");

/** The local skills catalog. The fingerprint keeps a re-scan that found nothing
 * new from replacing the array, so the list does not re-render on every poll. */
export function useSkills(section: string, notify: (text: string) => void) {
  const [catalog, setCatalog] = useState<SkillCatalogItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState("");
  const loadedRef = useRef(false);
  const fingerprintRef = useRef("");

  const load = useCallback(
    async (force = false) => {
      if (!window.bridge) {
        setCatalog([]);
        fingerprintRef.current = "";
        return;
      }
      setLoading(true);
      try {
        const nextCatalog = await window.bridge.catalog("skills", { force });
        const nextFingerprint = catalogFingerprint(nextCatalog);
        if (nextFingerprint !== fingerprintRef.current) {
          setCatalog(nextCatalog);
          fingerprintRef.current = nextFingerprint;
        }
        loadedRef.current = true;
      } catch (error) {
        notify(errorText(error));
      } finally {
        setLoading(false);
      }
    },
    [notify],
  );
  const refresh = useCallback(() => load(true), [load]);
  const manageSkill = useCallback(
    async (action: SkillManagementAction, item: SkillCatalogItem) => {
      if (!window.bridge?.skillsManage)
        throw new Error("Desktop skill management is unavailable.");
      const result = await window.bridge.skillsManage(action, item);
      notify(result.message);
      await load(true);
    },
    [load, notify],
  );

  useEffect(() => {
    setSearch("");
    if (section === "Skills" && !loadedRef.current) void load();
  }, [section, load]);

  return { catalog, loading, search, setSearch, refresh, manageSkill };
}
