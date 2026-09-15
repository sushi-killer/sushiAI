const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const path = require("node:path");

const appFile = path.join(__dirname, "..", "src", "App.tsx");
const source = readFileSync(appFile, "utf8");

test("App.tsx stays a composition layer", () => {
  const lines = source.split("\n").length;
  assert.ok(
    lines <= 600,
    `src/App.tsx is ${lines} lines; the shell must stay at or below 600`,
  );
});

test("App.tsx owns no domain implementation", () => {
  // Each of these belongs to a hook or a component. Finding one here means a
  // domain leaked back into the shell.
  const forbidden = [
    ["localStorage", "persistence lives in workspaceState/useAppPersistence"],
    ["window.bridge.herdr(", "Herdr calls live in useHerdr/useWorkspaces"],
    ["reconcileHerdrWorkspaces", "snapshot reconciliation lives in useHerdr"],
    ["disposeTerminal", "terminal teardown lives in useWorkspaces"],
    ["terminalClose", "terminal teardown lives in useWorkspaces"],
    ["modelProfilesList", "model profiles belong to the panel picker"],
    ["claudePluginsToggle", "plugin toggles live in ClaudeMcpSettings"],
    ["checkout", "branch checkout is a main-process operation only"],
  ];
  for (const [needle, why] of forbidden)
    assert.equal(
      source.includes(needle),
      false,
      `src/App.tsx must not contain "${needle}": ${why}`,
    );
});

test("App.tsx stores the workspace list but never edits it", () => {
  // The list lives here because useHerdr and useWorkspaces both write to it;
  // App hands the setter to those two and touches it nowhere else.
  const mentions = source.match(/setWorkspaces\b/g) || [];
  assert.equal(
    mentions.length,
    3,
    `setWorkspaces appears ${mentions.length} times in App.tsx; expected the ` +
      "useState declaration plus one handoff to each owning hook",
  );
  assert.equal(
    /setWorkspaces\(/.test(source),
    false,
    "App.tsx must not call setWorkspaces itself",
  );
});

test("the renderer runs no external code", () => {
  for (const pattern of [
    /\beval\(/,
    /new Function\(/,
    /dangerouslySetInnerHTML/,
  ]) {
    for (const file of ["App.tsx"])
      assert.equal(
        pattern.test(
          readFileSync(path.join(__dirname, "..", "src", file), "utf8"),
        ),
        false,
        `${file} must not use ${pattern}`,
      );
  }
});
