import { useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import type { CoreViewProps } from "../extensions/coreViews.ts";
import { Banner, Breadcrumb, Card, MetaLine } from "../ui/index.ts";
import { BuilderView, type Build } from "./BuilderView.tsx";
import { routinesClient } from "./client.ts";
import { entryKey, groupEntries } from "./model.ts";
import { DocView } from "./DocView.tsx";
import { RoutineDetail } from "./RoutineDetail.tsx";
import { RoutineList } from "./RoutineList.tsx";
import { RoutineStart } from "./RoutineStart.tsx";
import { forgetPageState, usePageState } from "./pageState.ts";
import { useRoutinesStore } from "./store.ts";
import type { RoutineEntry, RoutinesCatalog, RunSummary } from "./types.ts";

type Mode =
  | { kind: "list" }
  | { kind: "new"; edit?: RoutineEntry }
  | { kind: "build"; build: Build }
  | { kind: "doc"; entry: RoutineEntry; run: RunSummary };

/** The Routines page: the start (R1), the list with a detail (R2, R5-R7,
 * R10, R11) and the builder (R3, R4). */
export function RoutinesView({ onBare, openSession }: CoreViewProps) {
  const store = useRoutinesStore(true);
  const [catalog, setCatalog] = useState<RoutinesCatalog | null>(null);
  const [mode, setMode] = usePageState<Mode>("mode", { kind: "list" });
  const [selected, setSelected] = usePageState<string | null>("selected", null);
  // In a narrow page the list and the detail take turns.
  const [pane, setPane] = usePageState<"list" | "detail">("pane", "list");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  // Hosts, logins and models change with a refresh of the list.
  useEffect(() => {
    let live = true;
    routinesClient
      .catalog()
      .then((value) => live && setCatalog(value))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [store.entries.length]);

  const current =
    store.entries.find((entry) => entryKey(entry) === selected) ??
    groupEntries(store.entries)[0]?.entries[0] ??
    null;

  async function startBuild(text: string, edit?: RoutineEntry) {
    setBusy(true);
    setError("");
    try {
      const started = await routinesClient.build(text, edit?.routine.id);
      forgetPageState("start-draft");
      setMode({
        kind: "build",
        build: {
          host: started.host,
          sessionHost: started.sessionHost,
          sessionId: started.sessionId,
          harness: started.harness,
          text,
          known: store.entries.map(entryKey),
          ...(edit ? { target: entryKey(edit) } : {}),
        },
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  const starting =
    mode.kind === "new" || (store.loaded && store.entries.length === 0);
  // This page draws its own header, so the host draws none.
  useEffect(() => {
    onBare?.(true);
    return () => onBare?.(false);
  }, [onBare]);

  if (mode.kind === "doc")
    return (
      <DocView
        entry={mode.entry}
        run={mode.run}
        onEdit={() => setMode({ kind: "new", edit: mode.entry })}
        onBack={() => {
          setSelected(entryKey(mode.entry));
          setPane("detail");
          setMode({ kind: "list" });
        }}
      />
    );

  if (mode.kind === "build")
    return (
      <div className="rt-page">
        <BuilderView
          build={mode.build}
          entries={store.entries}
          catalog={catalog}
          onDone={(key) => {
            setSelected(key);
            setPane(key ? "detail" : "list");
            setMode({ kind: "list" });
          }}
        />
      </div>
    );

  if (starting)
    return (
      <div className="rt-page">
        {mode.kind === "new" ? (
          <Breadcrumb
            trail={[
              { label: "Routines", onClick: () => setMode({ kind: "list" }) },
            ]}
            current={mode.edit ? "Edit routine" : "New routine"}
          />
        ) : (
          <header className="rt-head">
            <h1>Routines</h1>
          </header>
        )}
        {store.error && <Banner tone="danger" title={store.error} />}
        <div className="rt-center">
          <RoutineStart
            busy={busy}
            error={error}
            title={
              mode.kind === "new" && mode.edit
                ? `What should change in ${mode.edit.routine.name}?`
                : "What should run on its own?"
            }
            onBuild={(text) =>
              void startBuild(text, mode.kind === "new" ? mode.edit : undefined)
            }
          />
        </div>
      </div>
    );

  return (
    <div className="rt-page" data-sheet-host>
      <header className="rt-head">
        <h1>Routines</h1>
        <button
          type="button"
          className="ui-button secondary"
          onClick={() => setMode({ kind: "new" })}
        >
          New routine
        </button>
      </header>
      {store.error && <Banner tone="danger" title={store.error} />}
      {store.unreachable.map((host) => (
        <Banner
          key={host.host}
          tone="warning"
          title={`${host.name} did not answer`}
          body={host.error}
        />
      ))}
      <div className="rt-split" data-pane={pane}>
        <RoutineList
          entries={store.entries}
          selected={current ? entryKey(current) : null}
          onSelect={(key) => {
            setSelected(key);
            setPane("detail");
          }}
        />
        <div className="rt-detail-col">
          <button
            type="button"
            className="ui-button ghost rt-to-list"
            onClick={() => setPane("list")}
          >
            <ArrowLeft size={14} aria-hidden />
            Routines
          </button>
          {current ? (
            <RoutineDetail
              key={entryKey(current)}
              entry={current}
              catalog={catalog}
              openSession={openSession}
              onEdit={(entry) => setMode({ kind: "new", edit: entry })}
              onGone={() => {
                setSelected(null);
                setPane("list");
              }}
              onDoc={(entry, run) => setMode({ kind: "doc", entry, run })}
            />
          ) : (
            <Card className="rt-pane">
              <MetaLine>
                {store.loaded ? "Pick a routine." : "Loading routines…"}
              </MetaLine>
            </Card>
          )}
        </div>
      </div>
    </div>
  );
}
