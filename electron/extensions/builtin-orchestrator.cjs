// The built-in orchestrator's manifest. The object itself still lives in
// ../orchestrator.cjs (step 6 owns that file) and is completed here; lane L6
// flips the ownership so this file defines it and orchestrator.cjs imports it.
const { ORCHESTRATOR_MANIFEST } = require("../orchestrator.cjs");
const { CONTRACT } = require("./manifest.cjs");

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
    "How tasks are planned, run, checked and landed. Saved to orchd for every project on this Mac.",
  allowedHosts: ["settings.page"],
  defaultHost: "settings.page",
  instancePolicy: "singleton",
  stateScope: "global",
  view: { kind: "core", viewId: "orchestrator.settings" },
};

ORCHESTRATOR_MANIFEST.contributions = {
  // TEMPORARY: the validator learns `settings.page` in lane L1. Until that is
  // merged the surface is left out so the manifest still validates. Delete the
  // condition after L1 lands.
  surfaces: CONTRACT.HOSTS.has("settings.page") ? [pane, settings] : [pane],
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
};

module.exports = { ORCHESTRATOR_MANIFEST };
