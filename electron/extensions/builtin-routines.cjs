// The built-in Routines module's manifest: the one owner of this object. It
// contributes one page (the Routines sidebar entry) and one Settings tab (the
// routine builder). The views are core views, drawn by src/routines.
const ROUTINES_MANIFEST = {
  id: "builtin.routines",
  name: "Routines",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  icon: "workflow",
  description:
    "Scheduled background agent runs, built from one sentence and run by the daemon of a host.",
  contributions: {
    surfaces: [
      {
        id: "page",
        title: "Routines",
        description: "Agent work that runs on a schedule, with its results.",
        allowedHosts: ["app.page"],
        defaultHost: "app.page",
        instancePolicy: "singleton",
        stateScope: "global",
        view: { kind: "core", viewId: "routines.page" },
      },
      {
        id: "settings",
        title: "Routines",
        description:
          "Who builds your routines and what it may see. Saved for every project on this Mac.",
        allowedHosts: ["settings.page"],
        defaultHost: "settings.page",
        instancePolicy: "singleton",
        stateScope: "global",
        view: { kind: "core", viewId: "routines.settings" },
      },
    ],
    navigation: [
      {
        id: "routines-page",
        targetSurfaceId: "page",
        allowedPlacements: ["sidebar.primary"],
        defaultPlacement: "sidebar.primary",
        label: "Routines",
        order: -10,
        icon: "workflow",
      },
    ],
    actions: [],
    commands: [],
  },
};

/** The skill this extension ships: its name, its file in this folder. */
const ROUTINES_SKILL = {
  name: "sushiai-routines",
  file: "routines-skill.md",
};

module.exports = { ROUTINES_MANIFEST, ROUTINES_SKILL };
