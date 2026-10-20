const ARTIFACTS_MANIFEST = {
  id: "builtin.artifacts",
  name: "Artifacts",
  version: "1.0.0",
  apiVersion: 1,
  source: { kind: "builtin" },
  scope: "app",
  icon: "file-text",
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

/** The skill this extension ships: its name, its file in this folder. */
const ARTIFACTS_SKILL = {
  name: "sushiai-artifacts",
  file: "artifacts-skill.md",
};

module.exports = { ARTIFACTS_MANIFEST, ARTIFACTS_SKILL };
