const test = require("node:test");
const assert = require("node:assert/strict");
const {
  groupAlsoInProject,
  unknownMcpVariables,
} = require("../src/projectMcp.ts");

test("project MCP variables are checked against Environment names", () => {
  const servers = {
    github: { env: { token: "${GITHUB_TOKEN}" } },
    sentry: { url: "https://example.test/${UNKNOWN_KEY}" },
  };
  assert.deepEqual(unknownMcpVariables(servers, [{ name: "GITHUB_TOKEN" }]), [
    "UNKNOWN_KEY",
  ]);
});

test("Also in this project keeps repo config read-only and personal sources separate", () => {
  const repo = { name: "repo", source: "project" };
  const personal = { name: "personal", source: "user" };
  const plugin = { name: "plugin" };
  assert.deepEqual(groupAlsoInProject([repo, personal], [plugin]), {
    repo: [repo],
    personal: [personal],
    plugins: [plugin],
  });
});
