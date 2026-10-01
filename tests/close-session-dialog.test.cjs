const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { createRequire } = require("node:module");
const React = require("react");
const ts = require("typescript");
const { renderToStaticMarkup } = require("react-dom/server");

const entry = path.resolve(__dirname, "../src/dialogs/CloseSessionDialog.tsx");
const source = fs.readFileSync(entry, "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.CommonJS,
    target: ts.ScriptTarget.ES2022,
    jsx: ts.JsxEmit.ReactJSX,
  },
}).outputText;
const componentExports = {};
const requireSource = createRequire(entry);
vm.runInNewContext(
  compiled,
  {
    exports: componentExports,
    require: (name) =>
      name === "../app/worktreeCleanup"
        ? require("../src/app/worktreeCleanup.ts")
        : requireSource(name),
    window: { bridge: {} },
  },
  { filename: entry },
);

function render({ cwd = "/tmp/project", git } = {}) {
  const workspace = {
    id: "workspace",
    name: "Project",
    cwd,
    panels: [{ id: "pane", herdrId: "session" }],
  };
  const panel = workspace.panels[0];
  return renderToStaticMarkup(
    React.createElement(componentExports.CloseSessionDialog, {
      workspace,
      panel,
      workspaces: [workspace],
      projectGit: git ? { [workspace.id]: git } : {},
      hidePanel: () => {},
      endSessions: async () => {},
      onClose: () => {},
    }),
  );
}

test("close dialog waits for Git identity before ending a session", () => {
  const markup = render();
  assert.match(markup, /Checking workspace Git status/);
  assert.match(markup, /<button class="danger" disabled="">/);

  const ready = render({ git: { linkedWorktree: false } });
  assert.doesNotMatch(ready, /<button class="danger" disabled="">/);

  const noCwd = render({ cwd: "" });
  assert.doesNotMatch(noCwd, /Checking workspace Git status/);
  assert.doesNotMatch(noCwd, /<button class="danger" disabled="">/);
});

test("close dialog waits for PR status before enabling session end", () => {
  const markup = render({
    git: {
      checkout: "/tmp/checkout",
      commonDir: "/tmp/checkout/.git",
      branch: "feature/task",
      linkedWorktree: true,
    },
  });
  assert.match(markup, /Delete this worktree after ending the session/);
  assert.match(
    markup,
    /Checking whether its pull request was merged and closed/,
  );
  assert.match(markup, /<button class="danger" disabled="">/);
});
