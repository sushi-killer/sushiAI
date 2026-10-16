import { openSettings } from "../lib/openSettings.ts";
import {
  Chip,
  Eyebrow,
  KeyValue,
  KeyValueRow,
  MenuHead,
  MenuItem,
  MetaLine,
  Popover,
  Segmented,
} from "../ui/index.ts";
import {
  briefRows,
  describeWhen,
  effortsFor,
  harnessProblem,
  HARNESSES,
  hostOf,
  modelNote,
  modelsFor,
  RUN_MODES,
  runModeHelp,
  runModeOf,
  withAgent,
  withRunMode,
} from "./model.ts";
import {
  describeWhere,
  projectsForHost,
  toggleProject,
  whereProblem,
} from "./where.ts";
import type { Harness, Routine, RoutinesCatalog } from "./types.ts";

export const sessionHostOf = (host: string) =>
  host === "local" ? "local" : host.replace(/^ssh:/, "");

/** The two cards of a routine: "How it works" (hairline rows - when, where,
 * which agent, read, do, result) and "How it runs". Every change is one call
 * to `onChange`; a change of host is `onMove`, because a routine lives on the
 * daemon of its host. */
export function Scheme({
  routine,
  host,
  brief = "",
  catalog,
  busy,
  hideMode = false,
  onChange,
  onMove,
}: {
  routine: Routine;
  host: string;
  brief?: string;
  catalog: RoutinesCatalog | null;
  busy: boolean;
  /** A failed routine shows no "How it runs": fix it first. */
  hideMode?: boolean;
  onChange(next: Routine): void;
  onMove(host: string): void;
}) {
  const hosts = (catalog?.hosts ?? []).filter(
    (item) => item.state === "ready" || item.id === host,
  );
  const selected = routine.where.projects ?? [];
  const projects = projectsForHost(catalog, host, selected);
  const isSelected = (id: string, name: string) =>
    selected.includes(id) || selected.includes(name);
  const hostName = hostOf(catalog, host)?.name ?? host;
  const hostChoices = hosts.map((item) => ({
    host: item,
    note: [
      item.id === "local" ? "Runs only while the Mac is awake" : "Always on",
      (Object.keys(item.harnesses) as Harness[])
        .filter((id) => item.harnesses[id].installed)
        .map((id) => HARNESSES.find((entry) => entry.id === id)?.label ?? id)
        .join(", "),
    ]
      .filter(Boolean)
      .join(" · "),
  }));
  const harness = routine.agent.harness;
  const harnessLabel =
    HARNESSES.find((item) => item.id === harness)?.label ?? harness;
  const models = modelsFor(catalog, host, harness);
  const modelLabel =
    models.find((model) => model.id === routine.agent.model)?.label ??
    routine.agent.model ??
    "default";
  const effort = routine.agent.effort ?? "medium";
  const problem = harnessProblem(catalog, host, harness);
  const whereIssue = whereProblem(harness, selected);
  const rows = briefRows(brief);
  const mode = runModeOf(routine);
  const agent = (patch: Parameters<typeof withAgent>[1]) =>
    onChange(withAgent(routine, patch, catalog, host));
  return (
    <>
      <section className="rt-scheme" aria-label="How it works">
        <Eyebrow>How it works</Eyebrow>
        <KeyValue>
          <KeyValueRow label="When">
            {describeWhen(routine.when.cron, routine.when.tz)}
          </KeyValueRow>
          <KeyValueRow label="Where" note={whereIssue || undefined}>
            <Popover
              label="Where"
              text={describeWhere(routine.where, catalog, hostName)}
              disabled={busy}
            >
              {(close) => (
                <div className="ui-menu">
                  <MenuHead>Host</MenuHead>
                  {hostChoices.map((choice) => (
                    <MenuItem
                      key={choice.host.id}
                      title={choice.host.name}
                      detail={choice.note}
                      on={choice.host.id === host}
                      onClick={() => {
                        close();
                        if (choice.host.id !== host) onMove(choice.host.id);
                      }}
                    />
                  ))}
                  {projects.length > 0 && (
                    <>
                      <hr className="ui-menu-sep" />
                      <MenuHead>Projects (optional)</MenuHead>
                      {projects.map((entry) => (
                        <MenuItem
                          key={entry.id}
                          title={entry.name}
                          detail={
                            entry.folders.some((folder) => folder.host === host)
                              ? undefined
                              : "Not on this host yet, it is cloned"
                          }
                          on={isSelected(entry.id, entry.name)}
                          onClick={() =>
                            onChange({
                              ...routine,
                              where: {
                                host: routine.where.host,
                                projects: toggleProject(
                                  selected,
                                  catalog,
                                  entry.id,
                                ),
                              },
                            })
                          }
                        />
                      ))}
                      <p className="ui-menu-note">
                        {whereIssue ||
                          "None: runs in the work folder. One: its own worktree. Several: read only."}
                      </p>
                    </>
                  )}
                  <hr className="ui-menu-sep" />
                  <MenuItem
                    title="Add a host…"
                    muted
                    onClick={() => {
                      close();
                      openSettings("connections");
                    }}
                  />
                </div>
              )}
            </Popover>
          </KeyValueRow>
          <KeyValueRow label="Agent" note={problem || undefined}>
            <Popover
              label="Agent"
              text={`${harnessLabel} · ${modelLabel} · ${effort}`}
              disabled={busy}
            >
              {() => (
                <div className="ui-menu-stack">
                  <div className="ui-menu-group">
                    <MenuHead>Harness</MenuHead>
                    <div
                      className="ui-menu-wrap"
                      role="radiogroup"
                      aria-label="Harness"
                    >
                      {HARNESSES.map((item) => (
                        <Chip
                          key={item.id}
                          radio
                          selected={harness === item.id}
                          onClick={() => agent({ harness: item.id })}
                        >
                          {item.label}
                        </Chip>
                      ))}
                    </div>
                  </div>
                  <div className="ui-menu-group">
                    <MenuHead>Model</MenuHead>
                    <div>
                      {[{ id: "", label: "Harness default" }, ...models].map(
                        (model) => (
                          <MenuItem
                            key={model.id || "default"}
                            title={model.label}
                            detail={modelNote(model.id) || undefined}
                            on={(routine.agent.model ?? "") === model.id}
                            onClick={() => agent({ model: model.id })}
                          />
                        ),
                      )}
                    </div>
                  </div>
                  <div className="ui-menu-group">
                    <MenuHead>Thinking</MenuHead>
                    <Segmented
                      label="Thinking"
                      value={effort}
                      options={effortsFor(harness).map((level) => ({
                        id: level,
                        label: level,
                      }))}
                      onChange={(level) => agent({ effort: level })}
                    />
                  </div>
                  <p className="ui-menu-note">
                    Default for new routines: Settings → Routines.
                  </p>
                </div>
              )}
            </Popover>
          </KeyValueRow>
          {routine.skills && routine.skills.length > 0 && (
            <KeyValueRow label="Skills">
              {routine.skills.join(", ")}
            </KeyValueRow>
          )}
          {rows.read && <KeyValueRow label="Read">{rows.read}</KeyValueRow>}
          {rows.doing && <KeyValueRow label="Do">{rows.doing}</KeyValueRow>}
          {rows.result && (
            <KeyValueRow label="Result">{rows.result}</KeyValueRow>
          )}
        </KeyValue>
      </section>
      {!hideMode && (
        <section className="rt-scheme" aria-label="How it runs">
          <Eyebrow>How it runs</Eyebrow>
          <Segmented
            label="How it runs"
            value={mode}
            disabled={busy}
            options={RUN_MODES}
            onChange={(next) => onChange(withRunMode(routine, next))}
          />
          <MetaLine>{runModeHelp(routine)}</MetaLine>
        </section>
      )}
    </>
  );
}
