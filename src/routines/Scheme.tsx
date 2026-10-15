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
  const projects = (catalog?.projects ?? []).filter(
    (project) =>
      project.id === routine.where.project ||
      project.name === routine.where.project ||
      project.folders.some((folder) => folder.host === host) ||
      project.remote,
  );
  const project =
    projects.find(
      (item) =>
        item.id === routine.where.project ||
        item.name === routine.where.project,
    )?.name ??
    routine.where.project ??
    "";
  const hostName = hostOf(catalog, host)?.name ?? host;
  const hostChoices = hosts.flatMap((item) => {
    const installed = (Object.keys(item.harnesses) as Harness[])
      .filter((id) => item.harnesses[id].installed)
      .map((id) => HARNESSES.find((entry) => entry.id === id)?.label ?? id);
    const note = [
      item.id === "local" ? "Runs only while the Mac is awake" : "Always on",
      installed.join(", "),
    ]
      .filter(Boolean)
      .join(" · ");
    const own = projects.filter((entry) =>
      entry.folders.some((folder) => folder.host === item.id),
    );
    return (own.length ? own : [{ id: "", name: "" }]).map((entry) => ({
      host: item,
      project: entry.name,
      projectId: entry.id,
      note,
    }));
  });
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
          <KeyValueRow label="Where">
            <Popover
              label="Where"
              text={project ? `${hostName} · ${project}` : hostName}
              disabled={busy}
            >
              {(close) => (
                <div className="ui-menu">
                  {hostChoices.map((choice) => {
                    const on =
                      choice.host.id === host && choice.project === project;
                    return (
                      <MenuItem
                        key={`${choice.host.id}:${choice.project}`}
                        title={
                          choice.project
                            ? `${choice.host.name} · ${choice.project}`
                            : choice.host.name
                        }
                        detail={choice.note}
                        on={on}
                        onClick={() => {
                          close();
                          if (choice.host.id !== host) onMove(choice.host.id);
                          else if (choice.project !== project)
                            onChange({
                              ...routine,
                              where: {
                                host: routine.where.host,
                                ...(choice.projectId
                                  ? { project: choice.projectId }
                                  : {}),
                              },
                            });
                        }}
                      />
                    );
                  })}
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
