const ARTIFACTS_MANIFEST = {
  id: "builtin.artifacts",
  name: "Artifacts",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  description:
    "Opens what an agent wrote - docs, plans, decks, diagrams - next to its pane.",
  contributions: {
    surfaces: [
      {
        id: "preview",
        title: "Preview",
        allowedHosts: ["workspace.pane"],
        defaultHost: "workspace.pane",
        instancePolicy: "multiple",
        stateScope: "instance",
        view: { kind: "core", viewId: "artifacts.preview" },
      },
    ],
    navigation: [],
    actions: [],
    commands: [],
  },
};

module.exports = { ARTIFACTS_MANIFEST };
