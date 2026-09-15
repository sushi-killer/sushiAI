const { test } = require("node:test");
const assert = require("node:assert/strict");

const view = import("../src/workspace/projectView.ts");
const state = import("../src/workspaceState.ts");

test("viewFor: a project never opened before starts split, nothing maximized", async () => {
  const { viewFor } = await view;
  assert.deepEqual(viewFor({}, "w-new"), { tabMode: false, zoomed: null });
  const views = { "w-docs": { tabMode: true, zoomed: "p-2" } };
  assert.deepEqual(viewFor(views, "w-docs"), { tabMode: true, zoomed: "p-2" });
});

test("rememberView keeps each project's own view and skips no-op writes", async () => {
  const { rememberView } = await view;
  const views = {
    "group:example.test/team/app::": { tabMode: false, zoomed: null },
    "w-docs": { tabMode: true, zoomed: "p-2" },
  };
  assert.equal(
    rememberView(views, "w-docs", { tabMode: true, zoomed: "p-2" }),
    views,
    "an unchanged view returns the same object",
  );
  const next = rememberView(views, "w-docs", { tabMode: false, zoomed: null });
  assert.deepEqual(next["w-docs"], { tabMode: false, zoomed: null });
  assert.equal(
    next["group:example.test/team/app::"],
    views["group:example.test/team/app::"],
    "another project's view is untouched",
  );
});

test("restore keeps well-formed project views and drops the rest", async () => {
  const { restore } = await state;
  const saved = {
    workspaces: [
      {
        id: "w-1",
        name: "app",
        cwd: "/Users/dev/app",
        panels: [],
        layout: null,
      },
    ],
    activeId: "w-1",
    views: {
      "w-1": { tabMode: true, zoomed: "p-1" },
      "w-2": { tabMode: "yes", zoomed: 7 },
      "w-3": null,
    },
  };
  const restored = restore({ getItem: () => JSON.stringify(saved) });
  assert.deepEqual(restored.views, {
    "w-1": { tabMode: true, zoomed: "p-1" },
    "w-2": { tabMode: false, zoomed: null },
  });
  const empty = restore({
    getItem: () => JSON.stringify({ ...saved, views: ["not", "a map"] }),
  });
  assert.deepEqual(empty.views, {});
});
