import {
  ArrowUpRight,
  Blocks,
  FolderOpen,
  Play,
  Plug,
  Plus,
  Sparkles,
  TerminalSquare,
  Trash2,
  Workflow,
  X,
} from "lucide-react";
import { ExtensionsView } from "../extensions/ExtensionsView.tsx";
import { PageFrame } from "./PageFrame.tsx";
import {
  ExtensionNavSlot,
  ExtensionPageActions,
  ExtensionSectionSlot,
} from "../extensions/ExtensionSlots.tsx";
import { ExtensionSurfaceView } from "../extensions/SurfaceRenderer.tsx";
import { activePage } from "../extensions/routes.ts";
import type { SectionRef } from "./navigation.ts";
import type { ExtensionRegistry } from "../extensions/registry.ts";
import { LocalSkillsView } from "../LocalSkillsView.tsx";
import { Empty, LayersIcon } from "./Empty.tsx";
import { codePanels } from "../workspaceState.ts";
import type { Routine } from "../workspaceState.ts";
import type { ExtensionSnapshot } from "../extensions/types.ts";
import type { Workspace } from "../types";
import type { SkillCatalogItem, SkillManagementAction } from "../types";

/** The one page in the working area. A core section and a page an extension
 * contributed are both drawn here, in the same frame - there is no second path
 * that could draw one differently, or draw one without the shell around it. */
export function SectionPage({
  section,
  runExtensionCommand,
  workspaces,
  routines,
  setRoutines,
  runRoutine,
  skills,
  extensionSnapshot,
  notify,
  connected,
  activeEndpoint,
  home,
  switchWorkspace,
  addExtensionPanel,
  setExtensionEnabled,
  refreshExtensions,
  openRoutineDialog,
  totalPanels,
  registry,
  openExtensionTarget,
  cwd,
  connection,
}: {
  section: SectionRef;
  runExtensionCommand(extensionId: string, commandId: string): void;
  workspaces: Workspace[];
  routines: Routine[];
  setRoutines(update: (items: Routine[]) => Routine[]): void;
  runRoutine(routine: Routine): void;
  skills: {
    catalog: SkillCatalogItem[];
    loading: boolean;
    search: string;
    setSearch(value: string): void;
    refresh(): void;
    manageSkill(
      action: SkillManagementAction,
      item: SkillCatalogItem,
    ): Promise<void>;
  };
  extensionSnapshot: ExtensionSnapshot;
  notify(text: string): void;
  connected: boolean;
  activeEndpoint: string;
  home?: string;
  switchWorkspace(id: string): void;
  addExtensionPanel(extensionId: string, contributionId: string): void;
  setExtensionEnabled(extensionId: string, enabled: boolean): void;
  refreshExtensions(): void;
  openRoutineDialog(): void;
  totalPanels: number;
  registry: ExtensionRegistry;
  openExtensionTarget(extensionId: string, targetSurfaceId: string): void;
  cwd: string;
  connection?: string;
}) {
  const surface =
    section.kind === "extension" ? activePage(registry, section) : undefined;
  if (section.kind === "extension")
    return (
      <PageFrame
        eyebrow="YOUR WORKSPACE"
        title={surface?.title || "Extension"}
        description={surface?.description}
        actions={
          surface && (
            <ExtensionPageActions
              registry={registry}
              surface={surface}
              onRun={runExtensionCommand}
            />
          )
        }
      >
        {surface ? (
          <ExtensionSurfaceView
            surface={surface}
            cwd={cwd}
            connection={connection}
            workspaces={workspaces}
            frame="page"
          />
        ) : null}
      </PageFrame>
    );
  const name = section.id;
  return (
    <PageFrame
      eyebrow="YOUR WORKSPACE"
      title={name}
      description={
        name === "Dashboard"
          ? "A little space for everything you\u2019re building."
          : name === "Routines"
            ? "Your everyday commands, one click away."
            : name === "Extensions"
              ? "App-wide packages that can add pages, panels and actions."
              : name === "Skills"
                ? "Skills found on this Mac, grouped by harness and ready for cleanup review."
                : "Extensions connected to your Herdr session."
      }
      actions={
        <>
          {name === "Extensions" && <Blocks size={26} />}
          {name === "Dashboard" && (
            <ExtensionNavSlot
              registry={registry}
              placement="dashboard.navigation"
              className="secondary extension-nav"
              onOpen={openExtensionTarget}
            />
          )}
          {name === "Skills" && (
            <ExtensionNavSlot
              registry={registry}
              placement="skills.navigation"
              className="secondary extension-nav"
              onOpen={openExtensionTarget}
            />
          )}
          {name === "Routines" && (
            <button className="primary" onClick={() => openRoutineDialog()}>
              <Plus size={14} /> New routine
            </button>
          )}
        </>
      }
    >
      {name === "Dashboard" ? (
        <>
          <ExtensionSectionSlot
            registry={registry}
            host="dashboard.section"
            cwd={cwd}
            connection={connection}
            workspaces={workspaces}
          />
          <div className="stat-grid">
            <div>
              <LayersIcon />
              <strong>{workspaces.length}</strong>
              <span>Workspaces</span>
            </div>
            <div>
              <TerminalSquare size={19} />
              <strong>{totalPanels}</strong>
              <span>Open panels</span>
            </div>
            <div>
              <Sparkles size={19} />
              <strong>
                {
                  workspaces
                    .flatMap((w) => w.panels)
                    .filter((p) => p.status === "working").length
                }
              </strong>
              <span>Agents working</span>
            </div>
          </div>
          <h3>Pick up where you left off</h3>
          <div className="project-grid">
            {workspaces.map((w) => (
              <button key={w.id} onClick={() => switchWorkspace(w.id)}>
                <FolderOpen size={19} />
                <strong>{w.name}</strong>
                <p>{w.cwd}</p>
                <span>
                  {codePanels(w).length} panels <ArrowUpRight size={13} />
                </span>
              </button>
            ))}
          </div>
        </>
      ) : name === "Routines" ? (
        routines.length ? (
          <div className="routine-list">
            {routines.map((r) => (
              <div key={r.id}>
                <Workflow size={17} />
                <div>
                  <strong>{r.name}</strong>
                  <code>{r.command}</code>
                </div>
                <button title="Run routine" onClick={() => runRoutine(r)}>
                  <Play size={15} />
                </button>
                <button
                  title="Delete routine"
                  onClick={() =>
                    setRoutines((items) =>
                      items.filter((item) => item.id !== r.id),
                    )
                  }
                >
                  <Trash2 size={14} />
                </button>
              </div>
            ))}
          </div>
        ) : (
          <Empty
            icon={<Workflow size={28} />}
            title="Make room for your rituals."
            text="Save build, test, and development commands for this workspace."
            action="Create a routine"
            onAction={() => openRoutineDialog()}
          />
        )
      ) : name === "Skills" ? (
        <>
          <ExtensionSectionSlot
            registry={registry}
            host="skills.section"
            cwd={cwd}
            connection={connection}
            workspaces={workspaces}
          />
          <LocalSkillsView
            items={skills.catalog}
            search={skills.search}
            loading={skills.loading}
            onSearchChange={skills.setSearch}
            onRefresh={() => void skills.refresh()}
            onManage={skills.manageSkill}
            home={home}
          />
        </>
      ) : name === "Extensions" ? (
        <ExtensionsView
          snapshot={extensionSnapshot}
          onSetEnabled={(extensionId, enabled) =>
            void setExtensionEnabled(extensionId, enabled)
          }
          onRefresh={refreshExtensions}
        />
      ) : null}
    </PageFrame>
  );
}
