const HERDR_CONTRACT = Object.freeze({
  // The release sushiAI installs and was verified against. Any other build
  // whose schema still offers everything below is used as it is: a version
  // or protocol number alone never blocks a session (see
  // herdr-compatibility.cjs).
  version: "0.9.3",
  protocol: 22,
  // The oldest protocol whose schema this contract was checked on (0.8.2).
  minProtocol: 20,
  schemaVersion: 1,
  sourceCommit: "7b116c05bfda646af39d2524c54e70c751f57ee8",
  release: "https://github.com/herdrdev/herdr/releases/tag/v0.9.3",
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
        "4de7aa3e25678812e92960de64f7c2aaa1bca1f0f80a3c5e559837e231e1f5c0",
    }),
    "linux-x64": Object.freeze({
      name: "herdr-linux-x86_64",
      sha256:
        "18a8dc65f1c2fa485884344356dea1cfd911c6f06cf46fa78e193f4087f4dba7",
    }),
    "darwin-arm64": Object.freeze({
      name: "herdr-macos-aarch64",
      sha256:
        "5173a3e0ae42d5d1ab7ebfa5d5e6329f7c3d23f8e1a3677c7ce3231da2884157",
    }),
    "darwin-x64": Object.freeze({
      name: "herdr-macos-x86_64",
      sha256:
        "db62d548ff3e832b087a96b1894a08d26be3905f1830309cd556783f215d4054",
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
