// The built-in orchestrator's manifest: the one owner of this object.
const pane = {
  id: "orchestration",
  title: "Orchestrator",
  description: "Tasks carried to done",
  allowedHosts: ["workspace.pane"],
  defaultHost: "workspace.pane",
  instancePolicy: "multiple",
  stateScope: "instance",
  view: { kind: "core", viewId: "orchestrator.panel" },
};

const settings = {
  id: "settings",
  title: "Orchestration",
  description:
    "How tasks are planned, run, checked and landed. Saved for every project on this Mac.",
  allowedHosts: ["settings.page"],
  defaultHost: "settings.page",
  instancePolicy: "singleton",
  stateScope: "global",
  view: { kind: "core", viewId: "orchestrator.settings" },
};

const ORCHESTRATOR_MANIFEST = {
  id: "builtin.orchestrator",
  name: "Orchestrator",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  description: "Carries tasks to a verified, committed done.",
  contributions: {
    surfaces: [pane, settings],
    navigation: [
      {
        id: "orchestration-picker",
        targetSurfaceId: "orchestration",
        allowedPlacements: ["panel.picker"],
        defaultPlacement: "panel.picker",
        label: "Orchestrator",
        icon: "list-todo",
      },
    ],
    actions: [],
    commands: [],
  },
};

module.exports = { ORCHESTRATOR_MANIFEST };
