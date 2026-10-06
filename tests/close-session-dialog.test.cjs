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
        : name === "../orchestrator/ui"
          ? // JSX sources cannot be type-stripped; the dialog only needs
            // their markup shape here.
            {
              Tag: ({ children }) =>
                React.createElement("span", { className: "ui-tag" }, children),
              Toggle: ({ checked, label, disabled }) =>
                React.createElement("input", {
                  type: "checkbox",
                  "aria-label": label,
                  checked,
                  disabled,
                  readOnly: true,
                }),
            }
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
      endSessions: async () => {},
      onClose: () => {},
    }),
  );
}

test("close dialog waits for Git identity before ending a session", () => {
  const markup = render();
  assert.match(markup, /Checking workspace Git status/);
  assert.match(markup, /<button class="ui-button danger" disabled="">/);

  const ready = render({ git: { linkedWorktree: false } });
  assert.doesNotMatch(ready, /<button class="ui-button danger" disabled="">/);

  const noCwd = render({ cwd: "" });
  assert.doesNotMatch(noCwd, /Checking workspace Git status/);
  assert.doesNotMatch(noCwd, /<button class="ui-button danger" disabled="">/);
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
  assert.match(markup, /Delete worktree feature\/task/);
  assert.match(markup, /Checking whether its pull request was merged/);
  assert.match(markup, /<button class="ui-button danger" disabled="">/);
});

test("close dialog offers only Close session and Cancel", () => {
  const markup = render({ git: { linkedWorktree: false } });
  assert.match(markup, />Cancel</);
  assert.match(markup, />Close session</);
  assert.doesNotMatch(markup, /Hide only/);
});
