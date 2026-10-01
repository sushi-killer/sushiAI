const HERDR_CONTRACT = Object.freeze({
  version: "0.8.2",
  protocol: 20,
  schemaVersion: 1,
  sourceCommit: "9eb521456ac0d19d3ab3d9d7cea3cca10baa8a4c",
  release: "https://github.com/herdrdev/herdr/releases/tag/v0.8.2",
  requiredMethods: Object.freeze([
    "ping",
    "session.snapshot",
    "workspace.create",
    "workspace.rename",
    "workspace.close",
    "worktree.create",
    "pane.rename",
    "pane.split",
    "pane.read",
    "pane.send_text",
    "pane.send_keys",
    "pane.send_input",
    "pane.close",
    "pane.process_info",
    "events.subscribe",
  ]),
  launchEnvMethods: Object.freeze(["WorkspaceCreateParams", "PaneSplitParams"]),
  streamCommand: Object.freeze(["terminal", "session", "control"]),
  eventTypes: Object.freeze([
    "workspace.created",
    "workspace.updated",
    "workspace.metadata_updated",
    "workspace.renamed",
    "workspace.moved",
    "workspace.reordered",
    "workspace.closed",
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.moved",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
  ]),
  artifacts: Object.freeze({
    "linux-arm64": Object.freeze({
      name: "herdr-linux-aarch64",
      sha256:
        "f55610658e1c2e0d2aaef730b4b2ab885f7f8ba00285ab372bfb14f2e3d5b40d",
    }),
    "linux-x64": Object.freeze({
      name: "herdr-linux-x86_64",
      sha256:
        "976150a14d490c94b243ea2e1a7eb2dfb67f12e36b182db90936f6728e6aecf4",
    }),
    "darwin-arm64": Object.freeze({
      name: "herdr-macos-aarch64",
      sha256:
        "a5d4f4d504d8b309c91f811050559300faba31258425f53c50852fc96f6ae574",
    }),
    "darwin-x64": Object.freeze({
      name: "herdr-macos-x86_64",
      sha256:
        "ab50262c8190cd7aa9056d249d255c08c328c3e8716de9cfa29db4f131b8e2c1",
    }),
  }),
});

function releaseArtifact(platform = process.platform, arch = process.arch) {
  const artifact = HERDR_CONTRACT.artifacts[`${platform}-${arch}`];
  if (!artifact)
    throw new Error(`No verified Herdr release for ${platform}/${arch}.`);
  return {
    ...artifact,
    url: `https://github.com/herdrdev/herdr/releases/download/v${HERDR_CONTRACT.version}/${artifact.name}`,
  };
}

module.exports = { HERDR_CONTRACT, releaseArtifact };
