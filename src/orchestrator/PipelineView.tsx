import { useState } from "react";
import { Blueprint } from "./Blueprint";
import { stageEdit, type StageChange } from "./blueprintModel";
import { errorText } from "./helpers";
import type { Settings } from "./types";
import { usePipeline } from "./usePipeline";

/** The Pipeline rail view: the repo's diagram under its preset. Picking a
 * preset or editing a stage writes the settings; the diagram then reloads. */
export function PipelineView({
  cwd,
  settings,
  onSettings,
}: {
  cwd: string;
  settings: Settings | null;
  onSettings(patch: Partial<Settings>): Promise<void>;
}) {
  const { pipeline, error } = usePipeline({ repo: cwd }, settings);
  const [saveError, setSaveError] = useState("");

  async function write(patch: Partial<Settings>) {
    setSaveError("");
    try {
      await onSettings(patch);
    } catch (e) {
      setSaveError(errorText(e));
    }
  }

  function pick(id: string) {
    if (!settings) return;
    const repoPresets = { ...settings.repoPresets };
    if (id) repoPresets[cwd] = id;
    else delete repoPresets[cwd];
    void write({ repoPresets });
  }

  function edit(stageId: string, change: StageChange) {
    if (!settings || !pipeline) return;
    void write(
      stageEdit(
        settings,
        cwd,
        pipeline.presetId,
        stageId,
        change,
        pipeline.presets,
      ),
    );
  }

  return (
    <div className="orch-view-scroll">
      <h2 className="orch-view-title">Pipeline</h2>
      <Blueprint
        pipeline={pipeline}
        routes={settings?.routes ?? []}
        error={saveError || error}
        onEdit={settings ? edit : undefined}
        preset={{
          options: [
            { id: "", label: "Global settings" },
            ...(pipeline?.presets ?? []).map((p) => ({
              id: p.id,
              label: p.label,
            })),
          ],
          value: pipeline?.presetId ?? "",
          disabled: !settings || !pipeline,
          onChange: pick,
        }}
      />
    </div>
  );
}
